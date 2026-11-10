use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use hound::{SampleFormat, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};

use crate::Config;
use crate::arrangement::{Arrangement, ArrangementCommand};
use crate::build::{ArrangementError, build_arrangement};
use crate::control::has_no_hard_links;
use crate::project::Project;

/// The most frames one [`Render::step`] renders.
pub const RENDER_BLOCK: usize = 1024;
/// The folder under the Project folder that Renders are written to.
pub const RENDERS_DIR: &str = "renders";
pub const PART_EXTENSION: &str = "part";
const WAV_EXTENSION: &str = "wav";
/// Master output below this level counts as quiet for the tail rule (−90 dBFS).
const TAIL_QUIET_LEVEL: f32 = 3.162_277_7e-5;
const TAIL_QUIET_SECONDS: f32 = 0.5;
const TAIL_MAX_SECONDS: u32 = 10;
const BAD_BITS: &str = "bits must be 16, 24 or \"32f\"";
const FLOAT_BITS: &str = "32f";

/// What a Render plays: the Project from frame 0 to the end of its last Item plus the tail, or the loop region exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderRange {
    Project,
    Loop,
}

/// The sample format of the WAV a Render writes; on the wire `16`, `24` or `"32f"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BitsOnWire", into = "BitsOnWire")]
pub enum RenderBits {
    /// 16-bit integer with TPDF dither.
    Pcm16,
    Pcm24,
    Float32,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum BitsOnWire {
    Depth(u32),
    Word(String),
}

#[derive(Debug)]
pub enum RenderError {
    NothingToRender,
    NoLoopRegion,
    Build(ArrangementError),
}

pub enum RenderStep<'a> {
    Audio { left: &'a [f32], right: &'a [f32] },
    Done,
}

/// A Render job with no thread and no file: its own Arrangement of a copy of the Project, played from the start of the range with no live input and no click, armed Tracks playing their Items.
pub struct Render {
    arrangement: Arrangement,
    bits: RenderBits,
    sample_rate: u32,
    left: Vec<f32>,
    right: Vec<f32>,
    /// Frames of the range itself, before any tail.
    main_frames: u64,
    rendered: u64,
    phase: Phase,
}

type WavFile = WavWriter<BufWriter<File>>;

#[derive(Clone, Copy)]
enum Phase {
    Main { tail: bool },
    Tail { frames: u64, quiet: u64 },
    Done,
}

impl Render {
    /// Builds the Render of `project` (its in-memory state, unsaved changes included) at the Project's sample rate from the loaded Take audio in `takes`.
    ///
    /// # Errors
    ///
    /// [`RenderError::NothingToRender`] when no Track has an Item, [`RenderError::NoLoopRegion`] for the loop range when the loop start is not before its end, and [`RenderError::Build`] when the Arrangement does not build.
    pub fn new<S: std::hash::BuildHasher>(
        config: &Config,
        project: &Project,
        takes: &HashMap<String, Arc<[f32]>, S>,
        range: RenderRange,
        bits: RenderBits,
    ) -> Result<Render, RenderError> {
        if project.tracks.iter().all(|track| track.items.is_empty()) {
            return Err(RenderError::NothingToRender);
        }
        let region = project.loop_region;
        let (start, main_frames) = match range {
            RenderRange::Project => (0, last_item_end(project)),
            RenderRange::Loop if region.start < region.end => {
                (region.start, region.end - region.start)
            }
            RenderRange::Loop => return Err(RenderError::NoLoopRegion),
        };
        let mut copy = project.clone();
        copy.loop_region.on = false;
        for track in &mut copy.tracks {
            track.arm = false;
        }
        let sample_rate = project.sample_rate;
        let (mut arrangement, _) =
            build_arrangement(config, &copy, takes, sample_rate as f32, RENDER_BLOCK)
                .map_err(RenderError::Build)?;
        arrangement.apply(ArrangementCommand::Locate(start));
        arrangement.apply(ArrangementCommand::Play);
        let tail = range == RenderRange::Project;
        Ok(Render {
            arrangement,
            bits,
            sample_rate,
            left: vec![0.0; RENDER_BLOCK],
            right: vec![0.0; RENDER_BLOCK],
            main_frames,
            rendered: 0,
            phase: if main_frames == 0 && tail {
                Phase::Tail {
                    frames: 0,
                    quiet: 0,
                }
            } else {
                Phase::Main { tail }
            },
        })
    }

