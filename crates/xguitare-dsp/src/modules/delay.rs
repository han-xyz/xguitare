use super::one_pole_coeff;
use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const TIME: usize = 0;
const FEEDBACK: usize = 1;
const MIX: usize = 2;
const PINGPONG: usize = 3;
const TIME_SMOOTHING_SECONDS: f32 = 0.05;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "delay",
    channels: Channels::Stereo,
    params: &[
        ParamSpec {
            name: "time",
            min: 10.0,
            max: 2000.0,
            default: 400.0,
            unit: Unit::Ms,
        },
        ParamSpec {
            name: "feedback",
            min: 0.0,
            max: 95.0,
            default: 35.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "mix",
            min: 0.0,
            max: 100.0,
            default: 25.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "pingpong",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: Unit::Linear,
        },
    ],
};

pub struct Delay {
    values: [f32; 4],
    samples_per_ms: f32,
    time_target: f32,
    time_current: f32,
    time_coeff: f32,
    feedback: Smoother,
    mix: Smoother,
    lines: [Vec<f32>; 2],
    write: usize,
}

impl Delay {
    pub fn new() -> Delay {
        let values: [f32; 4] = std::array::from_fn(|idx| SPEC.params[idx].default);
        Delay {
            values,
            samples_per_ms: 48.0,
            time_target: values[TIME],
            time_current: values[TIME],
            time_coeff: 0.0,
            feedback: Smoother::new(values[FEEDBACK] / 100.0),
            mix: Smoother::new(values[MIX] / 100.0),
            lines: [Vec::new(), Vec::new()],
            write: 0,
        }
    }

    fn is_pingpong(&self) -> bool {
        self.values[PINGPONG] >= 0.5
    }
}

impl Default for Delay {
    fn default() -> Delay {
        Delay::new()
    }
}

impl Module for Delay {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.samples_per_ms = sample_rate / 1000.0;
        self.time_coeff = one_pole_coeff(TIME_SMOOTHING_SECONDS, sample_rate);
        self.feedback.prepare(sample_rate);
        self.mix.prepare(sample_rate);
        let len = (SPEC.params[TIME].max * self.samples_per_ms).ceil() as usize + max_frames;
        self.lines = [vec![0.0; len], vec![0.0; len]];
        self.write = 0;
    }

    fn reset(&mut self) {
        self.time_current = self.time_target;
        self.feedback.snap();
        self.mix.snap();
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.write = 0;
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        match idx {
            TIME => self.time_target = value,
            FEEDBACK => self.feedback.set_target(value / 100.0),
            MIX => self.mix.set_target(value / 100.0),
            PINGPONG => {}
            _ => unreachable!("delay has no parameter {idx}"),
        }
        self.values[idx] = if idx == PINGPONG {
            value.round()
        } else {
            value
        };
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let pingpong = self.is_pingpong();
        let len = self.lines[0].len();
        let frames = io.frames;
        for (left, right) in io.left[..frames].iter_mut().zip(&mut io.right[..frames]) {
            self.time_current =
                self.time_target + self.time_coeff * (self.time_current - self.time_target);
            let read = self.write as f32 + len as f32 - self.time_current * self.samples_per_ms;
            let delayed = [
                read_interpolated(&self.lines[0], read),
                read_interpolated(&self.lines[1], read),
            ];
            let feedback = self.feedback.next_value();
            let mix = self.mix.next_value();
            let (written, wet) = if pingpong {
                (
                    [
                        (*left + *right) * 0.5 + feedback * delayed[1],
                        feedback * delayed[0],
                    ],
                    [delayed[1], delayed[0]],
                )
            } else {
                (
                    [
                        *left + feedback * delayed[0],
                        *right + feedback * delayed[1],
                    ],
                    delayed,
                )
            };
            self.lines[0][self.write] = written[0];
            self.lines[1][self.write] = written[1];
            self.write = (self.write + 1) % len;
            *left = *left * (1.0 - mix) + wet[0] * mix;
            *right = *right * (1.0 - mix) + wet[1] * mix;
        }
    }
}

fn read_interpolated(line: &[f32], position: f32) -> f32 {
    let len = line.len();
    let index = position.floor();
    let fraction = position - index;
    let first = index as usize % len;
    let second = (first + 1) % len;
    line[first] + fraction * (line[second] - line[first])
}
