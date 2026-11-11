//! The Cytomic linear trapezoidal state variable filter (Andrew Simper, "SvfLinearTrapOptimised2"): stable under audio-rate cutoff changes.

use std::f32::consts::PI;

/// One response of the filter: the output is `m0·input + m1·band + m2·low`.
#[derive(Clone, Copy, Debug)]
pub struct SvfCoefficients {
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    m0: f32,
    m1: f32,
    m2: f32,
}

impl SvfCoefficients {
    pub fn low_pass(cutoff_hz: f32, q: f32, sample_rate: f32) -> SvfCoefficients {
        let g = prewarp(cutoff_hz, sample_rate);
        SvfCoefficients::new(g, 1.0 / q, (0.0, 0.0, 1.0))
    }

    /// The gain at the cutoff is `q`, as in the analog band-pass `s / (s² + s/q + 1)`.
    pub fn band_pass(cutoff_hz: f32, q: f32, sample_rate: f32) -> SvfCoefficients {
        let g = prewarp(cutoff_hz, sample_rate);
        SvfCoefficients::new(g, 1.0 / q, (0.0, 1.0, 0.0))
    }

    /// The band-pass scaled to 0 dB at the cutoff for any `q`.
    pub fn band_pass_unity(cutoff_hz: f32, q: f32, sample_rate: f32) -> SvfCoefficients {
        let g = prewarp(cutoff_hz, sample_rate);
        let k = 1.0 / q;
        SvfCoefficients::new(g, k, (0.0, k, 0.0))
    }

    pub fn high_pass(cutoff_hz: f32, q: f32, sample_rate: f32) -> SvfCoefficients {
        let g = prewarp(cutoff_hz, sample_rate);
        let k = 1.0 / q;
        SvfCoefficients::new(g, k, (1.0, -k, -1.0))
    }

    /// Unity gain at every frequency, with the phase of the analog `(s² − s/q + 1) / (s² + s/q + 1)`.
    pub fn all_pass(cutoff_hz: f32, q: f32, sample_rate: f32) -> SvfCoefficients {
        let g = prewarp(cutoff_hz, sample_rate);
        let k = 1.0 / q;
        SvfCoefficients::new(g, k, (1.0, -2.0 * k, 0.0))
    }

    pub fn bell(cutoff_hz: f32, q: f32, gain_db: f32, sample_rate: f32) -> SvfCoefficients {
        let a = shelf_amplitude(gain_db);
        let g = prewarp(cutoff_hz, sample_rate);
        let k = 1.0 / (q * a);
        SvfCoefficients::new(g, k, (1.0, k * (a * a - 1.0), 0.0))
    }

    pub fn low_shelf(cutoff_hz: f32, q: f32, gain_db: f32, sample_rate: f32) -> SvfCoefficients {
        let a = shelf_amplitude(gain_db);
        let g = prewarp(cutoff_hz, sample_rate) / a.sqrt();
        let k = 1.0 / q;
        SvfCoefficients::new(g, k, (1.0, k * (a - 1.0), a * a - 1.0))
    }

    pub fn high_shelf(cutoff_hz: f32, q: f32, gain_db: f32, sample_rate: f32) -> SvfCoefficients {
        let a = shelf_amplitude(gain_db);
        let g = prewarp(cutoff_hz, sample_rate) * a.sqrt();
        let k = 1.0 / q;
        SvfCoefficients::new(g, k, (a * a, k * (1.0 - a) * a, 1.0 - a * a))
    }

    /// The filter's gain at `freq_hz` in dB, from its transfer function rather than by running it.
    pub fn gain_db(&self, freq_hz: f32, sample_rate: f32) -> f32 {
        let w = f64::from(prewarp(freq_hz, sample_rate)) / f64::from(self.g);
        let (k, m0, m1, m2) = (
            f64::from(self.k),
            f64::from(self.m0),
            f64::from(self.m1),
            f64::from(self.m2),
        );
        let (den_re, den_im) = (1.0 - w * w, k * w);
        let (num_re, num_im) = (m0 * den_re + m2, m0 * den_im + m1 * w);
        (10.0 * ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).log10())
            as f32
    }

    fn new(g: f32, k: f32, (m0, m1, m2): (f32, f32, f32)) -> SvfCoefficients {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        SvfCoefficients {
            g,
            k,
            a1,
            a2,
            a3: g * a2,
            m0,
            m1,
            m2,
        }
    }
}

fn prewarp(cutoff_hz: f32, sample_rate: f32) -> f32 {
    (PI * cutoff_hz / sample_rate).tan()
}

/// `A` of the cookbook formulas: the square root of the linear gain.
fn shelf_amplitude(gain_db: f32) -> f32 {
    10.0_f32.powf(gain_db / 40.0)
}

/// The filter's two integrator states; the coefficients come with each sample, so they may change every sample.
#[derive(Debug, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    pub fn new() -> Svf {
        Svf::default()
    }

    pub fn reset(&mut self) {
        *self = Svf::default();
    }

    pub fn process(&mut self, coefficients: &SvfCoefficients, input: f32) -> f32 {
        let c = coefficients;
        let v3 = input - self.ic2;
        let band = c.a1 * self.ic1 + c.a2 * v3;
        let low = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * band - self.ic1;
        self.ic2 = 2.0 * low - self.ic2;
        c.m0 * input + c.m1 * band + c.m2 * low
    }
}