    pub fn bits(&self) -> RenderBits {
        self.bits
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The share of the range rendered, 0 to 1; the tail does not count, and 1 once done.
    pub fn progress(&self) -> f32 {
        if matches!(self.phase, Phase::Done) || self.main_frames == 0 {
            return 1.0;
        }
        (self.rendered as f64 / self.main_frames as f64).min(1.0) as f32
    }

    /// Renders up to `frames` (at most [`RENDER_BLOCK`]) more frames of Master output; the tail ends once Master stays below −90 dBFS for 0.5 s, or after 10 s.
    pub fn step(&mut self, frames: usize) -> RenderStep<'_> {
        let frames = frames.min(RENDER_BLOCK);
        let want = match self.phase {
            Phase::Main { .. } => frames.min(clamp_frames(self.main_frames - self.rendered)),
            Phase::Tail { frames: done, .. } => {
                frames.min(clamp_frames(self.tail_max_frames() - done))
            }
            Phase::Done => return RenderStep::Done,
        };
        let (left, right) = (&mut self.left[..want], &mut self.right[..want]);
        self.arrangement.process(&[], left, right);
        let len = match self.phase {
            Phase::Main { tail } => {
                self.rendered += want as u64;
                if self.rendered == self.main_frames {
                    self.phase = if tail {
                        Phase::Tail {
                            frames: 0,
                            quiet: 0,
                        }
                    } else {
                        Phase::Done
                    };
                }
                want
            }
            Phase::Tail { frames, quiet } => self.follow_tail(frames, quiet, want),
            Phase::Done => 0,
        };
        RenderStep::Audio {
            left: &self.left[..len],
            right: &self.right[..len],
        }
    }

    /// Counts the quiet frames of the tail block just rendered; returns how many of its `len` frames belong to the Render.
    fn follow_tail(&mut self, done: u64, mut quiet: u64, len: usize) -> usize {
        let quiet_frames = (self.sample_rate as f32 * TAIL_QUIET_SECONDS) as u64;
        for (idx, (left, right)) in self.left[..len].iter().zip(&self.right[..len]).enumerate() {
            if left.abs().max(right.abs()) < TAIL_QUIET_LEVEL {
                quiet += 1;
            } else {
                quiet = 0;
            }
            if quiet == quiet_frames {
                self.phase = Phase::Done;
                return idx + 1;
            }
        }
        let frames = done + len as u64;
        self.phase = if frames == self.tail_max_frames() {
            Phase::Done
        } else {
            Phase::Tail { frames, quiet }
        };
        len
    }

    fn tail_max_frames(&self) -> u64 {
        u64::from(self.sample_rate) * u64::from(TAIL_MAX_SECONDS)
    }
}

fn clamp_frames(frames: u64) -> usize {
    usize::try_from(frames).unwrap_or(usize::MAX)
}

/// The frame after the last one any Item's active Take plays.
fn last_item_end(project: &Project) -> u64 {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.items)
        .map(|item| {
            let frames = item.takes[item.take].frames.saturating_sub(item.offset);
            item.position.saturating_add(frames)
        })
        .max()
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderState {
    Running,
    Done,
    Cancelled,
    Failed,
}

/// One Render's progress and outcome, shared by the Control API and the thread that writes it.
pub struct RenderJob {
    pub name: String,
    pub range: RenderRange,
    renders: PathBuf,
    status: Mutex<JobStatus>,
}

#[derive(Clone)]
struct JobStatus {
    /// The file it writes; moves on to the next free `-N` name when a file or part file appears on the way.
    file: PathBuf,
    number: u64,
    state: RenderState,
    progress: f32,
    error: Option<String>,
    cancel: bool,
}

impl RenderJob {
    /// The job of a Render named `name` into the folder `renders`, writing the first of `<name>.wav`, `<name>-2.wav`, … that exists neither as a file nor as a part file.
    pub fn new(name: String, range: RenderRange, renders: PathBuf) -> RenderJob {
        let number = first_free(&renders, &name, 1);
        RenderJob {
            status: Mutex::new(JobStatus {
                file: numbered_file(&renders, &name, number),
                number,
                state: RenderState::Running,
                progress: 0.0,
                error: None,
                cancel: false,
            }),
            name,
            range,
            renders,
        }
    }

    pub fn file(&self) -> PathBuf {
        self.status().file.clone()
    }

