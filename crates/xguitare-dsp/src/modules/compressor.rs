use super::{db_to_linear, one_pole_coeff};
use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const THRESHOLD: usize = 0;
const RATIO: usize = 1;
const ATTACK: usize = 2;
const RELEASE: usize = 3;
const MAKEUP: usize = 4;
const KNEE_DB: f32 = 6.0;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "compressor",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "threshold",
            min: -60.0,
            max: 0.0,
            default: -20.0,
            unit: Unit::Db,
            integer: false,
        },
        ParamSpec {
            name: "ratio",
            min: 1.0,
            max: 20.0,
            default: 4.0,
            unit: Unit::Linear,
            integer: false,
        },
        ParamSpec {
            name: "attack",
            min: 0.1,
            max: 100.0,
            default: 10.0,
            unit: Unit::Ms,
            integer: false,
        },
        ParamSpec {
            name: "release",
            min: 10.0,
            max: 1000.0,
            default: 100.0,
            unit: Unit::Ms,
            integer: false,
        },
        ParamSpec {
            name: "makeup",
            min: 0.0,
            max: 24.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
        },
    ],
};

pub struct Compressor {
    values: [f32; 5],
    sample_rate: f32,
    threshold: Smoother,
    ratio: Smoother,
    makeup: Smoother,
    attack_coeff: f32,
    release_coeff: f32,
    envelope: f32,
    gain_db: f32,
}

impl Compressor {
    pub fn new() -> Compressor {
        let values: [f32; 5] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let mut compressor = Compressor {
            values,
            sample_rate: 48_000.0,
            threshold: Smoother::new(values[THRESHOLD]),
            ratio: Smoother::new(values[RATIO]),
            makeup: Smoother::new(values[MAKEUP]),
            attack_coeff: 0.0,
            release_coeff: 0.0,
            envelope: 0.0,
            gain_db: 0.0,
        };
        compressor.update_coefficients();
        compressor
    }

    fn update_coefficients(&mut self) {
        self.attack_coeff = one_pole_coeff(self.values[ATTACK] / 1000.0, self.sample_rate);
        self.release_coeff = one_pole_coeff(self.values[RELEASE] / 1000.0, self.sample_rate);
    }
}

impl Default for Compressor {
    fn default() -> Compressor {
        Compressor::new()
    }
}

fn gain_reduction_db(over: f32, ratio: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio;
    let half_knee = KNEE_DB / 2.0;
    if over <= -half_knee {
        0.0
    } else if over < half_knee {
        // Quadratic knee: meets 0 at -knee/2 and the ratio line `over × slope` at +knee/2.
        slope * (over + half_knee).powi(2) / (2.0 * KNEE_DB)
    } else {
        slope * over
    }
}

impl Module for Compressor {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.sample_rate = sample_rate;
        self.threshold.prepare(sample_rate);
        self.ratio.prepare(sample_rate);
        self.makeup.prepare(sample_rate);
        self.update_coefficients();
    }

    fn reset(&mut self) {
        self.threshold.snap();
        self.ratio.snap();
        self.makeup.snap();
        self.envelope = 0.0;
        self.gain_db = 0.0;
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        match idx {
            THRESHOLD => self.threshold.set_target(value),
            RATIO => self.ratio.set_target(value),
            MAKEUP => self.makeup.set_target(value),
            _ => self.update_coefficients(),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            let threshold = self.threshold.next_value();
            let ratio = self.ratio.next_value();
            let makeup = self.makeup.next_value();
            self.envelope = sample.abs().max(self.envelope * self.release_coeff);
            let level = 20.0 * self.envelope.log10();
            let target = -gain_reduction_db(level - threshold, ratio);
            let coeff = if target < self.gain_db {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.gain_db = target + coeff * (self.gain_db - target);
            *sample *= db_to_linear(self.gain_db + makeup);
        }
    }
}
