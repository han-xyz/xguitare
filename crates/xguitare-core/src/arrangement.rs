use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_4, SQRT_2};
use std::fmt;
use std::hash::BuildHasher;
use std::sync::Arc;

use xguitare_dsp::smooth::Smoother;
use xguitare_dsp::{Rack, StereoRack};

use crate::engine::{MeterPoint, MixMeters, set_module_bypass, set_module_param};
use crate::metronome::{Blip, DEFAULT_VOLUME_DB};
use crate::project::{
    Bus, LoopRegion, MAX_BUSES, MAX_TRACKS, METER_BEATS, METER_NOTES, Master, Project, TEMPO_BPM,
    Track,
};
use crate::take_capture::{RING_SECONDS, TakeReader, TakeRecorder, take_ring};

const MIN_VOLUME_DB: f32 = -60.0;
const MAX_VOLUME_DB: f32 = 6.0;
const MAX_COUNT_IN_BARS: u32 = 2;
/// A Send ramping to off below this gain (-120 dB) stops, so an off Send costs nothing.
const SEND_SILENT: f32 = 1e-6;

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
    /// The Metronome volume, −40..=0 dB, from the next beat of the click.
    ClickVolume(f32),
    /// Bars of click before Recording starts.
    CountIn(u32),
    /// Whether the Track's own output goes to Master.
    ToMaster {
        track: usize,
        on: bool,
    },
    /// `-inf` turns the Send off.
    SendLevel {
        track: usize,
        bus: usize,
        db: f32,
    },
    SendPre {
        track: usize,
        bus: usize,
        on: bool,
    },
    BusVolume {
        bus: usize,
        db: f32,
    },
    BusPan {
        bus: usize,
        pan: f32,
    },
    BusMute {
        bus: usize,
        on: bool,
    },
    BusSolo {
        bus: usize,
        on: bool,
    },
    BusParam {
        bus: usize,
        module: usize,
        param: usize,
        value: f32,
    },
    BusBypass {
        bus: usize,
        module: usize,
        on: bool,
    },
    /// `-inf` silences Master; the click still sounds.
    MasterVolume {
        db: f32,
    },
    MasterParam {
        module: usize,
        param: usize,
        value: f32,
    },
    MasterBypass {
        module: usize,
        on: bool,
    },
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

/// The audio thread's Project: every Track through its own Rack and fader to Master and through its Sends to the Buses, each Bus through its Rack and fader to Master, Master through its Rack and volume, then the click, with transport, loop and count-in; while Recording each armed Track's dry input goes into its Take ring. Allocates and frees nothing after `new`.
pub struct Arrangement {
    tracks: Vec<TrackPlayer>,
    buses: Vec<BusPlayer>,
    master: MasterPlayer,
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
    /// The 1-based beat in the bar of the last click.
    click_beat: u32,
    beats_played: u64,
    /// The first Track and Bus index whose clip flags another Track or Bus held before this Arrangement; `MAX_TRACKS` and `MAX_BUSES` when none moved.
    moved_from: (usize, usize),
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "mute, solo, arm and to_master are independent Track switches"
)]
struct TrackPlayer {
    rack: Rack,
    items: Vec<PlacedTake>,
    volume_db: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    arm: bool,
    to_master: bool,
    /// Whether solo in place and mute let this Track sound.
    audible: bool,
    recorder: TakeRecorder,
    gain_l: Smoother,
    gain_r: Smoother,
    pan_l: Smoother,
    pan_r: Smoother,
    direct: Smoother,
    /// One per Bus, in Bus order.
    sends: Vec<SendPlayer>,
    peak: [f32; 2],
    source: Vec<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
    post_l: Vec<f32>,
    post_r: Vec<f32>,
    pre_l: Vec<f32>,
    pre_r: Vec<f32>,
}

struct SendPlayer {
    level_db: f32,
    pre: bool,
    gain: Smoother,
    /// The gain at the end of the last block; 0 once an off Send has ramped down.
    last: f32,
}

