use std::f32::consts::TAU;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use crate::project::TEMPO_BPM;

pub const VOLUME_DB: RangeInclusive<f64> = -40.0..=0.0;
pub(crate) const BAD_BPM: &str = "bpm must be between 20 and 300";
pub(crate) const BAD_VOLUME: &str = "volume must be between -40 and 0 dB";
pub const DEFAULT_VOLUME_DB: f64 = -12.0;
pub const DEFAULT_BPM: f64 = 120.0;
pub const DEFAULT_METER: (u32, u32) = (4, 4);
const BLIP_SECONDS: f32 = 0.005;
const BEAT_HZ: f32 = 1000.0;
const ACCENT_HZ: f32 = 1500.0;
/// A beat's start level at 0 dB volume.
const BEAT_LEVEL: f32 = 0.5;
/// +6 dB for beat 1 of a bar.
const ACCENT_GAIN: f32 = 2.0;
/// The blip decays to this share of its start level over its length.
const BLIP_END_LEVEL: f32 = 0.01;
const TAP_COUNT: usize = 4;
const TAP_RESET: Duration = Duration::from_secs(2);

/// What the audio thread plays with no Project open; `on` going from false to true starts at beat 1 of a bar, a tempo or meter change keeps the beat count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetronomeSettings {
    pub on: bool,
    pub bpm: f64,
    pub meter: (u32, u32),
    pub volume_db: f32,
    pub sample_rate: f32,
}

/// The control thread's side of the Metronome; its tempo, meter and volume live in `state.toml` ([`crate::SavedMetronome`]).
#[derive(Debug, Default)]
pub struct Metronome {
    /// Whether it plays with no Project open; with one open the Project's click stands in for it.
    pub(crate) on: bool,
    pub(crate) taps: TapTempo,
}

/// The tempo of a series of taps: the average interval of the last four, from the second tap on; a gap over two seconds starts a new series.
#[derive(Debug, Default)]
pub struct TapTempo {
    taps: Vec<Instant>,
}

impl TapTempo {
    /// Records a tap at `now`; the tempo in bpm, clamped to 20..=300 and rounded to 0.1, once the series has two taps.
    pub fn tap(&mut self, now: Instant) -> Option<f64> {
        if self
            .taps
            .last()
            .is_some_and(|&last| now.saturating_duration_since(last) > TAP_RESET)
        {
            self.taps.clear();
        }
        if self.taps.len() == TAP_COUNT {
            self.taps.remove(0);
        }
        self.taps.push(now);
        let (&first, &last) = (self.taps.first()?, self.taps.last()?);
        let intervals = self.taps.len() - 1;
        if intervals == 0 {
            return None;
        }
        let interval = last.saturating_duration_since(first).as_secs_f64() / intervals as f64;
        let bpm = (60.0 / interval).clamp(*TEMPO_BPM.start(), *TEMPO_BPM.end());
        Some((bpm * 10.0).round() / 10.0)
    }
}

/// One Metronome beat: a short decaying sine, beat 1 of a bar higher and 6 dB louder.
#[derive(Clone, Copy)]
pub(crate) struct Blip {
    frames: usize,
    left: usize,
    phase: f32,
    step: f32,
    beat_step: f32,
    accent_step: f32,
    gain: f32,
    level: f32,
    decay: f32,
}

impl Blip {
    pub(crate) fn new(sample_rate: f32, volume_db: f32) -> Blip {
        let frames = (BLIP_SECONDS * sample_rate).round().max(1.0) as usize;
        Blip {
            frames,
            left: 0,
            phase: 0.0,
            step: 0.0,
            beat_step: TAU * BEAT_HZ / sample_rate,
            accent_step: TAU * ACCENT_HZ / sample_rate,
            gain: volume_gain(volume_db),
            level: 0.0,
            decay: BLIP_END_LEVEL.powf(1.0 / frames as f32),
        }
    }

    /// Applies from the next beat; a non-finite volume is ignored.
    pub(crate) fn set_volume(&mut self, volume_db: f32) {
        if volume_db.is_finite() {
            self.gain = volume_gain(volume_db);
        }
    }

    /// Takes over `previous`'s volume, and its sounding beat when both run at one sample rate; a beat cut by a rate change is dropped.
    pub(crate) fn continue_from(&mut self, previous: &Blip) {
        if self.beat_step.to_bits() == previous.beat_step.to_bits() {
            *self = *previous;
        } else {
            self.gain = previous.gain;
        }
    }

