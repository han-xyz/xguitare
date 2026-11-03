use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const LEVEL: usize = 0;
const SMOOTHING_SECONDS: f32 = 0.005;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "cabinet",
    channels: Channels::MonoToStereo,
    params: &[ParamSpec {
        name: "level",
        min: -20.0,
        max: 20.0,
        default: 0.0,
        unit: Unit::Db,
        integer: false,
    }],
};

/// A Cabinet IR already resampled to the engine rate and truncated; `right: None` feeds `left` to both outputs.
#[derive(Debug, Clone, PartialEq)]
pub struct CabinetIr {
    pub left: Vec<f32>,
    pub right: Option<Vec<f32>>,
}

pub struct Cabinet {
    ir: Option<CabinetIr>,
    convolver: Option<Convolver>,
    level_db: f32,
    target: f32,
    current: f32,
    coeff: f32,
}

impl Cabinet {
    /// `None` builds a passthrough cabinet: mono copied to both outputs, level still applied.
    pub fn new(ir: Option<CabinetIr>) -> Cabinet {
        let level_db = SPEC.params[LEVEL].default;
        Cabinet {
            ir,
            convolver: None,
            level_db,
            target: db_to_linear(level_db),
            current: db_to_linear(level_db),
            coeff: 0.0,
        }
    }
}

impl Module for Cabinet {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.coeff = (-1.0 / (SMOOTHING_SECONDS * sample_rate)).exp();
        self.convolver = self.ir.as_ref().map(|ir| Convolver::new(ir, max_frames));
    }

    fn reset(&mut self) {
        self.current = self.target;
        if let Some(convolver) = &mut self.convolver {
            convolver.reset();
        }
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        debug_assert!(idx == LEVEL, "cabinet has no parameter {idx}");
        let spec = &SPEC.params[LEVEL];
        self.level_db = value.clamp(spec.min, spec.max);
        self.target = db_to_linear(self.level_db);
    }

    fn param(&self, idx: usize) -> f32 {
        debug_assert!(idx == LEVEL, "cabinet has no parameter {idx}");
        self.level_db
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let frames = io.frames;
        let (left, right) = (&mut io.left[..frames], &mut io.right[..frames]);
        match &mut self.convolver {
            Some(convolver) => convolver.process(&io.mono[..frames], left, right),
            None => {
                left.copy_from_slice(&io.mono[..frames]);
                right.copy_from_slice(&io.mono[..frames]);
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.current = self.target + self.coeff * (self.current - self.target);
            *l *= self.current;
            *r *= self.current;
        }
    }
}

/// Uniformly partitioned overlap-save convolution with partition size `block` and FFT size `2 * block`.
struct Convolver {
    block: usize,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    left: Vec<Vec<Complex<f32>>>,
    right: Option<Vec<Vec<Complex<f32>>>>,
    ring: Vec<Vec<Complex<f32>>>,
    newest: usize,
    fill: usize,
    history: Vec<f32>,
    fft_input: Vec<f32>,
    accumulator: Vec<Complex<f32>>,
    fft_output: Vec<f32>,
    forward_scratch: Vec<Complex<f32>>,
    inverse_scratch: Vec<Complex<f32>>,
}

impl Convolver {
    fn new(ir: &CabinetIr, block: usize) -> Convolver {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(2 * block);
        let inverse = planner.plan_fft_inverse(2 * block);
        let left = partition(&ir.left, block, forward.as_ref());
        let right = ir
            .right
            .as_deref()
            .map(|right| partition(right, block, forward.as_ref()));
        let ring_len = left.len().max(right.as_ref().map_or(0, Vec::len)).max(1);
        Convolver {
            block,
            left,
            right,
            ring: vec![forward.make_output_vec(); ring_len],
            newest: 0,
            fill: 0,
            history: vec![0.0; 2 * block],
            fft_input: forward.make_input_vec(),
            accumulator: forward.make_output_vec(),
            fft_output: inverse.make_output_vec(),
            forward_scratch: forward.make_scratch_vec(),
            inverse_scratch: inverse.make_scratch_vec(),
            forward,
            inverse,
        }
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        for spectrum in &mut self.ring {
            spectrum.fill(Complex::default());
        }
        self.newest = 0;
        self.fill = 0;
    }

