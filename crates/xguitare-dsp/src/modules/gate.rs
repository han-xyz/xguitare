use super::{db_to_linear, one_pole_coeff};
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const THRESHOLD: usize = 0;
const ATTACK: usize = 1;
const RELEASE: usize = 2;
const HYSTERESIS: usize = 3;
const ENVELOPE_DECAY_SECONDS: f32 = 0.05;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "gate",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "threshold",
            min: -90.0,
            max: 0.0,
            default: -60.0,
            unit: Unit::Db,
        },
        ParamSpec {
            name: "attack",
            min: 0.1,
            max: 50.0,
            default: 1.0,
            unit: Unit::Ms,
        },
        ParamSpec {
            name: "release",
            min: 5.0,
            max: 1000.0,
            default: 100.0,
            unit: Unit::Ms,
        },
        ParamSpec {
            name: "hysteresis",
            min: 0.0,
            max: 20.0,
            default: 6.0,
            unit: Unit::Db,
        },
    ],
};

pub struct Gate {
    values: [f32; 4],
    sample_rate: f32,
    open_level: f32,
    close_level: f32,
    attack_coeff: f32,
    release_coeff: f32,
    envelope_coeff: f32,
    envelope: f32,
    open: bool,
    gain: f32,
}

impl Gate {
    pub fn new() -> Gate {
        let mut gate = Gate {
            values: std::array::from_fn(|idx| SPEC.params[idx].default),
            sample_rate: 48_000.0,
            open_level: 0.0,
            close_level: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            envelope_coeff: 0.0,
            envelope: 0.0,
            open: false,
            gain: 0.0,
        };
        gate.update_coefficients();
        gate
    }

    fn update_coefficients(&mut self) {
        let threshold = self.values[THRESHOLD];
        self.open_level = db_to_linear(threshold);
        self.close_level = db_to_linear(threshold - self.values[HYSTERESIS]);
        self.attack_coeff = one_pole_coeff(self.values[ATTACK] / 1000.0, self.sample_rate);
        self.release_coeff = one_pole_coeff(self.values[RELEASE] / 1000.0, self.sample_rate);
        self.envelope_coeff = one_pole_coeff(ENVELOPE_DECAY_SECONDS, self.sample_rate);
    }
}

impl Default for Gate {
    fn default() -> Gate {
        Gate::new()
    }
}

impl Module for Gate {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.sample_rate = sample_rate;
        self.update_coefficients();
    }

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.open = false;
        self.gain = 0.0;
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        self.values[idx] = value.clamp(spec.min, spec.max);
        self.update_coefficients();
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            self.envelope = sample.abs().max(self.envelope * self.envelope_coeff);
            if self.envelope > self.open_level {
                self.open = true;
            } else if self.envelope < self.close_level {
                self.open = false;
            }
            let (target, coeff) = if self.open {
                (1.0, self.attack_coeff)
            } else {
                (0.0, self.release_coeff)
            };
            self.gain = target + coeff * (self.gain - target);
            *sample *= self.gain;
        }
    }
}
