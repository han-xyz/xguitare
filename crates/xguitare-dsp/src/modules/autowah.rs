use super::{db_to_linear, one_pole_coeff};
use crate::smooth::Smoother;
use crate::svf::{Svf, SvfCoefficients};
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const MODE: usize = 0;
const DIRECTION: usize = 1;
const SENSITIVITY: usize = 2;
const ATTACK: usize = 3;
const DECAY: usize = 4;
const FREQ_LOW: usize = 5;
const FREQ_HIGH: usize = 6;
const Q: usize = 7;
const DRY: usize = 8;
const WET: usize = 9;
const LOW_PASS: usize = 0;
const BAND_PASS: usize = 1;
const DOWN: usize = 1;

pub static SPEC: ModuleSpec = ModuleSpec {
    type_name: "autowah",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "mode",
            min: 0.0,
            max: 2.0,
            default: 0.0,
            unit: Unit::Linear,
            integer: true,
            labels: &["lp", "bp", "hp"],
        },
        ParamSpec {
            name: "direction",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: Unit::Linear,
            integer: true,
            labels: &["up", "down"],
        },
        ParamSpec {
            name: "sensitivity",
            min: -24.0,
            max: 36.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "attack",
            min: 0.5,
            max: 50.0,
            default: 5.0,
            unit: Unit::Ms,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "decay",
            min: 20.0,
            max: 1000.0,
            default: 150.0,
            unit: Unit::Ms,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "freq_low",
            min: 80.0,
            max: 800.0,
            default: 200.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "freq_high",
            min: 400.0,
            max: 4000.0,
            default: 2500.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "q",
            min: 0.5,
            max: 12.0,
            default: 3.0,
            unit: Unit::Linear,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "dry",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "wet",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
    ],
};

/// An envelope filter: an envelope follower on the input sweeps a resonant filter's cutoff between `freq_low` and `freq_high` on a log scale.
pub struct Autowah {
    values: [f32; 10],
    sample_rate: f32,
    sensitivity: Smoother,
    freq_low: Smoother,
    freq_high: Smoother,
    q: Smoother,
    dry: Smoother,
    wet: Smoother,
    attack_coeff: f32,
    decay_coeff: f32,
    envelope: f32,
    filter: Svf,
}

impl Autowah {
    pub fn new() -> Autowah {
        let values: [f32; 10] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let mut autowah = Autowah {
            values,
            sample_rate: 48_000.0,
            sensitivity: Smoother::new(values[SENSITIVITY]),
            freq_low: Smoother::new(values[FREQ_LOW]),
            freq_high: Smoother::new(values[FREQ_HIGH]),
            q: Smoother::new(values[Q]),
            dry: Smoother::new(values[DRY] / 100.0),
            wet: Smoother::new(values[WET] / 100.0),
            attack_coeff: 0.0,
            decay_coeff: 0.0,
            envelope: 0.0,
            filter: Svf::new(),
        };
        autowah.update_coefficients();
        autowah
    }

    fn update_coefficients(&mut self) {
        self.attack_coeff = one_pole_coeff(self.values[ATTACK] / 1000.0, self.sample_rate);
        self.decay_coeff = one_pole_coeff(self.values[DECAY] / 1000.0, self.sample_rate);
    }

    /// Where the cutoff sits between the two frequencies, 0 at one and 1 at the other, from the envelope of `input`.
    fn next_sweep(&mut self, input: f32) -> f32 {
        let level = input.abs();
        let coeff = if level > self.envelope {
            self.attack_coeff
        } else {
            self.decay_coeff
        };
        self.envelope = level + coeff * (self.envelope - level);
        let sweep = (self.envelope * db_to_linear(self.sensitivity.next_value())).clamp(0.0, 1.0);
        if self.values[DIRECTION] as usize == DOWN {
            1.0 - sweep
        } else {
            sweep
        }
    }

    fn coefficients(&mut self, sweep: f32) -> SvfCoefficients {
        let (a, b) = (self.freq_low.next_value(), self.freq_high.next_value());
        let (low, high) = (a.min(b), a.max(b));
        let cutoff = low * (high / low).powf(sweep);
        let q = self.q.next_value();
        match self.values[MODE] as usize {
            LOW_PASS => SvfCoefficients::low_pass(cutoff, q, self.sample_rate),
            BAND_PASS => SvfCoefficients::band_pass(cutoff, q, self.sample_rate),
            _ => SvfCoefficients::high_pass(cutoff, q, self.sample_rate),
        }
    }
}

impl Default for Autowah {
    fn default() -> Autowah {
        Autowah::new()
    }
}

impl Module for Autowah {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.sample_rate = sample_rate;
        for smoother in [
            &mut self.sensitivity,
            &mut self.freq_low,
            &mut self.freq_high,
            &mut self.q,
            &mut self.dry,
            &mut self.wet,
        ] {
            smoother.prepare(sample_rate);
        }
        self.update_coefficients();
        self.envelope = 0.0;
        self.filter.reset();
    }

    fn reset(&mut self) {
        for smoother in [
            &mut self.sensitivity,
            &mut self.freq_low,
            &mut self.freq_high,
            &mut self.q,
            &mut self.dry,
            &mut self.wet,
        ] {
            smoother.snap();
        }
        self.envelope = 0.0;
        self.filter.reset();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = if spec.integer { value.round() } else { value };
        match idx {
            SENSITIVITY => self.sensitivity.set_target(value),
            FREQ_LOW => self.freq_low.set_target(value),
            FREQ_HIGH => self.freq_high.set_target(value),
            Q => self.q.set_target(value),
            DRY => self.dry.set_target(value / 100.0),
            WET => self.wet.set_target(value / 100.0),
            ATTACK | DECAY => self.update_coefficients(),
            MODE | DIRECTION => {}
            _ => unreachable!("autowah has no parameter {idx}"),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            let input = *sample;
            let sweep = self.next_sweep(input);
            let coefficients = self.coefficients(sweep);
            let filtered = self.filter.process(&coefficients, input);
            *sample = self.dry.next_value() * input + self.wet.next_value() * filtered;
        }
    }
}
