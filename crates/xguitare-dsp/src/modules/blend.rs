use std::f32::consts::FRAC_1_SQRT_2;

use super::db_to_linear;
use crate::oversample::Oversampler4x;
use crate::smooth::Smoother;
use crate::svf::{Svf, SvfCoefficients};
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const BLEND: usize = 0;
const DRIVE: usize = 1;
const LEVEL: usize = 2;
const BASS: usize = 3;
const BASS_FREQ: usize = 4;
const MID: usize = 5;
const MID_FREQ: usize = 6;
const TREBLE: usize = 7;
const PRESENCE: usize = 8;
const CROSSOVER: usize = 9;
const PARAMS: usize = 10;
const OVERSAMPLING: f32 = 4.0;
const BUTTERWORTH_Q: f32 = FRAC_1_SQRT_2;
const SHELF_Q: f32 = FRAC_1_SQRT_2;
const MID_Q: f32 = 0.7;
const TREBLE_HZ: f32 = 3_000.0;
const PRESENCE_HZ: f32 = 2_000.0;
const PRESENCE_MAX_DB: f32 = 12.0;
const BASS_FREQS: [f32; 2] = [40.0, 80.0];
const MID_FREQS: [f32; 2] = [500.0, 1_000.0];

pub static SPEC: ModuleSpec = ModuleSpec {
    type_name: "blend",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "blend",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "drive",
            min: 0.0,
            max: 100.0,
            default: 40.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "level",
            min: -24.0,
            max: 12.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "bass",
            min: -12.0,
            max: 12.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "bass_freq",
            min: 0.0,
            max: 1.0,
            default: 1.0,
            unit: Unit::Linear,
            integer: true,
            labels: &["40 Hz", "80 Hz"],
        },
        ParamSpec {
            name: "mid",
            min: -12.0,
            max: 12.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "mid_freq",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: Unit::Linear,
            integer: true,
            labels: &["500 Hz", "1000 Hz"],
        },
        ParamSpec {
            name: "treble",
            min: -12.0,
            max: 12.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "presence",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "crossover",
            min: 20.0,
            max: 400.0,
            default: 20.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
    ],
};

/// A bass DI blend drive: below `crossover` the signal stays clean, above it is driven; `blend` mixes the clean input with that, and a tone EQ follows.
pub struct Blend {
    values: [f32; PARAMS],
    sample_rate: f32,
    oversampler: Oversampler4x,
    split_drive: SplitDrive,
    tone: ToneEq,
}

impl Blend {
    pub fn new() -> Blend {
        let values: [f32; PARAMS] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let sample_rate = 48_000.0;
        Blend {
            values,
            sample_rate,
            oversampler: Oversampler4x::new(),
            split_drive: SplitDrive::new(&values, OVERSAMPLING * sample_rate),
            tone: ToneEq::new(&values, sample_rate),
        }
    }
}

impl Default for Blend {
    fn default() -> Blend {
        Blend::new()
    }
}

impl Module for Blend {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.sample_rate = sample_rate;
        self.oversampler.prepare(max_frames);
        self.oversampler.reset();
        self.split_drive.prepare(OVERSAMPLING * sample_rate);
        self.tone.prepare(sample_rate);
    }

    fn reset(&mut self) {
        self.oversampler.reset();
        self.split_drive.reset();
        self.tone.reset();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        let value = if spec.integer { value.round() } else { value };
        self.values[idx] = value;
        match idx {
            BLEND => self.split_drive.mix.set_target(value / 100.0),
            DRIVE => self.split_drive.clip_gain.set_target(clip_gain(value)),
            LEVEL => self.split_drive.level.set_target(db_to_linear(value)),
            PRESENCE => self.split_drive.presence_db.set_target(presence_db(value)),
            CROSSOVER => self.split_drive.crossover_hz.set_target(value),
            BASS | MID | TREBLE => self.tone.gains_db[tone_band(idx)].set_target(value),
            BASS_FREQ => self.tone.choose_bass_freq(value),
            MID_FREQ => self.tone.choose_mid_freq(value),
            _ => unreachable!("blend has no parameter {idx}"),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let mono = &mut io.mono[..io.frames];
        for sample in self.oversampler.upsample(mono) {
            *sample = self.split_drive.process(*sample);
        }
        self.oversampler.downsample(mono);
        for sample in mono.iter_mut() {
            *sample = self.tone.process(*sample);
        }
    }
}

