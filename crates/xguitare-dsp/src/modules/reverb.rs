use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const SIZE: usize = 0;
const DAMPING: usize = 1;
const MIX: usize = 2;
const WIDTH: usize = 3;

// Freeverb tunings by Jezar at Dreampoint (public domain), in samples at 44.1 kHz.
const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const TUNING_RATE: f32 = 44_100.0;
const FIXED_GAIN: f32 = 0.015;
const ALLPASS_FEEDBACK: f32 = 0.5;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "reverb",
    channels: Channels::Stereo,
    params: &[
        ParamSpec {
            name: "size",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "damping",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "mix",
            min: 0.0,
            max: 100.0,
            default: 20.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "width",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            unit: Unit::Percent,
        },
    ],
};

pub struct Reverb {
    values: [f32; 4],
    feedback: Smoother,
    damping: Smoother,
    mix: Smoother,
    width: Smoother,
    channels: [Network; 2],
}

impl Reverb {
    pub fn new() -> Reverb {
        let values: [f32; 4] = std::array::from_fn(|idx| SPEC.params[idx].default);
        Reverb {
            values,
            feedback: Smoother::new(comb_feedback(values[SIZE])),
            damping: Smoother::new(comb_damping(values[DAMPING])),
            mix: Smoother::new(values[MIX] / 100.0),
            width: Smoother::new(values[WIDTH] / 100.0),
            channels: [Network::default(), Network::default()],
        }
    }
}

impl Default for Reverb {
    fn default() -> Reverb {
        Reverb::new()
    }
}

impl Module for Reverb {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        for smoother in [
            &mut self.feedback,
            &mut self.damping,
            &mut self.mix,
            &mut self.width,
        ] {
            smoother.prepare(sample_rate);
        }
        let scale = sample_rate / TUNING_RATE;
        self.channels = [Network::new(scale, 0), Network::new(scale, STEREO_SPREAD)];
    }

    fn reset(&mut self) {
        for smoother in [
            &mut self.feedback,
            &mut self.damping,
            &mut self.mix,
            &mut self.width,
        ] {
            smoother.snap();
        }
        for network in &mut self.channels {
            network.reset();
        }
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        match idx {
            SIZE => self.feedback.set_target(comb_feedback(value)),
            DAMPING => self.damping.set_target(comb_damping(value)),
            MIX => self.mix.set_target(value / 100.0),
            WIDTH => self.width.set_target(value / 100.0),
            _ => unreachable!("reverb has no parameter {idx}"),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let frames = io.frames;
        for (left, right) in io.left[..frames].iter_mut().zip(&mut io.right[..frames]) {
            let feedback = self.feedback.next_value();
            let damping = self.damping.next_value();
            let mix = self.mix.next_value();
            let width = self.width.next_value();
            let input = (*left + *right) * FIXED_GAIN;
            let wet_left = self.channels[0].process(input, feedback, damping);
            let wet_right = self.channels[1].process(input, feedback, damping);
            let wet1 = (width + 1.0) / 2.0;
            let wet2 = (1.0 - width) / 2.0;
            *left = *left * (1.0 - mix) + (wet_left * wet1 + wet_right * wet2) * mix;
            *right = *right * (1.0 - mix) + (wet_right * wet1 + wet_left * wet2) * mix;
        }
    }
}

fn comb_feedback(size: f32) -> f32 {
    0.7 + 0.28 * size / 100.0
}

fn comb_damping(damping: f32) -> f32 {
    0.4 * damping / 100.0
}

#[derive(Default)]
struct Network {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
}

impl Network {
    fn new(scale: f32, spread: usize) -> Network {
        let length = |tuning: usize| ((tuning + spread) as f32 * scale).round() as usize;
        Network {
            combs: COMB_TUNINGS
                .iter()
                .map(|&tuning| Comb::new(length(tuning)))
                .collect(),
            allpasses: ALLPASS_TUNINGS
                .iter()
                .map(|&tuning| Allpass::new(length(tuning)))
                .collect(),
        }
    }

    fn reset(&mut self) {
        for comb in &mut self.combs {
            comb.buffer.fill(0.0);
            comb.store = 0.0;
        }
        for allpass in &mut self.allpasses {
            allpass.buffer.fill(0.0);
        }
    }

    fn process(&mut self, input: f32, feedback: f32, damping: f32) -> f32 {
        let combed: f32 = self
            .combs
            .iter_mut()
            .map(|comb| comb.process(input, feedback, damping))
            .sum();
        self.allpasses
            .iter_mut()
            .fold(combed, |signal, allpass| allpass.process(signal))
    }
}

struct Comb {
    buffer: Vec<f32>,
    index: usize,
    store: f32,
}

impl Comb {
    fn new(length: usize) -> Comb {
        Comb {
            buffer: vec![0.0; length],
            index: 0,
            store: 0.0,
        }
    }

    fn process(&mut self, input: f32, feedback: f32, damping: f32) -> f32 {
        let output = self.buffer[self.index];
        self.store = output * (1.0 - damping) + self.store * damping;
        self.buffer[self.index] = input + self.store * feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }
}

struct Allpass {
    buffer: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(length: usize) -> Allpass {
        Allpass {
            buffer: vec![0.0; length],
            index: 0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.index];
        self.buffer[self.index] = input + delayed * ALLPASS_FEEDBACK;
        self.index = (self.index + 1) % self.buffer.len();
        delayed - input
    }
}