struct BusPlayer {
    rack: StereoRack,
    volume_db: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    audible: bool,
    gain_l: Smoother,
    gain_r: Smoother,
    peak: [f32; 2],
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

struct MasterPlayer {
    rack: StereoRack,
    volume_db: f32,
    gain: Smoother,
    peak: [f32; 2],
    in_l: Vec<f32>,
    in_r: Vec<f32>,
}

struct PlacedTake {
    position: u64,
    /// The first frame of `samples` that plays, at `position`.
    offset: usize,
    samples: Arc<[f32]>,
}

#[derive(Clone, Copy, Default)]
struct CountIn {
    total: u64,
    left: u64,
}

impl Arrangement {
    /// Builds the Arrangement of `project` at `sample_rate` for blocks of up to `max_frames`, from one Rack per Track, one stereo Rack per Bus and Master's stereo Rack (each prepared for at least `max_frames`) and the preloaded Take buffers keyed by Take file.
    ///
    /// # Errors
    ///
    /// [`TakeNotLoaded`] when an Item's active Take is missing from `takes`.
    ///
    /// # Panics
    ///
    /// When `racks` does not hold one Rack per Track or `bus_racks` one per Bus, or a Rack is prepared for fewer than `max_frames`.
    pub fn new<S: BuildHasher>(
        project: &Project,
        racks: Vec<Rack>,
        bus_racks: Vec<StereoRack>,
        master_rack: StereoRack,
        takes: &HashMap<String, Arc<[f32]>, S>,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Arrangement, TakeNotLoaded> {
        assert_eq!(racks.len(), project.tracks.len(), "one Rack per Track");
        assert_eq!(bus_racks.len(), project.buses.len(), "one Rack per Bus");
        assert!(
            racks.iter().all(|rack| rack.max_frames() >= max_frames),
            "a Rack prepared for fewer than {max_frames} frames"
        );
        assert!(
            bus_racks
                .iter()
                .chain([&master_rack])
                .all(|rack| rack.max_frames() >= max_frames),
            "a Bus or Master Rack prepared for fewer than {max_frames} frames"
        );
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
                    &project.buses,
                    rack,
                    recorder,
                    takes,
                    sample_rate,
                    max_frames,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let buses = project
            .buses
            .iter()
            .zip(bus_racks)
            .map(|(bus, rack)| BusPlayer::new(bus, rack, sample_rate, max_frames))
            .collect();
        let mut arrangement = Arrangement {
            tracks,
            buses,
            master: MasterPlayer::new(&project.master, master_rack, sample_rate, max_frames),
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
            blip: Blip::new(sample_rate, DEFAULT_VOLUME_DB as f32),
            click_beat: 0,
            beats_played: 0,
            moved_from: (MAX_TRACKS, MAX_BUSES),
        };
        arrangement.update_audible();
        for track in &mut arrangement.tracks {
            track.set_targets();
            track.snap();
        }
        for bus in &mut arrangement.buses {
            bus.set_targets();
            bus.gain_l.snap();
            bus.gain_r.snap();
        }
        Ok(arrangement)
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

    /// The beats left in the count-in, rounded up; 0 when none is under way.
    pub fn count_in_remaining(&self) -> u32 {
        (self.count_in.left as f64 / self.frames_per_beat).ceil() as u32
    }

    /// The disk thread's ends of the Take rings, one per Track in Track order; empty after the first call.
    pub fn take_readers(&mut self) -> Vec<TakeReader> {
        std::mem::take(&mut self.readers)
    }

    /// The pushes into Take rings that dropped frames since the last call.
    pub fn take_disk_overflows(&mut self) -> u64 {
        self.tracks
            .iter_mut()
            .map(|track| track.recorder.take_overflows())
            .sum()
    }

    /// The 1-based beat in the bar of the last click while the click sounds, during a count-in or with the click on and the transport running; 0 otherwise.
    pub fn click_beat(&self) -> u32 {
        let sounding =
            self.is_counting_in() || (self.click_on && self.transport != Transport::Stopped);
        if sounding { self.click_beat } else { 0 }
    }

    /// The clicks started so far, count-in included, wrapping.
    pub fn beats_played(&self) -> u64 {
        self.beats_played
    }

    /// Takes over the transport, playhead, count-in, click and open Takes of the Arrangement this one replaces, so a rebuild neither stops playback nor splits a Take; `previous` records nothing more.
    pub fn continue_from(&mut self, previous: &mut Arrangement) {
        for (track, old) in self.tracks.iter_mut().zip(previous.tracks.iter_mut()) {
            track.recorder.continue_from(&mut old.recorder);
        }
        self.transport = previous.transport;
        self.playhead = previous.playhead;
        self.click_on = previous.click_on;
        self.count_in_bars = previous.count_in_bars;
        self.count_in = previous.count_in;
        self.blip.continue_from(&previous.blip);
        self.click_beat = previous.click_beat;
        self.beats_played = previous.beats_played;
    }

    /// Applies `command`; a Track, Module or Parameter index out of range, a non-finite volume or pan, a tempo or meter a Project file would reject, and a count-in above two bars are ignored.
    pub fn apply(&mut self, command: ArrangementCommand) {
        match command {
            ArrangementCommand::Volume { track, db } if db.is_finite() => {
                self.with_track(track, |t| {
                    t.volume_db = db.clamp(MIN_VOLUME_DB, MAX_VOLUME_DB);
                });
            }
            ArrangementCommand::Pan { track, pan } if pan.is_finite() => {
                self.with_track(track, |t| t.pan = pan.clamp(-1.0, 1.0));
            }
            ArrangementCommand::Mute { track, on } => self.with_track(track, |t| t.mute = on),
            ArrangementCommand::Solo { track, on } => self.with_track(track, |t| t.solo = on),
            ArrangementCommand::Arm { track, on } => self.with_track(track, |t| {
                t.arm = on;
                if !on {
                    t.recorder.end_take(false);
                }
            }),
            ArrangementCommand::Param {
                track,
                module,
                param,
                value,
            } => self.with_track(track, |t| {
                set_module_param(&mut t.rack, module, param, value);
            }),
            ArrangementCommand::Bypass { track, module, on } => {
                self.with_track(track, |t| set_module_bypass(&mut t.rack, module, on));
            }
            ArrangementCommand::Play => {
                self.end_takes(false);
                self.transport = Transport::Playing;
                self.count_in = CountIn::default();
            }
            ArrangementCommand::Stop => {
                self.end_takes(false);
                self.transport = Transport::Stopped;
                self.count_in = CountIn::default();
                self.click_beat = 0;
            }
            ArrangementCommand::Record => self.record(),
            ArrangementCommand::Locate(frame) => {
                self.end_takes(false);
                self.playhead = frame;
            }
            ArrangementCommand::Loop(region) if !region.on || region.start < region.end => {
                self.loop_region = region;
            }
            ArrangementCommand::Tempo { bpm, meter }
                if TEMPO_BPM.contains(&bpm)
                    && METER_BEATS.contains(&meter.0)
                    && METER_NOTES.contains(&meter.1) =>
            {
                self.change_tempo(bpm, meter.0);
            }
            ArrangementCommand::Click(on) => {
                self.click_on = on;
                if !on {
                    self.click_beat = 0;
                }
            }
            ArrangementCommand::ClickVolume(db) => self.blip.set_volume(db),
            ArrangementCommand::CountIn(bars) if bars <= MAX_COUNT_IN_BARS => {
                self.count_in_bars = bars;
            }
            ArrangementCommand::Volume { .. }
            | ArrangementCommand::Pan { .. }
            | ArrangementCommand::Loop(_)
            | ArrangementCommand::Tempo { .. }
            | ArrangementCommand::CountIn(_) => {}
            mixer => self.apply_mixer(mixer),
        }
    }

    /// Applies a Send, Bus or Master command, or `ToMaster`; an index out of range or a non-finite value but a `-inf` level is ignored.
    fn apply_mixer(&mut self, command: ArrangementCommand) {
        match command {
            ArrangementCommand::ToMaster { track, on } => {
                self.with_track(track, |t| t.to_master = on);
            }
            ArrangementCommand::SendLevel { track, bus, db } => {
                if let Some(db) = level_db(db) {
                    self.with_send(track, bus, |send| send.level_db = db);
                }
            }
            ArrangementCommand::SendPre { track, bus, on } => {
                self.with_send(track, bus, |send| send.pre = on);
            }
            ArrangementCommand::BusVolume { bus, db } if db.is_finite() => {
                self.with_bus(bus, |b| {
                    b.volume_db = db.clamp(MIN_VOLUME_DB, MAX_VOLUME_DB);
                });
            }
            ArrangementCommand::BusPan { bus, pan } if pan.is_finite() => {
                self.with_bus(bus, |b| b.pan = pan.clamp(-1.0, 1.0));
            }
            ArrangementCommand::BusMute { bus, on } => self.with_bus(bus, |b| b.mute = on),
            ArrangementCommand::BusSolo { bus, on } => self.with_bus(bus, |b| b.solo = on),
            ArrangementCommand::BusParam {
                bus,
                module,
                param,
                value,
            } => self.with_bus(bus, |b| b.rack.set_param(module, param, value)),
            ArrangementCommand::BusBypass { bus, module, on } => {
                self.with_bus(bus, |b| b.rack.set_bypass(module, on));
            }
            ArrangementCommand::MasterVolume { db } => {
                if let Some(db) = level_db(db) {
                    self.master.volume_db = db;
                }
            }
            ArrangementCommand::MasterParam {
                module,
                param,
                value,
            } => self.master.rack.set_param(module, param, value),
            ArrangementCommand::MasterBypass { module, on } => {
                self.master.rack.set_bypass(module, on);
            }
            _ => {}
        }
    }

    fn with_bus(&mut self, bus: usize, change: impl FnOnce(&mut BusPlayer)) {
        if let Some(bus) = self.buses.get_mut(bus) {
            change(bus);
        }
    }

    fn with_send(&mut self, track: usize, bus: usize, change: impl FnOnce(&mut SendPlayer)) {
        if let Some(send) = self
            .tracks
            .get_mut(track)
            .and_then(|track| track.sends.get_mut(bus))
        {
            change(send);
        }
    }

    /// Solo in place: with anything soloed, a Track sounds when soloed or sending to a soloed Bus, a Bus when soloed or sent to by a soloed Track; mute always silences.
    fn update_audible(&mut self) {
        let any_solo =
            self.tracks.iter().any(|track| track.solo) || self.buses.iter().any(|bus| bus.solo);
        for (idx, bus) in self.buses.iter_mut().enumerate() {
            let sent_to_by_solo = self
                .tracks
                .iter()
                .any(|track| track.solo && track.sends[idx].is_on());
            bus.audible = !bus.mute && (!any_solo || bus.solo || sent_to_by_solo);
        }
        for track in &mut self.tracks {
            let sends_to_solo = track
                .sends
                .iter()
                .zip(&self.buses)
                .any(|(send, bus)| bus.solo && send.is_on());
            track.audible = !track.mute && (!any_solo || track.solo || sends_to_solo);
        }
    }

    /// Marks the Tracks from `track` and the Buses from `bus` as moved or gone, so their clip flags are cleared once this Arrangement has replaced the running one.
    pub(crate) fn clear_clips_from(&mut self, track: usize, bus: usize) {
        self.moved_from = (self.moved_from.0.min(track), self.moved_from.1.min(bus));
    }

    /// Takes over the moved Tracks and Buses of `replaced`, an Arrangement that never ran.
    pub(crate) fn keep_moved_clips(&mut self, replaced: &Arrangement) {
        self.clear_clips_from(replaced.moved_from.0, replaced.moved_from.1);
    }

    /// Clears the clip flags of the moved Tracks and Buses; called once the Arrangement it replaced no longer raises them.
    pub(crate) fn clear_moved_clips(&self, meters: &MixMeters) {
        let (first_track, first_bus) = self.moved_from;
        let tracks = (first_track..MAX_TRACKS).map(MeterPoint::Track);
        let buses = (first_bus..MAX_BUSES).map(MeterPoint::Bus);
        for point in tracks.chain(buses) {
            meters.clip_reset(Some(point));
        }
    }

    pub(crate) fn raise_meters(&self, meters: &MixMeters) {
        for (idx, track) in self.tracks.iter().enumerate() {
            meters.raise(MeterPoint::Track(idx), track.peak);
        }
        for (idx, bus) in self.buses.iter().enumerate() {
            meters.raise(MeterPoint::Bus(idx), bus.peak);
        }
        meters.raise(MeterPoint::Master, self.master.peak);
    }

    fn with_track(&mut self, track: usize, change: impl FnOnce(&mut TrackPlayer)) {
        if let Some(track) = self.tracks.get_mut(track) {
            change(track);
        }
    }

    fn end_takes(&mut self, wrapped: bool) {
        for track in &mut self.tracks {
            track.recorder.end_take(wrapped);
        }
    }

    /// A count-in under way keeps its length in bars at the new tempo.
    fn change_tempo(&mut self, bpm: f64, beats_per_bar: u32) {
        let new = frames_per_beat(self.sample_rate, bpm);
        let scale = new / self.frames_per_beat;
        let rescale = |frames: u64| (frames as f64 * scale).round() as u64;
        self.count_in = CountIn {
            total: rescale(self.count_in.total),
            left: rescale(self.count_in.left),
        };
        self.frames_per_beat = new;
        self.beats_per_bar = u64::from(beats_per_bar);
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
        self.update_audible();
        for bus in &mut self.buses {
            bus.in_l[..frames].fill(0.0);
            bus.in_r[..frames].fill(0.0);
        }
        let master_l = &mut self.master.in_l[..frames];
        let master_r = &mut self.master.in_r[..frames];
        master_l.fill(0.0);
        master_r.fill(0.0);
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
            track.mix_into(&mut self.buses, master_l, master_r);
        }
        for bus in &mut self.buses {
            bus.mix_into(master_l, master_r);
        }
        self.master.process(out_l, out_r);
        for ((left, right), &click) in out_l.iter_mut().zip(out_r.iter_mut()).zip(&self.click) {
            *left += click;
            *right += click;
        }
    }

    /// Moves the transport through `frames`, splitting at the count-in end and at each loop wrap, and writes the unarmed Tracks' Items, the armed Tracks' dry input and the click for each stretch.
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
                    self.end_takes(true);
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
                let in_bar = beat % self.beats_per_bar;
                self.blip.start(in_bar == 0);
                self.click_beat = in_bar as u32 + 1;
                self.beats_played = self.beats_played.wrapping_add(1);
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
        buses: &[Bus],
        rack: Rack,
        recorder: TakeRecorder,
        takes: &HashMap<String, Arc<[f32]>, S>,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<TrackPlayer, TakeNotLoaded> {
        let items = track
            .items
            .iter()
            .map(|item| {
                let file = &item.takes[item.take].file;
                let samples = takes.get(file).ok_or_else(|| TakeNotLoaded(file.clone()))?;
                Ok(PlacedTake {
                    position: item.position,
                    offset: usize::try_from(item.offset)
                        .unwrap_or(usize::MAX)
                        .min(samples.len()),
                    samples: Arc::clone(samples),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let sends = buses
            .iter()
            .map(|bus| {
                let send = track.sends.iter().find(|send| send.bus == bus.id);
                SendPlayer {
                    level_db: send.map_or(f32::NEG_INFINITY, |send| send.level_db),
                    pre: send.is_some_and(|send| send.pre),
                    gain: prepared(0.0, sample_rate),
                    last: 0.0,
                }
            })
            .collect();
        Ok(TrackPlayer {
            rack,
            items,
            volume_db: track.volume_db.clamp(MIN_VOLUME_DB, MAX_VOLUME_DB),
            pan: track.pan.clamp(-1.0, 1.0),
            mute: track.mute,
            solo: track.solo,
            arm: track.arm,
            to_master: track.to_master,
            audible: true,
            recorder,
            gain_l: prepared(0.0, sample_rate),
            gain_r: prepared(0.0, sample_rate),
            pan_l: prepared(0.0, sample_rate),
            pan_r: prepared(0.0, sample_rate),
            direct: prepared(0.0, sample_rate),
            sends,
            peak: [0.0; 2],
            source: vec![0.0; max_frames],
            left: vec![0.0; max_frames],
            right: vec![0.0; max_frames],
            post_l: vec![0.0; max_frames],
            post_r: vec![0.0; max_frames],
            pre_l: vec![0.0; max_frames],
            pre_r: vec![0.0; max_frames],
        })
    }

    /// Fader gains from volume and constant-power pan, unity at the centre; the pan alone for pre-fader Sends; all zero when the Track is not audible.
    fn set_targets(&mut self) {
        let (pan_l, pan_r) = if self.audible {
            pan_gains(self.pan)
        } else {
            (0.0, 0.0)
        };
        let volume = db_gain(self.volume_db);
        self.gain_l.set_target(volume * pan_l);
        self.gain_r.set_target(volume * pan_r);
        self.pan_l.set_target(pan_l);
        self.pan_r.set_target(pan_r);
        self.direct
            .set_target(if self.to_master { 1.0 } else { 0.0 });
        for send in &mut self.sends {
            send.gain.set_target(db_gain(send.level_db));
        }
    }

    fn snap(&mut self) {
        for smoother in [
            &mut self.gain_l,
            &mut self.gain_r,
            &mut self.pan_l,
            &mut self.pan_r,
            &mut self.direct,
        ] {
            smoother.snap();
        }
        for send in &mut self.sends {
            send.gain.snap();
            send.last = db_gain(send.level_db);
        }
    }

    /// Adds the Track's post-fader sound into `out_l`/`out_r` when it goes to Master, and its Sends into the Buses' inputs.
    fn mix_into(&mut self, buses: &mut [BusPlayer], out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = out_l.len();
        self.set_targets();
        let mut peak = [0.0f32; 2];
        for idx in 0..frames {
            let (left, right) = (self.left[idx], self.right[idx]);
            let post_l = left * self.gain_l.next_value();
            let post_r = right * self.gain_r.next_value();
            self.post_l[idx] = post_l;
            self.post_r[idx] = post_r;
            self.pre_l[idx] = left * self.pan_l.next_value();
            self.pre_r[idx] = right * self.pan_r.next_value();
            let direct = self.direct.next_value();
            out_l[idx] += post_l * direct;
            out_r[idx] += post_r * direct;
            peak = [peak[0].max(post_l.abs()), peak[1].max(post_r.abs())];
        }
        self.peak = peak;
        for (send, bus) in self.sends.iter_mut().zip(buses) {
            let (from_l, from_r) = if send.pre {
                (&self.pre_l[..frames], &self.pre_r[..frames])
            } else {
                (&self.post_l[..frames], &self.post_r[..frames])
            };
            send.add_into(from_l, from_r, &mut bus.in_l, &mut bus.in_r);
        }
    }

    /// Adds the Items sounding over the timeline frames `playhead..playhead + run` into `source[offset..offset + run]`.
    fn write_items(&mut self, playhead: u64, offset: usize, run: usize) {
        let stretch_end = playhead + run as u64;
        for item in &self.items {
            let audible = &item.samples[item.offset..];
            let item_end = item.position + audible.len() as u64;
            let from = item.position.max(playhead);
            let to = item_end.min(stretch_end);
            if from >= to {
                continue;
            }
            let take = &audible[(from - item.position) as usize..(to - item.position) as usize];
            let at = offset + (from - playhead) as usize;
            for (out, &sample) in self.source[at..at + take.len()].iter_mut().zip(take) {
                *out += sample;
            }
        }
    }
}

impl SendPlayer {
    fn is_on(&self) -> bool {
        self.level_db > f32::NEG_INFINITY
    }

    /// Adds `from` times the Send level into `to`; does nothing once an off Send has ramped down.
    #[allow(
        clippy::float_cmp,
        reason = "an off Send's gain is set to exactly 0.0 once it is below SEND_SILENT"
    )]
    fn add_into(&mut self, from_l: &[f32], from_r: &[f32], to_l: &mut [f32], to_r: &mut [f32]) {
        if !self.is_on() && self.last == 0.0 {
            return;
        }
        for (((&from_l, &from_r), to_l), to_r) in from_l.iter().zip(from_r).zip(to_l).zip(to_r) {
            let gain = self.gain.next_value();
            *to_l += from_l * gain;
            *to_r += from_r * gain;
            self.last = gain;
        }
        if !self.is_on() && self.last < SEND_SILENT {
            self.gain.snap();
            self.last = 0.0;
        }
    }
}

impl BusPlayer {
    fn new(bus: &Bus, rack: StereoRack, sample_rate: f32, max_frames: usize) -> BusPlayer {
        BusPlayer {
            rack,
            volume_db: bus.volume_db.clamp(MIN_VOLUME_DB, MAX_VOLUME_DB),
            pan: bus.pan.clamp(-1.0, 1.0),
            mute: bus.mute,
            solo: bus.solo,
            audible: true,
            gain_l: prepared(0.0, sample_rate),
            gain_r: prepared(0.0, sample_rate),
            peak: [0.0; 2],
            in_l: vec![0.0; max_frames],
            in_r: vec![0.0; max_frames],
            out_l: vec![0.0; max_frames],
            out_r: vec![0.0; max_frames],
        }
    }

    fn set_targets(&mut self) {
        let (left, right) = if self.audible {
            let volume = db_gain(self.volume_db);
            let (pan_l, pan_r) = pan_gains(self.pan);
            (volume * pan_l, volume * pan_r)
        } else {
            (0.0, 0.0)
        };
        self.gain_l.set_target(left);
        self.gain_r.set_target(right);
    }

    /// Runs the Bus's input through its Rack and fader into `out_l`/`out_r`.
    fn mix_into(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = out_l.len();
        self.rack.process(
            &self.in_l[..frames],
            &self.in_r[..frames],
            &mut self.out_l[..frames],
            &mut self.out_r[..frames],
        );
        self.set_targets();
        let mut peak = [0.0f32; 2];
        for (((out_l, out_r), &left), &right) in out_l
            .iter_mut()
            .zip(out_r.iter_mut())
            .zip(&self.out_l)
            .zip(&self.out_r)
        {
            let left = left * self.gain_l.next_value();
            let right = right * self.gain_r.next_value();
            *out_l += left;
            *out_r += right;
            peak = [peak[0].max(left.abs()), peak[1].max(right.abs())];
        }
        self.peak = peak;
    }
}

impl MasterPlayer {
    fn new(master: &Master, rack: StereoRack, sample_rate: f32, max_frames: usize) -> MasterPlayer {
        let volume_db = level_db(master.volume_db).unwrap_or(0.0);
        MasterPlayer {
            rack,
            volume_db,
            gain: prepared(db_gain(volume_db), sample_rate),
            peak: [0.0; 2],
            in_l: vec![0.0; max_frames],
            in_r: vec![0.0; max_frames],
        }
    }

    /// Runs the mix through Master's Rack and volume into `out_l`/`out_r`.
    fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = out_l.len();
        self.rack
            .process(&self.in_l[..frames], &self.in_r[..frames], out_l, out_r);
        self.gain.set_target(db_gain(self.volume_db));
        let mut peak = [0.0f32; 2];
        for (left, right) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let gain = self.gain.next_value();
            *left *= gain;
            *right *= gain;
            peak = [peak[0].max(left.abs()), peak[1].max(right.abs())];
        }
        self.peak = peak;
    }
}

fn prepared(value: f32, sample_rate: f32) -> Smoother {
    let mut smoother = Smoother::new(value);
    smoother.prepare(sample_rate);
    smoother
}

fn db_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Constant-power left and right gains, unity on both sides at the centre.
fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan + 1.0) * FRAC_PI_4;
    (SQRT_2 * angle.cos(), SQRT_2 * angle.sin())
}

/// A Send level or Master volume a command may set: `-inf`, or a finite dB clamped to the Track volume range.
fn level_db(db: f32) -> Option<f32> {
    if db == f32::NEG_INFINITY {
        Some(db)
    } else if db.is_finite() {
        Some(db.clamp(MIN_VOLUME_DB, MAX_VOLUME_DB))
    } else {
        None
    }
}
