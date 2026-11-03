use std::f32::consts::TAU;

/// One channel's delay line read by a sine LFO; `process` returns the delayed sample and writes `input + feedback × delayed` into the line.
pub struct ModulatedDelay {
    line: Vec<f32>,
    write: usize,
    sample_rate: f32,
    rate_hz: f32,
    phase: f32,
    phase_offset: f32,
}

impl ModulatedDelay {
    pub fn new() -> ModulatedDelay {
        ModulatedDelay {
            line: Vec::new(),
            write: 0,
            sample_rate: 48_000.0,
            rate_hz: 1.0,
            phase: 0.0,
            phase_offset: 0.0,
        }
    }

    pub fn prepare(&mut self, sample_rate: f32, max_delay_ms: f32) {
        self.sample_rate = sample_rate;
        let len = (max_delay_ms * sample_rate / 1000.0).ceil() as usize + 2;
        self.line = vec![0.0; len];
        self.reset();
    }

    pub fn reset(&mut self) {
        self.line.fill(0.0);
        self.write = 0;
        self.phase = 0.0;
    }

    pub fn set_rate(&mut self, hz: f32) {
        self.rate_hz = hz;
    }

    pub fn set_phase_offset(&mut self, radians: f32) {
        self.phase_offset = radians;
    }

    pub fn process(&mut self, input: f32, base_ms: f32, depth_ms: f32, feedback: f32) -> f32 {
        let lfo = (self.phase + self.phase_offset).sin();
        self.phase = (self.phase + TAU * self.rate_hz / self.sample_rate) % TAU;
        let len = self.line.len();
        let delay = (base_ms + depth_ms * lfo) * self.sample_rate / 1000.0;
        let read = self.write as f32 + len as f32 - delay;
        let index = read.floor();
        let fraction = read - index;
        let first = index as usize % len;
        let second = (first + 1) % len;
        let delayed = self.line[first] + fraction * (self.line[second] - self.line[first]);
        self.line[self.write] = input + feedback * delayed;
        self.write = (self.write + 1) % len;
        delayed
    }
}
