mod common;

use std::f64::consts::PI;

use common::{SAMPLE_RATE, bin_mag, db, noise, peak, sine};
use xguitare_dsp::svf::{Svf, SvfCoefficients};

const CUTOFF: f32 = 1_000.0;
const Q: f32 = 2.0;
const FREQS: [f32; 10] = [
    50.0, 120.0, 300.0, 600.0, 900.0, 1_100.0, 2_000.0, 4_000.0, 8_000.0, 15_000.0,
];
const SETTLE: usize = 9_600;
const WINDOW: usize = 48_000;

fn measured_db(coefficients: &SvfCoefficients, freq: f32) -> f32 {
    let input = sine(1.0, freq, SAMPLE_RATE, SETTLE + WINDOW);
    let mut svf = Svf::new();
    let output: Vec<f32> = input
        .iter()
        .map(|&x| svf.process(coefficients, x))
        .collect();
    db(
        bin_mag(&output[SETTLE..], freq, SAMPLE_RATE)
            / bin_mag(&input[SETTLE..], freq, SAMPLE_RATE),
    )
}

/// The analog prototype `numerator(s) / (s² + s/Q + 1)` at the bilinear-prewarped frequency of `freq`.
fn analytic_db(freq: f32, numerator: impl Fn(f64) -> (f64, f64)) -> f32 {
    let warp = |hz: f32| (PI * f64::from(hz) / f64::from(SAMPLE_RATE)).tan();
    let w = warp(freq) / warp(CUTOFF);
    let (num_re, num_im) = numerator(w);
    let (den_re, den_im) = (1.0 - w * w, w / f64::from(Q));
    (10.0 * ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).log10())
        as f32
}

fn assert_matches(coefficients: &SvfCoefficients, numerator: impl Fn(f64) -> (f64, f64)) {
    for freq in FREQS {
        let measured = measured_db(coefficients, freq);
        let expected = analytic_db(freq, &numerator);
        assert!(
            (measured - expected).abs() < 0.5,
            "{freq} Hz: measured {measured} dB, analytic {expected} dB"
        );
    }
}

#[test]
fn low_pass_matches_analytic_response() {
    let coefficients = SvfCoefficients::low_pass(CUTOFF, Q, SAMPLE_RATE);
    assert_matches(&coefficients, |_| (1.0, 0.0));
}

#[test]
fn band_pass_matches_analytic_response() {
    let coefficients = SvfCoefficients::band_pass(CUTOFF, Q, SAMPLE_RATE);
    assert_matches(&coefficients, |w| (0.0, w));
}

#[test]
fn high_pass_matches_analytic_response() {
    let coefficients = SvfCoefficients::high_pass(CUTOFF, Q, SAMPLE_RATE);
    assert_matches(&coefficients, |w| (-w * w, 0.0));
}

#[test]
fn bell_gains_its_db_at_cutoff_and_none_far_away() {
    let coefficients = SvfCoefficients::bell(CUTOFF, Q, 9.0, SAMPLE_RATE);
    let at_cutoff = measured_db(&coefficients, CUTOFF);
    let far = measured_db(&coefficients, 50.0);
    assert!((at_cutoff - 9.0).abs() < 0.5, "at cutoff {at_cutoff} dB");
    assert!(far.abs() < 0.5, "at 50 Hz {far} dB");
}

#[test]
fn shelves_gain_their_db_on_their_side_only() {
    let low = SvfCoefficients::low_shelf(CUTOFF, 0.707, -6.0, SAMPLE_RATE);
    let high = SvfCoefficients::high_shelf(CUTOFF, 0.707, 6.0, SAMPLE_RATE);
    let cases = [
        (measured_db(&low, 40.0), -6.0),
        (measured_db(&low, 15_000.0), 0.0),
        (measured_db(&high, 15_000.0), 6.0),
        (measured_db(&high, 40.0), 0.0),
    ];
    for (measured, expected) in cases {
        assert!(
            (measured - expected).abs() < 0.5,
            "measured {measured} dB, expected {expected} dB"
        );
    }
}

#[test]
fn bounded_under_audio_rate_cutoff_modulation() {
    let input = noise(1.0, 2 * WINDOW);
    for (q, mix) in [(0.5, 0), (12.0, 1), (12.0, 2)] {
        let mut svf = Svf::new();
        let output: Vec<f32> = input
            .iter()
            .enumerate()
            .map(|(n, &x)| {
                let phase = 2.0 * PI * 1_000.0 * n as f64 / f64::from(SAMPLE_RATE);
                let octaves = (10_000.0_f32 / 50.0).log2() * (0.5 + 0.5 * phase.sin() as f32);
                let cutoff = 50.0 * octaves.exp2();
                let coefficients = match mix {
                    0 => SvfCoefficients::low_pass(cutoff, q, SAMPLE_RATE),
                    1 => SvfCoefficients::band_pass(cutoff, q, SAMPLE_RATE),
                    _ => SvfCoefficients::high_pass(cutoff, q, SAMPLE_RATE),
                };
                svf.process(&coefficients, x)
            })
            .collect();
        assert!(output.iter().all(|s| s.is_finite()), "q {q} mix {mix}");
        assert!(
            peak(&output) < 4.0 * q,
            "q {q} mix {mix} peak {}",
            peak(&output)
        );
    }
}