    fn process(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        debug_assert!(
            input.len() <= self.block,
            "{} frames exceed the cabinet block {}",
            input.len(),
            self.block
        );
        // A short block fills the current partition in place, unfilled tail zero, so the output stays exact; each segment costs one forward FFT plus one inverse FFT per channel, and a call straddling a partition boundary is two segments.
        let mut done = 0;
        while done < input.len() {
            let frames = (input.len() - done).min(self.block - self.fill);
            let segment = done..done + frames;
            self.process_segment(
                &input[segment.clone()],
                &mut left[segment.clone()],
                &mut right[segment],
            );
            done += frames;
        }
    }

    fn process_segment(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        let start = self.block + self.fill;
        let end = start + input.len();
        self.history[start..end].copy_from_slice(input);
        // realfft uses its input buffer as scratch, so the transform runs on a copy of the history.
        self.fft_input.copy_from_slice(&self.history);
        self.forward
            .process_with_scratch(
                &mut self.fft_input,
                &mut self.ring[self.newest],
                &mut self.forward_scratch,
            )
            .expect("buffers are sized for the planned FFT");
        let scale = 1.0 / (2 * self.block) as f32;

        accumulate(&self.left, &self.ring, self.newest, &mut self.accumulator);
        inverse_transform(
            self.inverse.as_ref(),
            &mut self.accumulator,
            &mut self.fft_output,
            &mut self.inverse_scratch,
        );
        scale_into(&self.fft_output[start..end], scale, left);
        match &self.right {
            Some(spectra) => {
                accumulate(spectra, &self.ring, self.newest, &mut self.accumulator);
                inverse_transform(
                    self.inverse.as_ref(),
                    &mut self.accumulator,
                    &mut self.fft_output,
                    &mut self.inverse_scratch,
                );
                scale_into(&self.fft_output[start..end], scale, right);
            }
            None => right.copy_from_slice(left),
        }

        self.fill += input.len();
        if self.fill == self.block {
            self.history.copy_within(self.block.., 0);
            self.history[self.block..].fill(0.0);
            self.newest = (self.newest + 1) % self.ring.len();
            self.fill = 0;
        }
    }
}

fn partition(ir: &[f32], block: usize, forward: &dyn RealToComplex<f32>) -> Vec<Vec<Complex<f32>>> {
    let mut scratch = forward.make_scratch_vec();
    ir.chunks(block)
        .map(|taps| {
            let mut input = forward.make_input_vec();
            input[..taps.len()].copy_from_slice(taps);
            let mut spectrum = forward.make_output_vec();
            forward
                .process_with_scratch(&mut input, &mut spectrum, &mut scratch)
                .expect("buffers are sized for the planned FFT");
            spectrum
        })
        .collect()
}

fn accumulate(
    spectra: &[Vec<Complex<f32>>],
    ring: &[Vec<Complex<f32>>],
    newest: usize,
    accumulator: &mut [Complex<f32>],
) {
    accumulator.fill(Complex::default());
    for (age, partition) in spectra.iter().enumerate() {
        let input = &ring[(newest + ring.len() - age) % ring.len()];
        for ((sum, x), h) in accumulator.iter_mut().zip(input).zip(partition) {
            *sum += x * h;
        }
    }
}

fn inverse_transform(
    inverse: &dyn ComplexToReal<f32>,
    spectrum: &mut [Complex<f32>],
    output: &mut [f32],
    scratch: &mut [Complex<f32>],
) {
    // realfft rejects a non-zero imaginary part at DC and Nyquist, which a non-finite input sample would produce.
    let last = spectrum.len() - 1;
    spectrum[0].im = 0.0;
    spectrum[last].im = 0.0;
    inverse
        .process_with_scratch(spectrum, output, scratch)
        .expect("buffers are sized for the planned FFT");
}

fn scale_into(source: &[f32], scale: f32, target: &mut [f32]) {
    for (t, s) in target.iter_mut().zip(source) {
        *t = s * scale;
    }
}

fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
