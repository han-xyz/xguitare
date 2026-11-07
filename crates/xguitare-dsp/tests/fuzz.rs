mod common;

use common::{
    SAMPLE_RATE, bin_mag, db, full_scale_sine_then_noise, noise, peak, rack_with, run, sine,
};
use xguitare_dsp::modules::fuzz::Fuzz;

const SUSTAIN: usize = 0;
const TONE: usize = 1;
const FUNDAMENTAL: f32 = 110.0;
// 4800 samples hold exactly 11 cycles of 110 Hz at 48 kHz.
const WINDOW: usize = 4 * 4800;
const SETTLE: usize = 4800;

fn fuzz_output(sustain: f32, tone: f32, input: &[f32]) -> Vec<f32> {
    let mut rack = rack_with(Box::new(Fuzz::new()));
    rack.module_mut(0).set_param(SUSTAIN, sustain);
    rack.module_mut(0).set_param(TONE, tone);
    run(&mut rack, input)
}

fn sine_output(sustain: f32, amplitude: f32) -> Vec<f32> {
    let input = sine(amplitude, FUNDAMENTAL, SAMPLE_RATE, SETTLE + WINDOW);
    fuzz_output(sustain, 50.0, &input)
}

#[test]
fn heavy_harmonics_at_high_sustain() {
    let output = &sine_output(100.0, 1.0)[SETTLE..];
    let fundamental = bin_mag(output, FUNDAMENTAL, SAMPLE_RATE);
    let third = db(bin_mag(output, 3.0 * FUNDAMENTAL, SAMPLE_RATE) / fundamental);
    let fifth = db(bin_mag(output, 5.0 * FUNDAMENTAL, SAMPLE_RATE) / fundamental);
    assert!(third > -15.0, "third {third} dB");
    assert!(fifth > -30.0, "fifth {fifth} dB");
}

#[test]
fn output_bounded() {
    let output = sine_output(100.0, 1.0);
    assert!(
        peak(&output) <= 10.0_f32.powf(3.0 / 20.0),
        "peak {}",
        peak(&output)
    );
}

#[test]
fn tone_tilts_spectrum() {
    let input = noise(1.0, 2 * SAMPLE_RATE as usize);
    let tilt = |tone: f32| {
        let output = fuzz_output(60.0, tone, &input);
        db(bin_mag(&output, 3000.0, SAMPLE_RATE)) - db(bin_mag(&output, 200.0, SAMPLE_RATE))
    };
    let (dark, bright) = (tilt(0.0), tilt(100.0));
    assert!(
        bright - dark >= 10.0,
        "tone 0 tilt {dark} dB, tone 100 tilt {bright} dB"
    );
}

#[test]
fn near_clean_at_zero_sustain() {
    let output = &sine_output(0.0, 0.05)[SETTLE..];
    let fundamental = bin_mag(output, FUNDAMENTAL, SAMPLE_RATE);
    let harmonic_energy: f32 = (2..=10)
        .map(|h| bin_mag(output, h as f32 * FUNDAMENTAL, SAMPLE_RATE).powi(2))
        .sum();
    let thd = db(harmonic_energy.sqrt() / fundamental);
    assert!(thd < -30.0, "thd {thd} dB");
}

#[test]
fn silence_in_silence_out() {
    let mut rack = rack_with(Box::new(Fuzz::new()));
    let output = run(&mut rack, &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale() {
    let mut rack = rack_with(Box::new(Fuzz::new()));
    let output = run(&mut rack, &full_scale_sine_then_noise());
    assert!(output.iter().all(|s| s.is_finite()));
}

#[test]
fn tone_center_scoops_mids() {
    let input = noise(0.05, 2 * SAMPLE_RATE as usize);
    let output = fuzz_output(0.0, 50.0, &input);
    let response =
        |hz: f32| {
            db(bin_mag(&output[SETTLE..], hz, SAMPLE_RATE)
                / bin_mag(&input[SETTLE..], hz, SAMPLE_RATE))
        };
    let (low, mid, high) = (response(200.0), response(1000.0), response(4000.0));
    assert!(
        mid <= low - 4.0 && mid <= high - 4.0,
        "200 Hz {low} dB, 1 kHz {mid} dB, 4 kHz {high} dB"
    );
}