// drive 0..100 % maps exponentially onto a clipper gain of 1..100, as in drive.
fn clip_gain(drive: f32) -> f32 {
    10.0_f32.powf(drive / 100.0 * 2.0)
}

fn presence_db(presence: f32) -> f32 {
    presence / 100.0 * PRESENCE_MAX_DB
}

fn tone_band(idx: usize) -> usize {
    match idx {
        BASS => 0,
        MID => 1,
        _ => 2,
    }
}

fn choice(freqs: [f32; 2], value: f32) -> f32 {
    freqs[usize::from(value >= 0.5)]
}

/// Everything before the tone EQ, run at the oversampled rate so that the clean input, the low band and the driven high band stay time-aligned.
struct SplitDrive {
    rate: f32,
    mix: Smoother,
    clip_gain: Smoother,
    level: Smoother,
    presence_db: Smoother,
    crossover_hz: Smoother,
    crossover: f32,
    presence: f32,
    low_pass: SvfCoefficients,
    high_pass: SvfCoefficients,
    presence_shelf: SvfCoefficients,
    low_band: [Svf; 2],
    high_band: [Svf; 2],
    presence_filter: Svf,
}

impl SplitDrive {
    fn new(values: &[f32; PARAMS], rate: f32) -> SplitDrive {
        let crossover = values[CROSSOVER];
        let presence = presence_db(values[PRESENCE]);
        let mut split_drive = SplitDrive {
            rate,
            mix: Smoother::new(values[BLEND] / 100.0),
            clip_gain: Smoother::new(clip_gain(values[DRIVE])),
            level: Smoother::new(db_to_linear(values[LEVEL])),
            presence_db: Smoother::new(presence),
            crossover_hz: Smoother::new(crossover),
            crossover,
            presence,
            low_pass: SvfCoefficients::low_pass(crossover, BUTTERWORTH_Q, rate),
            high_pass: SvfCoefficients::high_pass(crossover, BUTTERWORTH_Q, rate),
            presence_shelf: SvfCoefficients::high_shelf(PRESENCE_HZ, SHELF_Q, presence, rate),
            low_band: Default::default(),
            high_band: Default::default(),
            presence_filter: Svf::new(),
        };
        split_drive.rebuild();
        split_drive
    }

    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        for smoother in [
            &mut self.mix,
            &mut self.clip_gain,
            &mut self.level,
            &mut self.presence_db,
            &mut self.crossover_hz,
        ] {
            smoother.prepare(rate);
        }
        self.rebuild();
        self.clear_filters();
    }

    fn reset(&mut self) {
        for smoother in [
            &mut self.mix,
            &mut self.clip_gain,
            &mut self.level,
            &mut self.presence_db,
            &mut self.crossover_hz,
        ] {
            smoother.snap();
        }
        self.crossover = self.crossover_hz.next_value_settling();
        self.presence = self.presence_db.next_value_settling();
        self.rebuild();
        self.clear_filters();
    }

    fn clear_filters(&mut self) {
        for filter in self.low_band.iter_mut().chain(&mut self.high_band) {
            filter.reset();
        }
        self.presence_filter.reset();
    }

    fn rebuild(&mut self) {
        self.low_pass = SvfCoefficients::low_pass(self.crossover, BUTTERWORTH_Q, self.rate);
        self.high_pass = SvfCoefficients::high_pass(self.crossover, BUTTERWORTH_Q, self.rate);
        self.presence_shelf =
            SvfCoefficients::high_shelf(PRESENCE_HZ, SHELF_Q, self.presence, self.rate);
    }

    fn process(&mut self, input: f32) -> f32 {
        let crossover = self.crossover_hz.next_value_settling();
        let presence = self.presence_db.next_value_settling();
        if crossover.to_bits() != self.crossover.to_bits()
            || presence.to_bits() != self.presence.to_bits()
        {
            self.crossover = crossover;
            self.presence = presence;
            self.rebuild();
        }
        let low = self
            .low_band
            .iter_mut()
            .fold(input, |value, filter| filter.process(&self.low_pass, value));
        let high = self.high_band.iter_mut().fold(input, |value, filter| {
            filter.process(&self.high_pass, value)
        });
        let lifted = self.presence_filter.process(&self.presence_shelf, high);
        let driven = (self.clip_gain.next_value() * lifted).tanh() * self.level.next_value();
        input + self.mix.next_value() * (low + driven - input)
    }
}

