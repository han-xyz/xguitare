use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_4, SQRT_2, TAU};
use std::fmt;
use std::hash::BuildHasher;
use std::sync::Arc;

use xguitare_dsp::Rack;
use xguitare_dsp::smooth::Smoother;

use crate::project::{LoopRegion, Project, Track};
use crate::recording::{RING_SECONDS, TakeReader, TakeRecorder, take_ring};

const MIN_VOLUME_DB: f32 = -60.0;
const MAX_VOLUME_DB: f32 = 6.0;
const CLICK_SECONDS: f32 = 0.005;
const CLICK_HZ: f32 = 1000.0;
const ACCENT_HZ: f32 = 1500.0;
const CLICK_LEVEL: f32 = 0.5;
/// The blip decays to this share of its start level over its length.
const CLICK_END_LEVEL: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stopped,
    Playing,
    Recording,
}

/// A change the audio thread applies to the running Arrangement without a rebuild.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrangementCommand {
    Volume {
        track: usize,
        db: f32,
    },
    Pan {
        track: usize,
        pan: f32,
    },
    Mute {
        track: usize,
        on: bool,
    },
    Solo {
        track: usize,
        on: bool,
    },
    Arm {
        track: usize,
        on: bool,
    },
    Param {
        track: usize,
        module: usize,
        param: usize,
        value: f32,
    },
    Bypass {
        track: usize,
        module: usize,
        on: bool,
    },
    Play,
    Stop,
    /// Starts Recording, after the count-in when the transport was stopped.
    Record,
    /// Moves the playhead to a frame.
    Locate(u64),
    Loop(LoopRegion),
    Tempo {
        bpm: f64,
        meter: (u32, u32),
    },
    Click(bool),
    /// Bars of click before Recording starts.
    CountIn(u32),
}

/// An Item's active Take was not in the preloaded buffers given to [`Arrangement::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeNotLoaded(pub String);

impl fmt::Display for TakeNotLoaded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "take '{}' is not loaded", self.0)
    }
}

impl std::error::Error for TakeNotLoaded {}

/// The audio thread's Project: every Track through its own Rack, summed with the click to stereo, with transport, loop and count-in; while Recording each armed Track's dry input goes into its recording ring. Allocates and frees nothing after `new`.
pub struct Arrangement {
    tracks: Vec<TrackPlayer>,
    readers: Vec<TakeReader>,
    silence: Vec<f32>,
    click: Vec<f32>,
    sample_rate: f32,
    max_frames: usize,
    transport: Transport,
    playhead: u64,
    loop_region: LoopRegion,
    frames_per_beat: f64,
    beats_per_bar: u64,
    click_on: bool,
    count_in_bars: u32,
    count_in: CountIn,
    blip: Blip,
}

struct TrackPlayer {
    rack: Rack,
    items: Vec<PlacedTake>,
    volume_db: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    arm: bool,
    recorder: TakeRecorder,
    gain_l: Smoother,
    gain_r: Smoother,
    source: Vec<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
}

struct PlacedTake {
    position: u64,
    samples: Arc<[f32]>,
}

#[derive(Clone, Copy, Default)]
struct CountIn {
    total: u64,
    left: u64,
}

#[derive(Clone, Copy)]
struct Blip {
    frames: usize,
    left: usize,
    phase: f32,
    step: f32,
    click_step: f32,
    accent_step: f32,
    level: f32,
    decay: f32,
}

