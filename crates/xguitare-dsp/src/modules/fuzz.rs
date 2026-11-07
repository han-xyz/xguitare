use std::f32::consts::TAU;

use super::db_to_linear;
use crate::oversample::Oversampler4x;
use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const SUSTAIN: usize = 0;
const TONE: usize = 1;
const LEVEL: usize = 2;
const STAGES: usize = 2;
const INPUT_HIGH_PASS_HZ: f32 = 30.0;
const DC_BLOCK_HZ: f32 = 10.0;
const TONE_LOW_PASS_HZ: f32 = 400.0;
const TONE_HIGH_PASS_HZ: f32 = 1500.0;
const OVERSAMPLING: f32 = 4.0;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "fuzz",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "sustain",
            min: 0.0,
            max: 100.0,
            default: 60.0,
            unit: Unit::Percent,
            integer: false,
        },
        ParamSpec {
            name: "tone",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
        },
        ParamSpec {
            name: "level",
            min: -30.0,
            max: 10.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
        },
    ],
};

pub struct Fuzz {
    values: [f32; 3],
    gain: Smoother,
    tone: Smoother,
    level: Smoother,
    input_high_pass_coeff: f32,
    input_high_pass_state: f32,
    dc_block_coeff: f32,
    dc_blocks: [DcBlock; STAGES],
    low_pass_coeff: f32,
    low_pass_state: f32,
    high_pass_coeff: f32,
    high_pass_state: f32,
    oversampler: Oversampler4x,
}

impl Fuzz {
    pub fn new() -> Fuzz {
        let values = std::array::from_fn(|idx| SPEC.params[idx].default);
        Fuzz {
            values,
            gain: Smoother::new(sustain_gain(values[SUSTAIN])),
            tone: Smoother::new(values[TONE] / 100.0),
            level: Smoother::new(db_to_linear(values[LEVEL])),
            input_high_pass_coeff: 0.0,
            input_high_pass_state: 0.0,
            dc_block_coeff: 0.0,
            dc_blocks: [DcBlock::default(); STAGES],
            low_pass_coeff: 0.0,
            low_pass_state: 0.0,
            high_pass_coeff: 0.0,
            high_pass_state: 0.0,
            oversampler: Oversampler4x::new(),
        }
    }
}

impl Default for Fuzz {
    fn default() -> Fuzz {
        Fuzz::new()
    }
}

impl Module for Fuzz {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        let oversampled_rate = OVERSAMPLING * sample_rate;
        self.gain.prepare(oversampled_rate);
        self.tone.prepare(sample_rate);
        self.level.prepare(sample_rate);
        self.input_high_pass_coeff = cutoff_coeff(INPUT_HIGH_PASS_HZ, oversampled_rate);
        self.dc_block_coeff = cutoff_coeff(DC_BLOCK_HZ, oversampled_rate);
        self.low_pass_coeff = cutoff_coeff(TONE_LOW_PASS_HZ, sample_rate);
        self.high_pass_coeff = cutoff_coeff(TONE_HIGH_PASS_HZ, sample_rate);
        self.oversampler.prepare(max_frames);
    }

    fn reset(&mut self) {
        self.gain.snap();
        self.tone.snap();
        self.level.snap();
        self.input_high_pass_state = 0.0;
        self.dc_blocks = [DcBlock::default(); STAGES];
        self.low_pass_state = 0.0;
        self.high_pass_state = 0.0;
        self.oversampler.reset();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        match idx {
            SUSTAIN => self.gain.set_target(sustain_gain(value)),
            TONE => self.tone.set_target(value / 100.0),
            LEVEL => self.level.set_target(db_to_linear(value)),
            _ => unreachable!("fuzz has no parameter {idx}"),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let mono = &mut io.mono[..io.frames];
        for sample in self.oversampler.upsample(mono) {
            self.input_high_pass_state =
                *sample + self.input_high_pass_coeff * (self.input_high_pass_state - *sample);
            *sample = self.gain.next_value() * (*sample - self.input_high_pass_state);
            for dc_block in &mut self.dc_blocks {
                *sample = dc_block.process(clip(*sample), self.dc_block_coeff);
            }
        }
        self.oversampler.downsample(mono);

        for sample in mono.iter_mut() {
            self.low_pass_state = *sample + self.low_pass_coeff * (self.low_pass_state - *sample);
            self.high_pass_state =
                *sample + self.high_pass_coeff * (self.high_pass_state - *sample);
            let high_pass = *sample - self.high_pass_state;
            let tone = self.tone.next_value();
            // Big Muff tone stack: the split low-pass and high-pass corners scoop the mids at tone 50.
            let shaped = (1.0 - tone) * self.low_pass_state + tone * high_pass;
            *sample = shaped * self.level.next_value();
        }
    }
}

fn sustain_gain(sustain: f32) -> f32 {
    10.0_f32.powf(sustain / 100.0 * 2.3)
}

// Big Muff diode pair: a cubic soft clip inside ±1 that hard-limits outside it.
fn clip(x: f32) -> f32 {
    let x = x.clamp(-1.0, 1.0);
    1.5 * x - 0.5 * x * x * x
}

fn cutoff_coeff(hz: f32, sample_rate: f32) -> f32 {
    (-TAU * hz / sample_rate).exp()
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
