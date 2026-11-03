use std::f32::consts::PI;

const TAPS: usize = 31;
const CENTRE: usize = TAPS / 2;

pub struct Oversampler4x {
    up: [HalfBand; 2],
    down: [HalfBand; 2],
    mid: Vec<f32>,
    high: Vec<f32>,
}

impl Oversampler4x {
    pub fn new() -> Oversampler4x {
        let stage = HalfBand::new(half_band_kernel());
        Oversampler4x {
            up: [stage.clone(), stage.clone()],
            down: [stage.clone(), stage],
            mid: Vec::new(),
            high: Vec::new(),
        }
    }

    pub fn prepare(&mut self, max_frames: usize) {
        self.mid = vec![0.0; 2 * max_frames];
        self.high = vec![0.0; 4 * max_frames];
    }

    pub fn reset(&mut self) {
        for stage in self.up.iter_mut().chain(&mut self.down) {
            stage.reset();
        }
    }

    pub fn upsample(&mut self, input: &[f32]) -> &mut [f32] {
        let frames = input.len();
        self.up[0].interpolate(input, &mut self.mid[..2 * frames]);
        self.up[1].interpolate(&self.mid[..2 * frames], &mut self.high[..4 * frames]);
        &mut self.high[..4 * frames]
    }

    pub fn downsample(&mut self, output: &mut [f32]) {
        let frames = output.len();
        self.down[0].decimate(&self.high[..4 * frames], &mut self.mid[..2 * frames]);
        self.down[1].decimate(&self.mid[..2 * frames], output);
    }
}

impl Default for Oversampler4x {
    fn default() -> Oversampler4x {
        Oversampler4x::new()
    }
}

#[derive(Clone)]
struct HalfBand {
    kernel: [f32; TAPS],
    history: [f32; 2 * TAPS],
    pos: usize,
}

impl HalfBand {
    fn new(kernel: [f32; TAPS]) -> HalfBand {
        HalfBand {
            kernel,
            history: [0.0; 2 * TAPS],
            pos: 0,
        }
    }

    fn reset(&mut self) {
        self.history = [0.0; 2 * TAPS];
        self.pos = 0;
    }

    fn interpolate(&mut self, input: &[f32], output: &mut [f32]) {
        for (&sample, [even, odd]) in input.iter().zip(output.as_chunks_mut::<2>().0) {
            *even = self.push(2.0 * sample);
            *odd = self.push(0.0);
        }
    }

    fn decimate(&mut self, input: &[f32], output: &mut [f32]) {
        for (&[even, odd], out) in input.as_chunks::<2>().0.iter().zip(output) {
            *out = self.push(even);
            self.push(odd);
        }
    }

    fn push(&mut self, sample: f32) -> f32 {
        self.pos = if self.pos == 0 {
            TAPS - 1
        } else {
            self.pos - 1
        };
        self.history[self.pos] = sample;
        self.history[self.pos + TAPS] = sample;
        self.kernel
            .iter()
            .zip(&self.history[self.pos..self.pos + TAPS])
            .map(|(k, x)| k * x)
            .sum()
    }
}

// Half-band low-pass: sinc with cutoff at a quarter of the rate, Blackman-windowed, normalised to unity DC gain.
fn half_band_kernel() -> [f32; TAPS] {
    let mut kernel = [0.0; TAPS];
    for (n, tap) in kernel.iter_mut().enumerate() {
        let k = n as f32 - CENTRE as f32;
        let sinc = if n == CENTRE {
            0.5
        } else if n.abs_diff(CENTRE) % 2 == 0 {
            0.0
        } else {
            (PI * k / 2.0).sin() / (PI * k)
        };
        let phase = 2.0 * PI * n as f32 / (TAPS - 1) as f32;
        let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
        *tap = sinc * window;
    }
    let sum: f32 = kernel.iter().sum();
    kernel.map(|tap| tap / sum)
}