impl Arrangement {
    /// Builds the Arrangement of `project` at `sample_rate` for blocks of up to `max_frames`, from one Rack per Track (prepared for at least `max_frames`) and the preloaded Take buffers keyed by Take file.
    ///
    /// # Errors
    ///
    /// [`TakeNotLoaded`] when an Item's active Take is missing from `takes`.
    ///
    /// # Panics
    ///
    /// When `racks` does not hold one Rack per Track, or a Rack is prepared for fewer than `max_frames`.
    pub fn new<S: BuildHasher>(
        project: &Project,
        racks: Vec<Rack>,
        takes: &HashMap<String, Arc<[f32]>, S>,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Arrangement, TakeNotLoaded> {
        assert_eq!(racks.len(), project.tracks.len(), "one Rack per Track");
        assert!(
            racks.iter().all(|rack| rack.max_frames() >= max_frames),
            "a Rack prepared for fewer than {max_frames} frames"
        );
        let any_solo = project.tracks.iter().any(|track| track.solo);
        let ring_capacity = sample_rate.ceil() as usize * RING_SECONDS as usize;
        let mut readers = Vec::with_capacity(racks.len());
        let tracks = project
            .tracks
            .iter()
            .zip(racks)
            .map(|(track, rack)| {
                let (recorder, reader) = take_ring(ring_capacity);
                readers.push(reader);
                TrackPlayer::new(
                    track,
                    rack,
                    recorder,
                    takes,
                    sample_rate,
                    max_frames,
                    any_solo,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Arrangement {
            tracks,
            readers,
            silence: vec![0.0; max_frames],
            click: vec![0.0; max_frames],
            sample_rate,
            max_frames,
            transport: Transport::Stopped,
            playhead: 0,
            loop_region: project.loop_region,
            frames_per_beat: frames_per_beat(sample_rate, project.tempo_bpm),
            beats_per_bar: u64::from(project.meter.0.max(1)),
            click_on: false,
            count_in_bars: 0,
            count_in: CountIn::default(),
            blip: Blip::new(sample_rate),
        })
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    pub fn playhead(&self) -> u64 {
        self.playhead
    }

    /// True while Recording waits for its count-in; the playhead holds until it ends.
    pub fn is_counting_in(&self) -> bool {
        self.count_in.left > 0
    }

    /// The disk thread's ends of the recording rings, one per Track in Track order; empty after the first call.
    pub fn take_readers(&mut self) -> Vec<TakeReader> {
        std::mem::take(&mut self.readers)
    }

    /// The pushes into recording rings that dropped frames since the last call.
    pub fn take_disk_overflows(&mut self) -> u64 {
        self.tracks
            .iter_mut()
            .map(|track| track.recorder.take_overflows())
            .sum()
    }

    /// Takes over the transport, playhead, count-in, click and open recording passes of the Arrangement this one replaces, so a rebuild neither stops playback nor splits a Take; `previous` records nothing more.
    pub fn continue_from(&mut self, previous: &mut Arrangement) {
        for (track, old) in self.tracks.iter_mut().zip(previous.tracks.iter_mut()) {
            track.recorder.continue_from(&mut old.recorder);
        }
        self.transport = previous.transport;
        self.playhead = previous.playhead;
        self.click_on = previous.click_on;
        self.count_in_bars = previous.count_in_bars;
        self.count_in = previous.count_in;
        self.blip = previous.blip;
    }

    /// Applies `command`; a Track index out of range is ignored.
    pub fn apply(&mut self, command: ArrangementCommand) {
        match command {
            ArrangementCommand::Volume { track, db } => {
                self.with_track(track, |t| {
                    t.volume_db = db.clamp(MIN_VOLUME_DB, MAX_VOLUME_DB);
                });
            }
            ArrangementCommand::Pan { track, pan } => {
                self.with_track(track, |t| t.pan = pan.clamp(-1.0, 1.0));
            }
            ArrangementCommand::Mute { track, on } => self.with_track(track, |t| t.mute = on),
            ArrangementCommand::Solo { track, on } => self.with_track(track, |t| t.solo = on),
            ArrangementCommand::Arm { track, on } => self.with_track(track, |t| {
                t.arm = on;
                if !on {
                    t.recorder.end_pass(false);
                }
            }),
            ArrangementCommand::Param {
                track,
                module,
                param,
                value,
            } => self.with_track(track, |t| {
                t.rack.module_mut(module).set_param(param, value);
            }),
            ArrangementCommand::Bypass { track, module, on } => {
                self.with_track(track, |t| t.rack.set_bypass(module, on));
            }
            ArrangementCommand::Play => {
                self.end_passes(false);
                self.transport = Transport::Playing;
                self.count_in = CountIn::default();
            }
            ArrangementCommand::Stop => {
                self.end_passes(false);
                self.transport = Transport::Stopped;
                self.count_in = CountIn::default();
            }
            ArrangementCommand::Record => self.record(),
            ArrangementCommand::Locate(frame) => {
                self.end_passes(false);
                self.playhead = frame;
            }
            ArrangementCommand::Loop(region) => self.loop_region = region,
            ArrangementCommand::Tempo { bpm, meter } => {
                self.frames_per_beat = frames_per_beat(self.sample_rate, bpm);
                self.beats_per_bar = u64::from(meter.0.max(1));
            }
            ArrangementCommand::Click(on) => self.click_on = on,
            ArrangementCommand::CountIn(bars) => self.count_in_bars = bars,
        }
    }

    fn with_track(&mut self, track: usize, change: impl FnOnce(&mut TrackPlayer)) {
        if let Some(track) = self.tracks.get_mut(track) {
            change(track);
        }
    }

    fn end_passes(&mut self, wrapped: bool) {
        for track in &mut self.tracks {
            track.recorder.end_pass(wrapped);
        }
    }

    fn record(&mut self) {
        if self.transport == Transport::Stopped {
            let bars = u64::from(self.count_in_bars);
            let total = beat_frame(bars * self.beats_per_bar, self.frames_per_beat);
            self.count_in = CountIn { total, left: total };
        }
        self.transport = Transport::Recording;
    }

    /// Runs one block: `inputs` holds one input per Track (a missing one is silence), each at least `out_l.len()` frames, which must not exceed `max_frames`.
    pub fn process(&mut self, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = out_l.len();
        debug_assert!(
            frames <= self.max_frames,
            "{frames} frames exceed max_frames {}",
            self.max_frames
        );
        for track in &mut self.tracks {
            track.source[..frames].fill(0.0);
        }
        self.advance(inputs, frames);
        out_l.fill(0.0);
        out_r.fill(0.0);
        let any_solo = self.tracks.iter().any(|track| track.solo);
        let silence = &self.silence[..frames];
        for (idx, track) in self.tracks.iter_mut().enumerate() {
            let source = if track.arm {
                inputs
                    .get(idx)
                    .and_then(|input| input.get(..frames))
                    .unwrap_or(silence)
            } else {
                &track.source[..frames]
            };
            track.rack.process(
                source,
                &mut track.left[..frames],
                &mut track.right[..frames],
            );
            track.mix_into(any_solo, out_l, out_r);
        }
        for ((left, right), &click) in out_l.iter_mut().zip(out_r.iter_mut()).zip(&self.click) {
            *left += click;
            *right += click;
        }
    }

    /// Moves the transport through `frames`, splitting at the count-in end and at each loop wrap, and writes the unarmed Tracks' Items, the armed Tracks' recording and the click for each stretch.
    fn advance(&mut self, inputs: &[&[f32]], frames: usize) {
        let mut done = 0;
        while done < frames && self.transport != Transport::Stopped {
            let rest = frames - done;
            if self.count_in.left > 0 {
                let run = rest.min(usize::try_from(self.count_in.left).unwrap_or(usize::MAX));
                let grid = self.count_in.total - self.count_in.left;
                self.click_stretch(Some(grid), done, run);
                self.count_in.left -= run as u64;
                done += run;
                continue;
            }
            let LoopRegion { start, end, on } = self.loop_region;
            let wraps = on && start < end && self.playhead < end;
            let run = if wraps {
                rest.min(usize::try_from(end - self.playhead).unwrap_or(usize::MAX))
            } else {
                rest
            };
            let recording = self.transport == Transport::Recording;
            for (idx, track) in self.tracks.iter_mut().enumerate() {
                if !track.arm {
                    track.write_items(self.playhead, done, run);
                } else if recording {
                    let input = inputs
                        .get(idx)
                        .and_then(|input| input.get(done..done + run))
                        .unwrap_or(&self.silence[..run]);
                    track.recorder.push(self.playhead, input);
                }
            }
            let grid = self.click_on.then_some(self.playhead);
            self.click_stretch(grid, done, run);
            self.playhead += run as u64;
            if wraps && self.playhead == end {
                self.playhead = start;
                if recording {
                    self.end_passes(true);
                }
            }
            done += run;
        }
        self.click_stretch(None, done, frames - done);
    }

    /// Writes the click into `click[offset..offset + run]`, starting a blip on each beat when `grid` gives the stretch's first frame on the beat grid, and carrying on a blip already sounding.
    fn click_stretch(&mut self, grid: Option<u64>, offset: usize, run: usize) {
        let frames_per_beat = self.frames_per_beat;
        let mut next_beat = grid.map(|first| self.first_beat_from(first));
        for (idx, sample) in self.click[offset..offset + run].iter_mut().enumerate() {
            if let (Some(first), Some(beat)) = (grid, next_beat)
                && beat_frame(beat, frames_per_beat) == first + idx as u64
            {
                self.blip.start(beat % self.beats_per_bar == 0);
                next_beat = Some(beat + 1);
            }
            *sample = self.blip.next_sample();
        }
    }

    fn first_beat_from(&self, frame: u64) -> u64 {
        let mut beat = (frame as f64 / self.frames_per_beat).floor() as u64;
        while beat_frame(beat, self.frames_per_beat) < frame {
            beat += 1;
        }
        beat
    }
}

fn beat_frame(beat: u64, frames_per_beat: f64) -> u64 {
    (beat as f64 * frames_per_beat).round() as u64
}

fn frames_per_beat(sample_rate: f32, bpm: f64) -> f64 {
    f64::from(sample_rate) * 60.0 / bpm
}

impl TrackPlayer {
    fn new<S: BuildHasher>(
        track: &Track,
        rack: Rack,
        recorder: TakeRecorder,
        takes: &HashMap<String, Arc<[f32]>, S>,
        sample_rate: f32,
        max_frames: usize,
        any_solo: bool,
    ) -> Result<TrackPlayer, TakeNotLoaded> {
        let items = track
            .items
            .iter()
            .map(|item| {
                let file = &item.takes[item.take].file;
                let samples = takes.get(file).ok_or_else(|| TakeNotLoaded(file.clone()))?;
                Ok(PlacedTake {
                    position: item.position,
                    samples: Arc::clone(samples),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut player = TrackPlayer {
            rack,
            items,
            volume_db: track.volume_db.clamp(MIN_VOLUME_DB, MAX_VOLUME_DB),
            pan: track.pan.clamp(-1.0, 1.0),
            mute: track.mute,
            solo: track.solo,
            arm: track.arm,
            recorder,
            gain_l: Smoother::new(0.0),
            gain_r: Smoother::new(0.0),
            source: vec![0.0; max_frames],
            left: vec![0.0; max_frames],
            right: vec![0.0; max_frames],
        };
        let (left, right) = player.gains(any_solo);
        player.gain_l = Smoother::new(left);
        player.gain_r = Smoother::new(right);
        player.gain_l.prepare(sample_rate);
        player.gain_r.prepare(sample_rate);
        Ok(player)
    }

    /// Left and right gain from volume and constant-power pan, unity at the centre; zero when muted or when another Track is soloed.
    fn gains(&self, any_solo: bool) -> (f32, f32) {
        if self.mute || (any_solo && !self.solo) {
            return (0.0, 0.0);
        }
        let volume = 10f32.powf(self.volume_db / 20.0) * SQRT_2;
        let angle = (self.pan + 1.0) * FRAC_PI_4;
        (volume * angle.cos(), volume * angle.sin())
    }

    fn mix_into(&mut self, any_solo: bool, out_l: &mut [f32], out_r: &mut [f32]) {
        let (left, right) = self.gains(any_solo);
        self.gain_l.set_target(left);
        self.gain_r.set_target(right);
        for (((out_l, out_r), &left), &right) in out_l
            .iter_mut()
            .zip(out_r.iter_mut())
            .zip(&self.left)
            .zip(&self.right)
        {
            *out_l += left * self.gain_l.next_value();
            *out_r += right * self.gain_r.next_value();
        }
    }

    /// Adds the Items sounding over the timeline frames `playhead..playhead + run` into `source[offset..offset + run]`.
    fn write_items(&mut self, playhead: u64, offset: usize, run: usize) {
        let stretch_end = playhead + run as u64;
        for item in &self.items {
            let item_end = item.position + item.samples.len() as u64;
            let from = item.position.max(playhead);
            let to = item_end.min(stretch_end);
            if from >= to {
                continue;
            }
            let take =
                &item.samples[(from - item.position) as usize..(to - item.position) as usize];
            let at = offset + (from - playhead) as usize;
            for (out, &sample) in self.source[at..at + take.len()].iter_mut().zip(take) {
                *out += sample;
            }
        }
    }
}

impl Blip {
    fn new(sample_rate: f32) -> Blip {
        let frames = (CLICK_SECONDS * sample_rate).round().max(1.0) as usize;
        Blip {
            frames,
            left: 0,
            phase: 0.0,
            step: 0.0,
            click_step: TAU * CLICK_HZ / sample_rate,
            accent_step: TAU * ACCENT_HZ / sample_rate,
            level: 0.0,
            decay: CLICK_END_LEVEL.powf(1.0 / frames as f32),
        }
    }

    fn start(&mut self, accent: bool) {
        self.left = self.frames;
        self.phase = 0.0;
        self.step = if accent {
            self.accent_step
        } else {
            self.click_step
        };
        self.level = CLICK_LEVEL;
    }

    fn next_sample(&mut self) -> f32 {
        if self.left == 0 {
            return 0.0;
        }
        let sample = self.level * self.phase.sin();
        self.phase += self.step;
        self.level *= self.decay;
        self.left -= 1;
        sample
    }
}
