use std::f64::consts::PI;

const LENGTH_S: f64 = 0.1;
const DECAY_TO_MINUS_60_DB_S: f64 = 0.08;
const LOW_CUT_HZ: f64 = 80.0;
const HIGH_CUT_HZ: f64 = 5_000.0;
const BUTTERWORTH_4_Q: [f64; 2] = [0.541_196_1, 1.306_563];
const RESONANCES_HZ: [f64; 2] = [120.0, 2_500.0];
const RESONANCE_GAIN_DB: f64 = 6.0;
const RESONANCE_Q: f64 = 2.0;
const PEAK_DBFS: f64 = -3.0;
const SEED: u32 = 0x5EED_CAB1;

/// Returns the synthesized default cabinet IR at `sample_rate`: 100 ms mono, peak at −3 dBFS, the same samples on every call.
pub fn default_cabinet(sample_rate: u32) -> Vec<f32> {
    let rate = f64::from(sample_rate);
    let frames = (LENGTH_S * rate).round() as usize;
    // Band-pass 80 Hz..5 kHz as a 4th-order Butterworth high-pass and low-pass, each two RBJ cookbook biquads with the Butterworth pole Qs.
    let mut filters: Vec<Biquad> = BUTTERWORTH_4_Q
        .iter()
        .flat_map(|&q| {
            [
                Biquad::high_pass(LOW_CUT_HZ, q, rate),
                Biquad::low_pass(HIGH_CUT_HZ, q, rate),
            ]
        })
        .collect();
    filters.extend(
        RESONANCES_HZ
            .iter()
            .map(|&freq| Biquad::peaking(freq, RESONANCE_Q, RESONANCE_GAIN_DB, rate)),
    );
    let decay_per_sample = (10f64.powf(-60.0 / 20.0)).ln() / (DECAY_TO_MINUS_60_DB_S * rate);
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
