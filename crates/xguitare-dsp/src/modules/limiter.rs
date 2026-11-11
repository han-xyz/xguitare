use super::{db_to_linear, one_pole_coeff};
use crate::smooth::Smoother;
use crate::window_peak::WindowPeak;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const INPUT: usize = 0;
const CEILING: usize = 1;
const RELEASE: usize = 2;
const LOOKAHEAD: usize = 3;
const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

pub static SPEC: ModuleSpec = ModuleSpec {
    type_name: "limiter",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "input",
            min: 0.0,
            max: 18.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "ceiling",
            min: -12.0,
            max: 0.0,
            default: -1.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "release",
            min: 10.0,
            max: 1000.0,
            default: 100.0,
            unit: Unit::Ms,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "lookahead",
            min: 0.0,
            max: 5.0,
            default: 1.0,
            unit: Unit::Ms,
            integer: false,
            labels: &[],
        },
    ],
};

/// A brickwall limiter: the audio is delayed by `lookahead` while a peak detector over that window sets the gain a peak needs; the gain glides there across the delay, reaching it as the peak leaves, and a final clamp holds every sample at the ceiling.
pub struct Limiter {
    values: [f32; 4],
    sample_rate: f32,
    input: Smoother,
    ceiling: Smoother,
    delay: Smoother,
    release_coeff: f32,
    lines: [Vec<f32>; 2],
    write: usize,
    window: WindowPeak,
    envelope: f32,
    glide: GainGlide,
}

impl Limiter {
    pub fn new() -> Limiter {
        let values: [f32; 4] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let mut limiter = Limiter {
            values,
            sample_rate: DEFAULT_SAMPLE_RATE,
            input: Smoother::new(values[INPUT]),
            ceiling: Smoother::new(values[CEILING]),
            delay: Smoother::new(0.0),
            release_coeff: 0.0,
            lines: [Vec::new(), Vec::new()],
            write: 0,
            window: WindowPeak::new(0),
            envelope: 0.0,
            glide: GainGlide::new(0),
        };
        limiter.prepare(DEFAULT_SAMPLE_RATE, 0);
        limiter
    }

    fn delay_samples(&self) -> f32 {
        (self.values[LOOKAHEAD] / 1000.0 * self.sample_rate).round()
    }

    fn update_release(&mut self) {
        self.release_coeff = one_pole_coeff(self.values[RELEASE] / 1000.0, self.sample_rate);
    }

    fn clear(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.write = 0;
        self.window.clear();
        self.envelope = 0.0;
        self.glide.clear();
    }

    /// Takes the detector level of the frame just written to the delay; returns the gain and ceiling for the frame leaving it, its read offset and the fraction toward the next older sample.
    fn step(&mut self, level: f32) -> (f32, f32, usize, f32) {
        let delay = self.delay.next_value_settling();
        let whole = delay.floor();
        let window = delay.ceil() as usize;
        self.envelope = self
            .window
            .push(level, window)
            .max(self.envelope * self.release_coeff);
        let ceiling = db_to_linear(self.ceiling.next_value());
        let target = if self.envelope > ceiling {
            ceiling / self.envelope
        } else {
            1.0
        };
        let gain = self.glide.push(target, window + 1);
        (gain, ceiling, whole as usize, delay - whole)
    }

    fn advance_write(&mut self) {
        self.write = (self.write + 1) % self.lines[0].len();
    }

    fn read(&self, side: usize, offset: usize, fraction: f32) -> f32 {
        let line = &self.lines[side];
        let len = line.len();
        let newer = line[(self.write + len - offset) % len];
        if fraction == 0.0 {
            return newer;
        }
        let older = line[(self.write + len - offset - 1) % len];
        newer + fraction * (older - newer)
    }
}

impl Default for Limiter {
    fn default() -> Limiter {
        Limiter::new()
    }
}

impl Module for Limiter {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.sample_rate = sample_rate;
        self.input.prepare(sample_rate);
        self.ceiling.prepare(sample_rate);
        self.delay.prepare(sample_rate);
        let max_delay = (SPEC.params[LOOKAHEAD].max / 1000.0 * sample_rate).ceil() as usize;
        self.lines = [vec![0.0; max_delay + 2], vec![0.0; max_delay + 2]];
        self.window = WindowPeak::new(max_delay + 2);
        self.glide = GainGlide::new(max_delay + 2);
        self.delay.set_target(self.delay_samples());
        self.delay.snap();
        self.update_release();
        self.clear();
    }

    fn reset(&mut self) {
        self.input.snap();
        self.ceiling.snap();
        self.delay.snap();
        self.clear();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        match idx {
            INPUT => self.input.set_target(value),
            CEILING => self.ceiling.set_target(value),
            RELEASE => self.update_release(),
            _ => self.delay.set_target(self.delay_samples()),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            let input = *sample * db_to_linear(self.input.next_value());
            self.lines[0][self.write] = input;
            let (gain, ceiling, offset, fraction) = self.step(input.abs());
            *sample = (self.read(0, offset, fraction) * gain).clamp(-ceiling, ceiling);
            self.advance_write();
        }
    }

    fn process_linked(&mut self, left: &mut [f32], right: &mut [f32]) -> bool {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let input_gain = db_to_linear(self.input.next_value());
            let (in_l, in_r) = (*l * input_gain, *r * input_gain);
            self.lines[0][self.write] = in_l;
            self.lines[1][self.write] = in_r;
            let (gain, ceiling, offset, fraction) = self.step(in_l.abs().max(in_r.abs()));
            *l = (self.read(0, offset, fraction) * gain).clamp(-ceiling, ceiling);
            *r = (self.read(1, offset, fraction) * gain).clamp(-ceiling, ceiling);
            self.advance_write();
        }
        true
    }
}

/// The mean of the last `len` gains, kept as a running sum of their shortfalls below 1. Over a window peak held for `len` frames it falls in even steps to a peak's gain by the frame the peak leaves the delay; with no shortfall in the window it is exactly 1.
struct GainGlide {
    shortfalls: Vec<f32>,
    write: usize,
    len: usize,
    sum: f32,
    short: usize,
}

impl GainGlide {
    fn new(capacity: usize) -> GainGlide {
        GainGlide {
            shortfalls: vec![0.0; capacity.max(1)],
            write: 0,
            len: 0,
            sum: 0.0,
            short: 0,
        }
    }

    fn clear(&mut self) {
        self.shortfalls.fill(0.0);
        self.write = 0;
        self.len = 0;
        self.sum = 0.0;
        self.short = 0;
    }

    fn push(&mut self, gain: f32, len: usize) -> f32 {
        let capacity = self.shortfalls.len();
        let shortfall = 1.0 - gain;
        self.shortfalls[self.write] = shortfall;
        self.write = (self.write + 1) % capacity;
        self.sum += shortfall;
        self.short += usize::from(shortfall > 0.0);
        self.len += 1;
        while self.len > len {
            let oldest = self.shortfalls[(self.write + capacity - self.len) % capacity];
            self.sum -= oldest;
            self.short -= usize::from(oldest > 0.0);
            self.len -= 1;
        }
        if self.short == 0 {
            self.sum = 0.0;
        }
        1.0 - self.sum / self.len as f32
    }
}
