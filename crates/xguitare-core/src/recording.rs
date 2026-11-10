use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::Path;
use std::sync::Arc;

use hound::WavReader;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::control::slug;
use crate::project::{Item, Project, TAKES_DIR, Take, Track};

/// A recording ring holds at least this many seconds, so the disk thread may fall that far behind before frames drop.
pub const RING_SECONDS: u32 = 2;
const MARK_CAPACITY: usize = 64;
const UNNAMED_TRACK: &str = "track";

/// One stretch of a Track's recording that becomes one Take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pass {
    /// The playhead at the pass's first recorded frame.
    pub start: u64,
    /// The frames that reached the ring; frames dropped on overflow are not counted.
    pub frames: u64,
    /// True when the pass began at a loop wrap of the same recording.
    pub loop_pass: bool,
}

#[derive(Debug, Clone, Copy)]
enum Mark {
    PassEnd(Pass),
    HandedOver,
}

/// Where [`TakeReader::read`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStop {
    /// The ring holds nothing more for now.
    Empty,
    /// The samples read were the last of this pass.
    PassEnd(Pass),
    /// The recorder handed its open pass to the one that replaced it, or was dropped: this reader gets nothing more.
    Done,
}

/// The audio thread's end of one Track's recording ring. Allocates nothing after [`take_ring`].
pub struct TakeRecorder {
    samples: Producer<f32>,
    marks: Producer<Mark>,
    open: Option<Pass>,
    loop_next: bool,
    overflows: u64,
    handed_over: bool,
}

/// The disk thread's end of one Track's recording ring.
pub struct TakeReader {
    samples: Consumer<f32>,
    marks: Consumer<Mark>,
}

/// A pass the disk thread wrote to `file` (relative to the Project folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedTake {
    pub track: usize,
    pub file: String,
    pub pass: Pass,
}

/// JACK latencies in frames: of the capture port feeding the Track, and of the playback port the output feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Latency {
    pub capture: u32,
    pub playback: u32,
}

/// A recording ring of `capacity` samples.
pub fn take_ring(capacity: usize) -> (TakeRecorder, TakeReader) {
    let (samples_tx, samples_rx) = RingBuffer::new(capacity);
    let (marks_tx, marks_rx) = RingBuffer::new(MARK_CAPACITY);
    (
        TakeRecorder {
            samples: samples_tx,
            marks: marks_tx,
            open: None,
            loop_next: false,
            overflows: 0,
            handed_over: false,
        },
        TakeReader {
            samples: samples_rx,
            marks: marks_rx,
        },
    )
}

impl TakeRecorder {
    /// Appends `samples`, recorded from playhead frame `at`, to the open pass, opening one at `at` when none is open; what does not fit is dropped and counted as one overflow. Does nothing once this recorder handed over to another.
    pub fn push(&mut self, at: u64, samples: &[f32]) {
        if self.handed_over {
            return;
        }
        let loop_pass = self.loop_next;
        let pass = self.open.get_or_insert(Pass {
            start: at,
            frames: 0,
            loop_pass,
        });
        let (pushed, dropped) = self.samples.push_partial_slice(samples);
        pass.frames += pushed.len() as u64;
        if !dropped.is_empty() {
            self.overflows += 1;
        }
    }

    /// Ends the open pass, if any; `wrapped` makes the next pass a loop pass.
    pub fn end_pass(&mut self, wrapped: bool) {
        let Some(pass) = self.open.take() else {
            self.loop_next = false;
            return;
        };
        self.loop_next = wrapped;
        self.mark(Mark::PassEnd(pass));
    }

    /// Takes over the open pass of the recorder this one replaces; `previous` records nothing more.
    pub fn continue_from(&mut self, previous: &mut TakeRecorder) {
        self.open = previous.open.take();
        self.loop_next = previous.loop_next;
        self.overflows += previous.take_overflows();
        previous.handed_over = true;
        previous.mark(Mark::HandedOver);
    }

    /// The overflows since the last call.
    pub fn take_overflows(&mut self) -> u64 {
        std::mem::take(&mut self.overflows)
    }

    fn mark(&mut self, mark: Mark) {
        if self.marks.push(mark).is_err() {
            self.overflows += 1;
        }
    }
}

