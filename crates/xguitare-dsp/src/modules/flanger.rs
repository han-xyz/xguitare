use std::f32::consts::FRAC_PI_2;

use crate::modulated_delay::ModulatedDelay;
use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const RATE: usize = 0;
const DEPTH: usize = 1;
const FEEDBACK: usize = 2;
const MIX: usize = 3;
const MIN_DELAY_MS: f32 = 0.5;
const MAX_SPAN_MS: f32 = 9.5;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "flanger",
    channels: Channels::Stereo,
    params: &[
        ParamSpec {
            name: "rate",
            min: 0.05,
            max: 5.0,
            default: 0.3,
            unit: Unit::Hz,
            integer: false,
        },
        ParamSpec {
            name: "depth",
            min: 0.0,
            max: 100.0,
            default: 60.0,
            unit: Unit::Percent,
            integer: false,
        },
        ParamSpec {
            name: "feedback",
            min: -95.0,
            max: 95.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
        },
        ParamSpec {
            name: "mix",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
        },
    ],
};

pub struct Flanger {
    values: [f32; 4],
    rate: Smoother,
    depth: Smoother,
    feedback: Smoother,
    mix: Smoother,
    lines: [ModulatedDelay; 2],
}

impl Flanger {
    pub fn new() -> Flanger {
        let values: [f32; 4] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let mut right = ModulatedDelay::new();
        right.set_phase_offset(FRAC_PI_2);
        Flanger {
            values,
            rate: Smoother::new(values[RATE]),
            depth: Smoother::new(values[DEPTH] / 100.0),
            feedback: Smoother::new(values[FEEDBACK] / 100.0),
            mix: Smoother::new(values[MIX] / 100.0),
            lines: [ModulatedDelay::new(), right],
        }
    }
}

impl Default for Flanger {
    fn default() -> Flanger {
        Flanger::new()
    }
}

impl Module for Flanger {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.rate.prepare(sample_rate);
        self.depth.prepare(sample_rate);
        self.feedback.prepare(sample_rate);
        self.mix.prepare(sample_rate);
        for line in &mut self.lines {
            line.prepare(sample_rate, MIN_DELAY_MS + MAX_SPAN_MS);
        }
    }

    fn reset(&mut self) {
        self.rate.snap();
        self.depth.snap();
        self.feedback.snap();
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
            FEEDBACK => self.feedback.set_target(value / 100.0),
            MIX => self.mix.set_target(value / 100.0),
            _ => unreachable!("flanger has no parameter {idx}"),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let frames = io.frames;
        for (left, right) in io.left[..frames].iter_mut().zip(&mut io.right[..frames]) {
            let rate = self.rate.next_value();
            // delay = 0.5 ms + span × (0.5 + 0.5·sin): the sweep runs from 0.5 ms up to 0.5 ms + span.
            let half_span_ms = self.depth.next_value() * MAX_SPAN_MS / 2.0;
            let feedback = self.feedback.next_value();
            let mix = self.mix.next_value();
            for (sample, line) in [left, right].into_iter().zip(&mut self.lines) {
                line.set_rate(rate);
                let wet =
                    line.process(*sample, MIN_DELAY_MS + half_span_ms, half_span_ms, feedback);
                *sample = *sample * (1.0 - mix) + wet * mix;
            }
        }
    }
}
