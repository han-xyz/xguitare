use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const LEVEL: usize = 0;
const LOW_CUT: usize = 1;
const HIGH_CUT: usize = 2;
const SMOOTHING_SECONDS: f32 = 0.005;
const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "cabinet",
    channels: Channels::MonoToStereo,
    params: &[
        ParamSpec {
            name: "level",
            min: -20.0,
            max: 20.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
        },
        ParamSpec {
            name: "low_cut",
            min: 20.0,
            max: 300.0,
            default: 20.0,
            unit: Unit::Hz,
            integer: true,
        },
        ParamSpec {
            name: "high_cut",
            min: 2000.0,
            max: 20000.0,
            default: 20000.0,
            unit: Unit::Hz,
            integer: true,
        },
    ],
};

/// A Cabinet IR already resampled to the engine rate and truncated; `right: None` feeds `left` to both outputs.
#[derive(Debug, Clone, PartialEq)]
pub struct CabinetIr {
    pub left: Vec<f32>,
    pub right: Option<Vec<f32>>,
}

impl CabinetIr {
    /// Scales `left` and `right` by one factor so the energy (sum of squares) of `left` is 1.0; an all-zero `left` leaves both unchanged.
    pub fn normalise(&mut self) {
        let energy: f32 = self.left.iter().map(|tap| tap * tap).sum();
        if energy == 0.0 {
            return;
        }
        let factor = 1.0 / energy.sqrt();
        for tap in self.left.iter_mut().chain(self.right.iter_mut().flatten()) {
            *tap *= factor;
        }
    }
}

pub struct Cabinet {
    ir: Option<CabinetIr>,
    convolver: Option<Convolver>,
    level_db: f32,
    target: f32,
    current: f32,
    coeff: f32,
    sample_rate: f64,
    low_cut_hz: f32,
    high_cut_hz: f32,
    high_pass: Cut,
    low_pass: Cut,
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
            sample_rate: 0.0,
            low_cut_hz: SPEC.params[LOW_CUT].default,
            high_cut_hz: SPEC.params[HIGH_CUT].default,
            high_pass: Cut::default(),
            low_pass: Cut::default(),
        }
    }

    // A cut at its default (outermost) value is bypassed: the samples are not touched, so default Presets stay bit-exact.
    fn update_cuts(&mut self) {
        let rate = self.sample_rate;
        let low_cut = f64::from(self.low_cut_hz);
        let high_cut = f64::from(self.high_cut_hz);
        self.high_pass.engage(
            (self.low_cut_hz != SPEC.params[LOW_CUT].default)
                .then(|| Biquad::high_pass(low_cut, rate)),
        );
        self.low_pass.engage(
            (self.high_cut_hz != SPEC.params[HIGH_CUT].default)
                .then(|| Biquad::low_pass(high_cut, rate)),
        );
    }
}

impl Module for Cabinet {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.coeff = (-1.0 / (SMOOTHING_SECONDS * sample_rate)).exp();
        self.sample_rate = f64::from(sample_rate);
        self.update_cuts();
        self.convolver = self.ir.as_ref().map(|ir| Convolver::new(ir, max_frames));
    }

    fn reset(&mut self) {
        self.current = self.target;
        self.high_pass.clear();
        self.low_pass.clear();
        if let Some(convolver) = &mut self.convolver {
            convolver.reset();
        }
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        match idx {
            LEVEL => {
                self.level_db = value;
                self.target = db_to_linear(value);
            }
            LOW_CUT => {
                self.low_cut_hz = value.round();
                self.update_cuts();
            }
            _ => {
                self.high_cut_hz = value.round();
                self.update_cuts();
            }
        }
    }

    fn param(&self, idx: usize) -> f32 {
        match idx {
            LEVEL => self.level_db,
            LOW_CUT => self.low_cut_hz,
            _ => self.high_cut_hz,
        }
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
        self.high_pass.process(left, right);
        self.low_pass.process(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.current = self.target + self.coeff * (self.current - self.target);
            *l *= self.current;
            *r *= self.current;
        }
    }
}

#[derive(Default)]
struct Cut {
    biquad: Option<Biquad>,
    state: [[f64; 2]; 2],
}

impl Cut {
    fn engage(&mut self, biquad: Option<Biquad>) {
        if biquad.is_none() {
            self.clear();
        }
        self.biquad = biquad;
    }

    fn clear(&mut self) {
        self.state = [[0.0; 2]; 2];
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let Some(biquad) = &self.biquad else {
            return;
        };
        let [left_state, right_state] = &mut self.state;
        biquad.process(left, left_state);
        biquad.process(right, right_state);
    }
}

/// RBJ cookbook biquad, normalised so `a0 == 1`, run as transposed direct form II.
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    fn high_pass(freq: f64, rate: f64) -> Biquad {
        let (cos, alpha) = angle(freq, rate);
        Biquad::normalised(
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    fn low_pass(freq: f64, rate: f64) -> Biquad {
        let (cos, alpha) = angle(freq, rate);
        Biquad::normalised(
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    fn normalised(b: [f64; 3], a: [f64; 3]) -> Biquad {
        Biquad {
            b: b.map(|coeff| coeff / a[0]),
            a: [a[1] / a[0], a[2] / a[0]],
        }
    }

    fn process(&self, samples: &mut [f32], state: &mut [f64; 2]) {
        for sample in samples {
            let x = f64::from(*sample);
            let y = self.b[0] * x + state[0];
            state[0] = self.b[1] * x - self.a[0] * y + state[1];
            state[1] = self.b[2] * x - self.a[1] * y;
            *sample = y as f32;
        }
    }
}

fn angle(freq: f64, rate: f64) -> (f64, f64) {
    let w0 = std::f64::consts::TAU * freq / rate;
    (w0.cos(), w0.sin() / (2.0 * BUTTERWORTH_Q))
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