    /// Moves on to the next free `-N` file and returns it.
    fn next_file(&self) -> PathBuf {
        let mut status = self.status();
        let number = first_free(&self.renders, &self.name, status.number + 1);
        status.number = number;
        status.file = numbered_file(&self.renders, &self.name, number);
        status.file.clone()
    }

    /// Asks a running Render to stop; it removes its part file.
    pub fn cancel(&self) {
        self.status().cancel = true;
    }

    /// Ends a Render that never ran with `error`.
    pub fn fail(&self, error: String) {
        let mut status = self.status();
        status.state = RenderState::Failed;
        status.error = Some(error);
    }

    pub fn state(&self) -> RenderState {
        self.status().state
    }

    pub fn progress(&self) -> f32 {
        self.status().progress
    }

    pub fn error(&self) -> Option<String> {
        self.status().error.clone()
    }

    fn status(&self) -> std::sync::MutexGuard<'_, JobStatus> {
        self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A Render and its job, for the Daemon to run on a thread of its own.
pub struct RenderTask {
    pub render: Render,
    pub job: Arc<RenderJob>,
}

impl RenderTask {
    /// Runs [`run_render`] to the end.
    pub fn run(self) {
        run_render(self.render, &self.job, |part, file| {
            fs::hard_link(part, file)
        });
    }
}

/// `file` with `.part` appended, which a Render writes before linking it to `file`.
pub fn part_file(file: &Path) -> PathBuf {
    let mut part = file.as_os_str().to_owned();
    part.push(".");
    part.push(PART_EXTENSION);
    PathBuf::from(part)
}

/// `renders/<name>.wav` for `number` 1, `renders/<name>-<number>.wav` after.
fn numbered_file(renders: &Path, name: &str, number: u64) -> PathBuf {
    if number == 1 {
        renders.join(format!("{name}.{WAV_EXTENSION}"))
    } else {
        renders.join(format!("{name}-{number}.{WAV_EXTENSION}"))
    }
}

/// The first number from `number` whose file is free.
fn first_free(renders: &Path, name: &str, mut number: u64) -> u64 {
    while !is_free(&numbered_file(renders, name, number)) {
        number += 1;
    }
    number
}

/// Whether neither `file` nor its part file exists, also as a symlink.
fn is_free(file: &Path) -> bool {
    fs::symlink_metadata(file).is_err() && fs::symlink_metadata(part_file(file)).is_err()
}

/// Writes `render` to a new part file, links it to the job's file without replacing anything there and removes it, also on cancel or error, and records the outcome in `job`.
pub fn run_render(
    mut render: Render,
    job: &RenderJob,
    link: impl Fn(&Path, &Path) -> io::Result<()>,
) {
    let outcome =
        open_part(job, render.bits().spec(render.sample_rate())).and_then(|(part, writer)| {
            let written = write_wav(&mut render, writer, &part, job).and_then(|finished| {
                if finished {
                    link_final(&part, job, link)?;
                }
                Ok(finished)
            });
            let _ = fs::remove_file(&part);
            written
        });
    let mut status = job.status();
    match outcome {
        Ok(true) => {
            status.state = RenderState::Done;
            status.progress = 1.0;
        }
        Ok(false) => status.state = RenderState::Cancelled,
        Err(error) => {
            status.state = RenderState::Failed;
            status.error = Some(error);
        }
    }
}

/// Creates the part file of the job's file, which must not exist (a symlink counts), moving on to the next `-N` file while one does.
fn open_part(job: &RenderJob, spec: WavSpec) -> Result<(PathBuf, WavFile), String> {
    let mut part = part_file(&job.file());
    loop {
        let cannot_write =
            |err: &dyn fmt::Display| format!("cannot write {}: {err}", part.display());
        match OpenOptions::new().write(true).create_new(true).open(&part) {
            Ok(file) => {
                return match WavWriter::new(BufWriter::new(file), spec) {
                    Ok(writer) => Ok((part, writer)),
                    Err(err) => {
                        let _ = fs::remove_file(&part);
                        Err(cannot_write(&err))
                    }
                };
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                part = part_file(&job.next_file());
            }
            Err(err) => return Err(cannot_write(&err)),
        }
    }
}

/// `link`s `part` to the job's file, which fails rather than replace a file there, moving on to the next `-N` file while one does. When the filesystem has no hard links, the file is created new and `part` renamed onto that empty file.
fn link_final(
    part: &Path,
    job: &RenderJob,
    link: impl Fn(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    let mut file = job.file();
    loop {
        match link(part, &file) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => file = job.next_file(),
            Err(err) if has_no_hard_links(&err) => return rename_final(part, job),
            Err(err) => {
                return Err(format!(
                    "cannot link {} to {}: {err}",
                    part.display(),
                    file.display()
                ));
            }
        }
    }
}

/// Creates the job's file new, moving on to the next `-N` file while one exists, and renames `part` onto it.
fn rename_final(part: &Path, job: &RenderJob) -> Result<(), String> {
    let mut file = job.file();
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&file) {
            Ok(_) => break,
            Err(err) if err.kind() == ErrorKind::AlreadyExists => file = job.next_file(),
            Err(err) => return Err(format!("cannot write {}: {err}", file.display())),
        }
    }
    fs::rename(part, &file).map_err(|err| {
        let _ = fs::remove_file(&file);
        format!(
            "cannot rename {} to {}: {err}",
            part.display(),
            file.display()
        )
    })
}

