use std::f32::consts::TAU;

use super::db_to_linear;
use crate::oversample::Oversampler4x;
use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const GAIN: usize = 0;
const BIAS: usize = 1;
const MASTER: usize = 2;
const STAGES: usize = 3;
const MAX_STAGES: usize = 4;
const BIAS_SCALE: f32 = 0.3;
const DC_BLOCK_HZ: f32 = 10.0;
const OVERSAMPLING: f32 = 4.0;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "preamp",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "gain",
            min: 0.0,
            max: 100.0,
            default: 40.0,
            unit: Unit::Percent,
            integer: false,
        },
        ParamSpec {
            name: "bias",
            min: -1.0,
            max: 1.0,
            default: 0.0,
            unit: Unit::Linear,
            integer: false,
        },
        ParamSpec {
            name: "master",
            min: -60.0,
            max: 0.0,
            default: -12.0,
            unit: Unit::Db,
            integer: false,
        },
        ParamSpec {
            name: "stages",
            min: 1.0,
            max: MAX_STAGES as f32,
            default: 3.0,
            unit: Unit::Linear,
            integer: true,
        },
    ],
};

pub struct Preamp {
    values: [f32; 4],
    stages: usize,
    gain: Smoother,
    bias: Smoother,
    master: Smoother,
    dc_block_coeff: f32,
    dc_blocks: [DcBlock; MAX_STAGES],
    oversampler: Oversampler4x,
}

impl Preamp {
    pub fn new() -> Preamp {
        let values = std::array::from_fn(|idx| SPEC.params[idx].default);
        Preamp {
            values,
            stages: values[STAGES] as usize,
            gain: Smoother::new(total_gain(values[GAIN])),
            bias: Smoother::new(values[BIAS] * BIAS_SCALE),
            master: Smoother::new(db_to_linear(values[MASTER])),
            dc_block_coeff: 0.0,
            dc_blocks: [DcBlock::default(); MAX_STAGES],
            oversampler: Oversampler4x::new(),
        }
    }
}

impl Default for Preamp {
    fn default() -> Preamp {
        Preamp::new()
    }
}

impl Module for Preamp {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        let oversampled_rate = OVERSAMPLING * sample_rate;
        self.gain.prepare(oversampled_rate);
        self.bias.prepare(oversampled_rate);
        self.master.prepare(sample_rate);
        self.dc_block_coeff = (-TAU * DC_BLOCK_HZ / oversampled_rate).exp();
        self.oversampler.prepare(max_frames);
    }

    fn reset(&mut self) {
        self.gain.snap();
        self.bias.snap();
        self.master.snap();
        self.dc_blocks = [DcBlock::default(); MAX_STAGES];
        self.oversampler.reset();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let mut value = value.clamp(spec.min, spec.max);
        match idx {
            GAIN => self.gain.set_target(total_gain(value)),
            BIAS => self.bias.set_target(value * BIAS_SCALE),
            MASTER => self.master.set_target(db_to_linear(value)),
            STAGES => {
                value = value.round();
                self.stages = value as usize;
            }
            _ => unreachable!("preamp has no parameter {idx}"),
        }
        self.values[idx] = value;
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let mono = &mut io.mono[..io.frames];
        let per_stage = 1.0 / self.stages as f32;
        for sample in self.oversampler.upsample(mono) {
            let stage_gain = self.gain.next_value().powf(per_stage);
            let bias = self.bias.next_value();
            let rest = bias.tanh();
            for dc_block in &mut self.dc_blocks[..self.stages] {
                let shaped = (stage_gain * *sample + bias).tanh() - rest;
                *sample = dc_block.process(shaped, self.dc_block_coeff);
            }
        }
        self.oversampler.downsample(mono);

        for sample in mono.iter_mut() {
            *sample *= self.master.next_value();
        }
    }
}

fn total_gain(gain: f32) -> f32 {
    10.0_f32.powf(gain / 100.0 * 1.6)
}

#[derive(Clone, Copy, Default)]
struct DcBlock {
    previous_input: f32,
    previous_output: f32,
}

impl DcBlock {
    fn process(&mut self, input: f32, coeff: f32) -> f32 {
        self.previous_output = input - self.previous_input + coeff * self.previous_output;
        self.previous_input = input;
        self.previous_output
    }
}
