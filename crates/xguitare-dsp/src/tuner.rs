use std::fmt;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

const WINDOW_SECONDS: f32 = 0.085;
const SILENCE_RMS: f32 = 0.001;
const PEAK_RATIO: f32 = 0.8;
const CONFIDENT_CLARITY: f32 = 0.90;
const MIN_HZ: f32 = 35.0;
const MAX_HZ: f32 = 1400.0;
const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pitch {
    pub hz: f32,
    pub clarity: f32,
    pub confident: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub name: &'static str,
    pub octave: i32,
    pub cents: i32,
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}{}", self.name, self.octave)
    }
}

/// The nearest note to `hz` with A4 at `reference_hz`: sharps only, scientific octaves (C4 is middle C), cents in -49..=50 so a pitch exactly between two notes names the lower one.
pub fn note_name(hz: f32, reference_hz: f32) -> Note {
    let total_cents = (1200.0 * (hz / reference_hz).log2()).round() as i32;
    let semitones = (total_cents + 49).div_euclid(100);
    let midi = 69 + semitones;
    Note {
        name: NAMES[midi.rem_euclid(12) as usize],
        octave: midi.div_euclid(12) - 1,
        cents: total_cents - 100 * semitones,
    }
}

/// McLeod pitch detection over a sliding window of the most recent 85 ms (rounded up to a power of two); all buffers are allocated in `new`, so `push` and `analyse` never allocate.
pub struct Detector {
    sample_rate: f32,
    window: Vec<f32>,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    fft_input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    autocorrelation: Vec<f32>,
    forward_scratch: Vec<Complex<f32>>,
    inverse_scratch: Vec<Complex<f32>>,
    nsdf: Vec<f32>,
}

impl Detector {
    pub fn new(sample_rate: f32) -> Detector {
        let size = ((WINDOW_SECONDS * sample_rate).ceil() as usize).next_power_of_two();
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(2 * size);
        let inverse = planner.plan_fft_inverse(2 * size);
        Detector {
            sample_rate,
            window: vec![0.0; size],
            fft_input: forward.make_input_vec(),
            spectrum: forward.make_output_vec(),
            autocorrelation: inverse.make_output_vec(),
            forward_scratch: forward.make_scratch_vec(),
            inverse_scratch: inverse.make_scratch_vec(),
            nsdf: vec![0.0; size / 2],
            forward,
            inverse,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        let size = self.window.len();
        if samples.len() >= size {
            self.window
                .copy_from_slice(&samples[samples.len() - size..]);
            return;
        }
        self.window.copy_within(samples.len().., 0);
        self.window[size - samples.len()..].copy_from_slice(samples);
    }

    pub fn analyse(&mut self) -> Option<Pitch> {
        let energy: f32 = self.window.iter().map(|s| s * s).sum();
        if (energy / self.window.len() as f32).sqrt() < SILENCE_RMS {
            return None;
        }
        self.compute_nsdf(energy);
        let (lag, clarity) = self.pick_peak()?;
        let hz = self.sample_rate / lag;
        if !(MIN_HZ..=MAX_HZ).contains(&hz) {
            return None;
        }
        Some(Pitch {
            hz,
            clarity,
            confident: clarity >= CONFIDENT_CLARITY,
        })
    }

    fn compute_nsdf(&mut self, energy: f32) {
        let size = self.window.len();
        self.fft_input[..size].copy_from_slice(&self.window);
        self.fft_input[size..].fill(0.0);
        self.forward
            .process_with_scratch(
                &mut self.fft_input,
                &mut self.spectrum,
                &mut self.forward_scratch,
            )
            .expect("buffers are sized for the planned FFT");
        for bin in &mut self.spectrum {
            *bin = Complex::new(bin.norm_sqr(), 0.0);
        }
        self.inverse
            .process_with_scratch(
                &mut self.spectrum,
                &mut self.autocorrelation,
                &mut self.inverse_scratch,
            )
            .expect("buffers are sized for the planned FFT");
        let scale = 1.0 / (2 * size) as f32;
        let mut squares = 2.0 * energy;
        for (lag, value) in self.nsdf.iter_mut().enumerate() {
            if lag > 0 {
                squares -= self.window[lag - 1].powi(2) + self.window[size - lag].powi(2);
            }
            *value = if squares > 0.0 {
                2.0 * self.autocorrelation[lag] * scale / squares
            } else {
                0.0
            };
        }
    }

    // McLeod: one key maximum per positive lobe after the first negative-going zero crossing; the first key maximum within PEAK_RATIO of the highest wins, which keeps the fundamental over its harmonics' sub-multiples.
    fn pick_peak(&self) -> Option<(f32, f32)> {
        let highest = self
            .key_maxima()
            .map(|lag| self.nsdf[lag])
            .reduce(f32::max)?;
        let lag = self
            .key_maxima()
            .find(|&lag| self.nsdf[lag] >= PEAK_RATIO * highest)?;
        Some(self.refine(lag))
    }

    fn key_maxima(&self) -> impl Iterator<Item = usize> + '_ {
        let nsdf = &self.nsdf;
        let start = (1..nsdf.len())
            .find(|&lag| nsdf[lag - 1] > 0.0 && nsdf[lag] <= 0.0)
            .unwrap_or(nsdf.len());
        let mut lag = start;
        std::iter::from_fn(move || {
            while lag < nsdf.len() && nsdf[lag] <= 0.0 {
                lag += 1;
            }
            let mut best = lag;
            while lag < nsdf.len() && nsdf[lag] > 0.0 {
                if nsdf[lag] > nsdf[best] {
                    best = lag;
                }
                lag += 1;
            }
            (best + 1 < nsdf.len()).then_some(best)
        })
    }

    fn refine(&self, lag: usize) -> (f32, f32) {
        let (a, b, c) = (self.nsdf[lag - 1], self.nsdf[lag], self.nsdf[lag + 1]);
        let curvature = a - 2.0 * b + c;
        if curvature >= 0.0 {
            return (lag as f32, b);
        }
        let shift = 0.5 * (a - c) / curvature;
        (lag as f32 + shift, b - 0.25 * (a - c) * shift)
    }
}
