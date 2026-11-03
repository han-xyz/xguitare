use std::f32::consts::TAU;

use super::{db_to_linear, one_pole_coeff};
use crate::oversample::Oversampler4x;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const DRIVE: usize = 0;
const TONE: usize = 1;
const LEVEL: usize = 2;
const SMOOTHING_SECONDS: f32 = 0.005;
const HIGH_PASS_HZ: f32 = 720.0;
const OVERSAMPLING: f32 = 4.0;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "drive",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "drive",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "tone",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "level",
            min: -30.0,
            max: 10.0,
            default: 0.0,
            unit: Unit::Db,
        },
    ],
};

pub struct Drive {
    values: [f32; 3],
    sample_rate: f32,
    clip_gain: Smoother,
    tone_hz: Smoother,
    level: Smoother,
    high_pass_coeff: f32,
    high_pass_state: f32,
    low_pass_state: f32,
    oversampler: Oversampler4x,
}

impl Drive {
    pub fn new() -> Drive {
        let values = std::array::from_fn(|idx| SPEC.params[idx].default);
        Drive {
            values,
            sample_rate: 48_000.0,
            clip_gain: Smoother::new(clip_gain(values[DRIVE])),
            tone_hz: Smoother::new(tone_hz(values[TONE])),
            level: Smoother::new(db_to_linear(values[LEVEL])),
            high_pass_coeff: 0.0,
            high_pass_state: 0.0,
            low_pass_state: 0.0,
            oversampler: Oversampler4x::new(),
        }
    }
}

impl Default for Drive {
    fn default() -> Drive {
        Drive::new()
    }
}

impl Module for Drive {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.sample_rate = sample_rate;
        self.clip_gain.coeff = one_pole_coeff(SMOOTHING_SECONDS, OVERSAMPLING * sample_rate);
        self.tone_hz.coeff = one_pole_coeff(SMOOTHING_SECONDS, sample_rate);
        self.level.coeff = one_pole_coeff(SMOOTHING_SECONDS, sample_rate);
        self.high_pass_coeff = cutoff_coeff(HIGH_PASS_HZ, sample_rate);
        self.oversampler.prepare(max_frames);
    }

    fn reset(&mut self) {
        self.clip_gain.current = self.clip_gain.target;
        self.tone_hz.current = self.tone_hz.target;
        self.level.current = self.level.target;
        self.high_pass_state = 0.0;
        self.low_pass_state = 0.0;
        self.oversampler.reset();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        match idx {
            DRIVE => self.clip_gain.target = clip_gain(value),
            TONE => self.tone_hz.target = tone_hz(value),
            _ => self.level.target = db_to_linear(value),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let mono = &mut io.mono[..io.frames];
        for sample in mono.iter_mut() {
            self.high_pass_state =
                *sample + self.high_pass_coeff * (self.high_pass_state - *sample);
            *sample -= self.high_pass_state;
        }

        for sample in self.oversampler.upsample(mono) {
            *sample = (self.clip_gain.next() * *sample).tanh();
        }
        self.oversampler.downsample(mono);

        let low_pass_coeff = cutoff_coeff(self.tone_hz.advance(io.frames), self.sample_rate);
        for sample in mono.iter_mut() {
            self.low_pass_state = *sample + low_pass_coeff * (self.low_pass_state - *sample);
            *sample = self.low_pass_state * self.level.next();
        }
    }
}

// drive 0..100 % maps exponentially onto a clipper gain of 1..100.
fn clip_gain(drive: f32) -> f32 {
    10.0_f32.powf(drive / 100.0 * 2.0)
}

fn tone_hz(tone: f32) -> f32 {
    1000.0 * 10.0_f32.powf(tone / 100.0)
}

fn cutoff_coeff(hz: f32, sample_rate: f32) -> f32 {
    (-TAU * hz / sample_rate).exp()
}

struct Smoother {
    target: f32,
    current: f32,
    coeff: f32,
}

impl Smoother {
    fn new(value: f32) -> Smoother {
        Smoother {
            target: value,
            current: value,
            coeff: 0.0,
        }
    }

    fn next(&mut self) -> f32 {
        self.current = self.target + self.coeff * (self.current - self.target);
        self.current
    }

    fn advance(&mut self, frames: usize) -> f32 {
        self.current = self.target + self.coeff.powi(frames as i32) * (self.current - self.target);
        self.current
    }
}
