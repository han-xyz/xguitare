mod common;

use common::{SAMPLE_RATE, bin_mag, db, full_scale_sine_then_noise, peak, rack_with, run, sine};
use xguitare_dsp::Preamp;

const GAIN: usize = 0;
const BIAS: usize = 1;
const MASTER: usize = 2;
const STAGES: usize = 3;
const FUNDAMENTAL: f32 = 220.0;
const WINDOW: usize = 4 * 2400;
const SETTLE: usize = 4800;

fn preamp_output(settings: &[(usize, f32)], input: &[f32]) -> Vec<f32> {
    let mut rack = rack_with(Box::new(Preamp::new()));
    for &(idx, value) in settings {
        rack.module_mut(0).set_param(idx, value);
    }
    run(&mut rack, input)
}

fn settled_sine_output(settings: &[(usize, f32)], amplitude: f32) -> Vec<f32> {
    let input = sine(amplitude, FUNDAMENTAL, SAMPLE_RATE, SETTLE + WINDOW);
    preamp_output(settings, &input)[SETTLE..].to_vec()
}

fn harmonic_db(output: &[f32], harmonic: f32) -> f32 {
    db(bin_mag(output, harmonic * FUNDAMENTAL, SAMPLE_RATE)
        / bin_mag(output, FUNDAMENTAL, SAMPLE_RATE))
}

fn thd(output: &[f32]) -> f32 {
    let harmonic_energy: f32 = (2..=10)
        .map(|h| bin_mag(output, h as f32 * FUNDAMENTAL, SAMPLE_RATE).powi(2))
        .sum();
    db(harmonic_energy.sqrt() / bin_mag(output, FUNDAMENTAL, SAMPLE_RATE))
}

#[test]
fn output_bounded() {
    let input = sine(1.0, FUNDAMENTAL, SAMPLE_RATE, SETTLE + WINDOW);
    let output = preamp_output(&[(GAIN, 100.0), (BIAS, 0.0), (MASTER, -12.0)], &input);
    assert!(
        peak(&output) <= 10.0_f32.powf(-11.0 / 20.0),
        "peak {}",
        peak(&output)
    );
}

#[test]
fn adds_harmonics() {
    let output = settled_sine_output(&[(GAIN, 100.0)], 1.0);
    let third = harmonic_db(&output, 3.0);
    assert!(third >= -30.0, "third {third} dB");
}

#[test]
fn bias_adds_even_harmonics() {
    let unbiased = harmonic_db(&settled_sine_output(&[(GAIN, 60.0), (BIAS, 0.0)], 1.0), 2.0);
    let biased = harmonic_db(&settled_sine_output(&[(GAIN, 60.0), (BIAS, 0.5)], 1.0), 2.0);
    assert!(
        biased - unbiased >= 10.0,
        "second harmonic {unbiased} dB unbiased, {biased} dB biased"
    );
}

#[test]
fn stages_change_distortion() {
    let one = thd(&settled_sine_output(&[(GAIN, 60.0), (STAGES, 1.0)], 0.01));
    let four = thd(&settled_sine_output(&[(GAIN, 60.0), (STAGES, 4.0)], 0.01));
    assert!(four - one >= 3.0, "thd {one} dB at 1 stage, {four} dB at 4");
}

#[test]
fn clean_at_low_gain() {
    let thd = thd(&settled_sine_output(&[(GAIN, 0.0)], 0.05));
    assert!(thd < -40.0, "thd {thd} dB");
}

#[test]
fn silence_in_silence_out() {
    let output = preamp_output(&[], &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale() {
    let output = preamp_output(&[(GAIN, 100.0)], &full_scale_sine_then_noise());
    assert!(output.iter().all(|s| s.is_finite()));
}