impl TakeReader {
    /// Replaces `out` with the ring's samples of the current pass, of which `pass_written` were read before (through this reader or the ones it follows), stopping at that pass's end.
    pub fn read(&mut self, pass_written: u64, out: &mut Vec<f32>) -> ReadStop {
        out.clear();
        let abandoned = self.samples.is_abandoned();
        let mark = self.marks.peek().ok().copied();
        let available = self.samples.slots();
        let wanted = match mark {
            Some(Mark::PassEnd(pass)) => available.min(
                usize::try_from(pass.frames.saturating_sub(pass_written)).unwrap_or(usize::MAX),
            ),
            _ => available,
        };
        if let Ok(chunk) = self.samples.read_chunk(wanted) {
            out.extend(chunk);
        }
        match mark {
            Some(Mark::PassEnd(pass)) if pass_written + out.len() as u64 == pass.frames => {
                let _ = self.marks.pop();
                ReadStop::PassEnd(pass)
            }
            Some(Mark::HandedOver) if self.samples.is_empty() => ReadStop::Done,
            None if abandoned && self.samples.is_empty() => ReadStop::Done,
            _ => ReadStop::Empty,
        }
    }
}

/// The timeline frame where a Take recorded from playhead `record_start` belongs: earlier by the capture and playback latency and by `offset_ms` of the Project's latency offset, never before frame 0.
pub fn take_position(
    record_start: u64,
    capture_latency: u32,
    playback_latency: u32,
    offset_ms: f64,
    rate: u32,
) -> u64 {
    let offset = (offset_ms * f64::from(rate) / 1000.0).round() as i128;
    let compensation = i128::from(capture_latency) + i128::from(playback_latency) + offset;
    let position = i128::from(record_start) - compensation;
    u64::try_from(position.max(0)).unwrap_or(u64::MAX)
}

/// Adds `take` at `position`: a loop pass at the position of the Track's last Item becomes that Item's newest, active Take; anything else becomes a new Item.
pub fn add_take(track: &mut Track, position: u64, take: Take, loop_pass: bool) {
    if loop_pass
        && let Some(item) = track.items.last_mut()
        && item.position == position
    {
        item.takes.push(take);
        item.take = item.takes.len() - 1;
        return;
    }
    track.items.push(Item {
        position,
        take: 0,
        takes: vec![take],
    });
}

/// The first free `takes/<track slug>-<n>.wav` under `project_dir`, counting from 1.
pub fn next_take_file(project_dir: &Path, track_name: &str) -> String {
    let mut stem = slug(track_name);
    if stem.is_empty() {
        stem = UNNAMED_TRACK.to_string();
    }
    let mut n = 1u64;
    loop {
        let file = format!("{TAKES_DIR}/{stem}-{n}.wav");
        if !project_dir.join(&file).exists() {
            return file;
        }
        n += 1;
    }
}

/// Reads the mono f32 Take `file` under `project_dir` into memory.
///
/// # Errors
///
/// The `hound` error when the file cannot be read, and `Unsupported` when it is not mono.
pub fn load_take(project_dir: &Path, file: &str) -> Result<Arc<[f32]>, hound::Error> {
    let reader = WavReader::open(project_dir.join(file))?;
    if reader.spec().channels != 1 {
        return Err(hound::Error::Unsupported);
    }
    let samples = reader
        .into_samples::<f32>()
        .collect::<Result<Vec<_>, _>>()?;
    Ok(samples.into())
}

/// Loads a finished pass into `takes` and adds it as a Take on its Track at its latency-compensated position; `None` when that Track no longer exists.
///
/// # Errors
///
/// The `hound` error from [`load_take`]; the Project is then unchanged.
pub fn add_finished_take<S: BuildHasher>(
    project: &mut Project,
    project_dir: &Path,
    takes: &mut HashMap<String, Arc<[f32]>, S>,
    finished: &FinishedTake,
    latency: Latency,
) -> Result<Option<u64>, hound::Error> {
    let position = take_position(
        finished.pass.start,
        latency.capture,
        latency.playback,
        project.latency_offset_ms,
        project.sample_rate,
    );
    let Some(track) = project.tracks.get_mut(finished.track) else {
        return Ok(None);
    };
    let samples = load_take(project_dir, &finished.file)?;
    let take = Take {
        file: finished.file.clone(),
        frames: samples.len() as u64,
    };
    takes.insert(finished.file.clone(), samples);
    add_take(track, position, take, finished.pass.loop_pass);
    Ok(Some(position))
}
