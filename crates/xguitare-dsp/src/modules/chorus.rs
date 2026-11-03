use std::f32::consts::FRAC_PI_2;

use crate::modulated_delay::ModulatedDelay;
use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const RATE: usize = 0;
const DEPTH: usize = 1;
const MIX: usize = 2;
const LINE_MS: f32 = 40.0;
const BASE_MS: f32 = 15.0;
const MAX_DEPTH_MS: f32 = 5.0;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "chorus",
    channels: Channels::Stereo,
    params: &[
        ParamSpec {
            name: "rate",
            min: 0.1,
            max: 5.0,
            default: 1.0,
            unit: Unit::Hz,
            integer: false,
        },
        ParamSpec {
            name: "depth",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
        },
        ParamSpec {
            name: "mix",
            min: 0.0,
            max: 100.0,
            default: 40.0,
            unit: Unit::Percent,
            integer: false,
        },
    ],
};

pub struct Chorus {
    values: [f32; 3],
    rate: Smoother,
    depth: Smoother,
    mix: Smoother,
    lines: [ModulatedDelay; 2],
}

impl Chorus {
    pub fn new() -> Chorus {
        let values: [f32; 3] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let mut right = ModulatedDelay::new();
        right.set_phase_offset(FRAC_PI_2);
        Chorus {
            values,
            rate: Smoother::new(values[RATE]),
            depth: Smoother::new(values[DEPTH] / 100.0),
            mix: Smoother::new(values[MIX] / 100.0),
            lines: [ModulatedDelay::new(), right],
        }
    }
}

impl Default for Chorus {
    fn default() -> Chorus {
        Chorus::new()
    }
}

impl Module for Chorus {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.rate.prepare(sample_rate);
        self.depth.prepare(sample_rate);
        self.mix.prepare(sample_rate);
        for line in &mut self.lines {
            line.prepare(sample_rate, LINE_MS);
        }
    }

    fn reset(&mut self) {
        self.rate.snap();
        self.depth.snap();
        self.mix.snap();
        for line in &mut self.lines {
            line.reset();
        }
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        match idx {
            RATE => self.rate.set_target(value),
            DEPTH => self.depth.set_target(value / 100.0),
            MIX => self.mix.set_target(value / 100.0),
            _ => unreachable!("chorus has no parameter {idx}"),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let frames = io.frames;
        for (left, right) in io.left[..frames].iter_mut().zip(&mut io.right[..frames]) {
            let rate = self.rate.next_value();
            let depth_ms = self.depth.next_value() * MAX_DEPTH_MS;
            let mix = self.mix.next_value();
            for (sample, line) in [left, right].into_iter().zip(&mut self.lines) {
                line.set_rate(rate);
                let wet = line.process(*sample, BASE_MS, depth_ms, 0.0);
                *sample = *sample * (1.0 - mix) + wet * mix;
            }
        }
    }
}
