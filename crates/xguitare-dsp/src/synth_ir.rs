use std::f64::consts::PI;

const BUTTERWORTH_4_Q: [f64; 2] = [0.541_196_1, 1.306_563];
const RESONANCE_GAIN_DB: f64 = 6.0;
const RESONANCE_Q: f64 = 2.0;
const PEAK_DBFS: f64 = -3.0;
const SEED: u32 = 0x5EED_CAB1;

const GUITAR: Shape = Shape {
    length_s: 0.1,
    decay_to_minus_60_db_s: 0.08,
    low_cut_hz: 80.0,
    high_cut_hz: 5_000.0,
    resonances_hz: [120.0, 2_500.0],
};

const BASS: Shape = Shape {
    length_s: 0.12,
    decay_to_minus_60_db_s: 0.1,
    low_cut_hz: 40.0,
    high_cut_hz: 3_000.0,
    resonances_hz: [60.0, 1_200.0],
};

/// Returns the synthesized default cabinet IR at `sample_rate`: 100 ms mono, peak at −3 dBFS, the same samples on every call.
pub fn default_cabinet(sample_rate: u32) -> Vec<f32> {
    synthesize(&GUITAR, sample_rate)
}

/// Returns the synthesized bass cabinet IR at `sample_rate`: 120 ms mono, band-passed 40 Hz..3 kHz, peak at −3 dBFS, the same samples on every call.
pub fn bass_cabinet(sample_rate: u32) -> Vec<f32> {
    synthesize(&BASS, sample_rate)
}

struct Shape {
    length_s: f64,
    decay_to_minus_60_db_s: f64,
    low_cut_hz: f64,
    high_cut_hz: f64,
    resonances_hz: [f64; 2],
}

fn synthesize(shape: &Shape, sample_rate: u32) -> Vec<f32> {
    let rate = f64::from(sample_rate);
    let frames = (shape.length_s * rate).round() as usize;
    // Band-pass as a 4th-order Butterworth high-pass and low-pass, each two RBJ cookbook biquads with the Butterworth pole Qs.
    let mut filters: Vec<Biquad> = BUTTERWORTH_4_Q
        .iter()
        .flat_map(|&q| {
            [
                Biquad::high_pass(shape.low_cut_hz, q, rate),
                Biquad::low_pass(shape.high_cut_hz, q, rate),
            ]
        })
        .collect();
    filters.extend(
        shape
            .resonances_hz
            .iter()
            .map(|&freq| Biquad::peaking(freq, RESONANCE_Q, RESONANCE_GAIN_DB, rate)),
    );
    let decay_per_sample = (10f64.powf(-60.0 / 20.0)).ln() / (shape.decay_to_minus_60_db_s * rate);
    let mut noise = Lcg(SEED);
    let ir: Vec<f64> = (0..frames)
        .map(|n| {
            let excitation = noise.next_bipolar() * (decay_per_sample * n as f64).exp();
            filters
                .iter_mut()
                .fold(excitation, |sample, filter| filter.process(sample))
        })
        .collect();
    let peak = ir.iter().fold(0.0f64, |m, s| m.max(s.abs()));
    let scale = 10f64.powf(PEAK_DBFS / 20.0) / peak;
    ir.iter().map(|s| (s * scale) as f32).collect()
}

struct Lcg(u32);

impl Lcg {
    fn next_bipolar(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(self.0) / f64::from(u32::MAX) * 2.0 - 1.0
    }
}

struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn high_pass(freq: f64, q: f64, rate: f64) -> Biquad {
        let (cos, alpha) = Self::angle(freq, q, rate);
        Self::normalised(
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    fn low_pass(freq: f64, q: f64, rate: f64) -> Biquad {
        let (cos, alpha) = Self::angle(freq, q, rate);
        Self::normalised(
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    fn peaking(freq: f64, q: f64, gain_db: f64, rate: f64) -> Biquad {
        let (cos, alpha) = Self::angle(freq, q, rate);
        let amp = 10f64.powf(gain_db / 40.0);
        Self::normalised(
            [1.0 + alpha * amp, -2.0 * cos, 1.0 - alpha * amp],
            [1.0 + alpha / amp, -2.0 * cos, 1.0 - alpha / amp],
        )
    }

    fn angle(freq: f64, q: f64, rate: f64) -> (f64, f64) {
        let w0 = 2.0 * PI * freq / rate;
        (w0.cos(), w0.sin() / (2.0 * q))
    }

    fn normalised(b: [f64; 3], a: [f64; 3]) -> Biquad {
        Biquad {
            b: b.map(|coeff| coeff / a[0]),
            a: [a[1] / a[0], a[2] / a[0]],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        let output = self.b[0] * input + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [input, self.x[0]];
        self.y = [output, self.y[0]];
        output
    }
}