/// Bass shelf, mid bell and treble shelf after the blend.
struct ToneEq {
    rate: f32,
    gains_db: [Smoother; 3],
    smoothed_db: [f32; 3],
    bass_hz: f32,
    mid_hz: f32,
    coefficients: [SvfCoefficients; 3],
    filters: [Svf; 3],
}

impl ToneEq {
    fn new(values: &[f32; PARAMS], rate: f32) -> ToneEq {
        let smoothed_db = [values[BASS], values[MID], values[TREBLE]];
        let bass_hz = choice(BASS_FREQS, values[BASS_FREQ]);
        let mid_hz = choice(MID_FREQS, values[MID_FREQ]);
        ToneEq {
            rate,
            gains_db: smoothed_db.map(Smoother::new),
            smoothed_db,
            bass_hz,
            mid_hz,
            coefficients: tone_coefficients(smoothed_db, bass_hz, mid_hz, rate),
            filters: Default::default(),
        }
    }

    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        for smoother in &mut self.gains_db {
            smoother.prepare(rate);
        }
        self.rebuild();
        self.clear_filters();
    }

    fn reset(&mut self) {
        for (smoother, smoothed) in self.gains_db.iter_mut().zip(&mut self.smoothed_db) {
            smoother.snap();
            *smoothed = smoother.next_value_settling();
        }
        self.rebuild();
        self.clear_filters();
    }

    fn clear_filters(&mut self) {
        for filter in &mut self.filters {
            filter.reset();
        }
    }

    fn choose_bass_freq(&mut self, value: f32) {
        self.bass_hz = choice(BASS_FREQS, value);
        self.rebuild();
    }

    fn choose_mid_freq(&mut self, value: f32) {
        self.mid_hz = choice(MID_FREQS, value);
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.coefficients =
            tone_coefficients(self.smoothed_db, self.bass_hz, self.mid_hz, self.rate);
    }

    fn process(&mut self, input: f32) -> f32 {
        let mut moved = false;
        for (smoother, last) in self.gains_db.iter_mut().zip(&mut self.smoothed_db) {
            let value = smoother.next_value_settling();
            if value.to_bits() != last.to_bits() {
                *last = value;
                moved = true;
            }
        }
        if moved {
            self.rebuild();
        }
        self.filters
            .iter_mut()
            .zip(&self.coefficients)
            .fold(input, |value, (filter, coefficients)| {
                filter.process(coefficients, value)
            })
    }
}

fn tone_coefficients(
    [bass_db, mid_db, treble_db]: [f32; 3],
    bass_hz: f32,
    mid_hz: f32,
    rate: f32,
) -> [SvfCoefficients; 3] {
    [
        SvfCoefficients::low_shelf(bass_hz, SHELF_Q, bass_db, rate),
        SvfCoefficients::bell(mid_hz, MID_Q, mid_db, rate),
        SvfCoefficients::high_shelf(TREBLE_HZ, SHELF_Q, treble_db, rate),
    ]
}