/// Returns `false` when the job was cancelled before the last frame.
fn write_wav(
    render: &mut Render,
    mut writer: WavFile,
    part: &Path,
    job: &RenderJob,
) -> Result<bool, String> {
    let cannot_write = |err: hound::Error| format!("cannot write {}: {err}", part.display());
    let mut dither = Dither::default();
    loop {
        if job.status().cancel {
            return Ok(false);
        }
        let bits = render.bits();
        let RenderStep::Audio { left, right } = render.step(RENDER_BLOCK) else {
            break;
        };
        for (&left, &right) in left.iter().zip(right) {
            for sample in [left, right] {
                write_sample(&mut writer, bits, sample, &mut dither).map_err(cannot_write)?;
            }
        }
        job.status().progress = render.progress();
    }
    writer.finalize().map_err(cannot_write)?;
    Ok(true)
}

fn write_sample(
    writer: &mut WavFile,
    bits: RenderBits,
    sample: f32,
    dither: &mut Dither,
) -> Result<(), hound::Error> {
    match bits {
        RenderBits::Pcm16 => {
            let scaled = sample.clamp(-1.0, 1.0) * f32::from(i16::MAX) + dither.next_tpdf();
            writer.write_sample(
                scaled
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
            )
        }
        RenderBits::Pcm24 => {
            const MAX_24: f32 = 8_388_607.0;
            writer.write_sample((sample.clamp(-1.0, 1.0) * MAX_24).round() as i32)
        }
        RenderBits::Float32 => writer.write_sample(sample),
    }
}

/// Triangular dither of ±1 LSB from a xorshift generator; a fixed seed, so a Render is repeatable.
struct Dither(u32);

impl Default for Dither {
    fn default() -> Self {
        Dither(0x9E37_79B9)
    }
}

impl Dither {
    fn next_uniform(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn next_tpdf(&mut self) -> f32 {
        self.next_uniform() - self.next_uniform()
    }
}

impl RenderBits {
    fn spec(self, sample_rate: u32) -> WavSpec {
        let (bits_per_sample, sample_format) = match self {
            RenderBits::Pcm16 => (16, SampleFormat::Int),
            RenderBits::Pcm24 => (24, SampleFormat::Int),
            RenderBits::Float32 => (32, SampleFormat::Float),
        };
        WavSpec {
            channels: 2,
            sample_rate,
            bits_per_sample,
            sample_format,
        }
    }
}

impl TryFrom<BitsOnWire> for RenderBits {
    type Error = &'static str;

    fn try_from(bits: BitsOnWire) -> Result<Self, Self::Error> {
        match bits {
            BitsOnWire::Depth(16) => Ok(RenderBits::Pcm16),
            BitsOnWire::Depth(24) => Ok(RenderBits::Pcm24),
            BitsOnWire::Word(word) if word == FLOAT_BITS => Ok(RenderBits::Float32),
            BitsOnWire::Depth(_) | BitsOnWire::Word(_) => Err(BAD_BITS),
        }
    }
}

impl From<RenderBits> for BitsOnWire {
    fn from(bits: RenderBits) -> Self {
        match bits {
            RenderBits::Pcm16 => BitsOnWire::Depth(16),
            RenderBits::Pcm24 => BitsOnWire::Depth(24),
            RenderBits::Float32 => BitsOnWire::Word(FLOAT_BITS.to_string()),
        }
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::NothingToRender => f.write_str("nothing to render"),
            RenderError::NoLoopRegion => f.write_str("no loop region"),
            RenderError::Build(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RenderError {}