    pub(crate) fn start(&mut self, accent: bool) {
        self.left = self.frames;
        self.phase = 0.0;
        (self.step, self.level) = if accent {
            (self.accent_step, BEAT_LEVEL * ACCENT_GAIN * self.gain)
        } else {
            (self.beat_step, BEAT_LEVEL * self.gain)
        };
    }

    pub(crate) fn next_sample(&mut self) -> f32 {
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

fn volume_gain(volume_db: f32) -> f32 {
    let db = volume_db.clamp(*VOLUME_DB.start() as f32, *VOLUME_DB.end() as f32);
    10f32.powf(db / 20.0)
}

/// The no-Project Metronome on the audio thread: beat `n` starts on frame `round((n - anchor) * frames_per_beat)` counted from the last tempo change, so beats do not drift. Allocates nothing.
pub(crate) struct PracticeMetronome {
    settings: MetronomeSettings,
    blip: Blip,
    frames_per_beat: f64,
    /// The beat position, in beats, where `elapsed` started counting.
    anchor: f64,
    elapsed: u64,
    next_beat: u64,
    next_onset: u64,
    /// The 1-based beat in the bar last started; 0 while off.
    beat: u32,
    beats_played: u64,
}

impl PracticeMetronome {
    pub(crate) fn new() -> PracticeMetronome {
        let settings = MetronomeSettings {
            on: false,
            bpm: DEFAULT_BPM,
            meter: DEFAULT_METER,
            volume_db: DEFAULT_VOLUME_DB as f32,
            sample_rate: 48_000.0,
        };
        PracticeMetronome {
            settings,
            blip: Blip::new(settings.sample_rate, settings.volume_db),
            frames_per_beat: frames_per_beat(settings),
            anchor: 0.0,
            elapsed: 0,
            next_beat: 0,
            next_onset: 0,
            beat: 0,
            beats_played: 0,
        }
    }

    /// A tempo outside 20..=300 bpm, a meter of 0 beats or a sample rate that is not positive is ignored.
    pub(crate) fn apply(&mut self, new: MetronomeSettings) {
        let valid = TEMPO_BPM.contains(&new.bpm) && new.meter.0 > 0 && new.sample_rate > 0.0;
        if !valid {
            return;
        }
        let old = std::mem::replace(&mut self.settings, new);
        if !new.on {
            self.beat = 0;
        } else if !old.on || old.sample_rate.to_bits() != new.sample_rate.to_bits() {
            self.blip = Blip::new(new.sample_rate, new.volume_db);
            self.frames_per_beat = frames_per_beat(new);
            self.anchor = 0.0;
            self.elapsed = 0;
            self.next_beat = 0;
            self.next_onset = 0;
        } else {
            self.anchor += self.elapsed as f64 / self.frames_per_beat;
            self.elapsed = 0;
            self.frames_per_beat = frames_per_beat(new);
            self.next_onset = self.onset(self.next_beat);
            self.blip.set_volume(new.volume_db);
        }
    }

    /// Switches it off, as an Arrangement starting does.
    pub(crate) fn switch_off(&mut self) {
        self.settings.on = false;
        self.beat = 0;
    }

    /// Adds the beats of the next `out_l.len()` frames to both outputs.
    pub(crate) fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        if !self.settings.on {
            return;
        }
        let beats_per_bar = u64::from(self.settings.meter.0);
        for (left, right) in out_l.iter_mut().zip(out_r.iter_mut()) {
            if self.elapsed >= self.next_onset {
                let in_bar = self.next_beat % beats_per_bar;
                self.blip.start(in_bar == 0);
                self.beat = in_bar as u32 + 1;
                self.beats_played = self.beats_played.wrapping_add(1);
                self.next_beat += 1;
                self.next_onset = self.onset(self.next_beat);
            }
            let sample = self.blip.next_sample();
            *left += sample;
            *right += sample;
            self.elapsed += 1;
        }
    }

    pub(crate) fn beat(&self) -> u32 {
        self.beat
    }

    /// The beats started so far, wrapping.
    pub(crate) fn beats_played(&self) -> u64 {
        self.beats_played
    }

    fn onset(&self, beat: u64) -> u64 {
        ((beat as f64 - self.anchor) * self.frames_per_beat)
            .round()
            .max(0.0) as u64
    }
}

fn frames_per_beat(settings: MetronomeSettings) -> f64 {
    f64::from(settings.sample_rate) * 60.0 / settings.bpm
}
