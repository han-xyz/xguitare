mod common;

use common::{SAMPLE_RATE, bin_mag, db, full_scale_sine_then_noise, peak, rack_with, run, sine};
use xguitare_dsp::Drive;

const DRIVE: usize = 0;
const FUNDAMENTAL: f32 = 220.0;
// 2400 samples hold exactly 11 cycles of 220 Hz at 48 kHz.
const WINDOW: usize = 4 * 2400;
const SETTLE: usize = 4800;

fn drive_output(drive: f32, amplitude: f32) -> Vec<f32> {
    let mut rack = rack_with(Box::new(Drive::new()));
    rack.module_mut(0).set_param(DRIVE, drive);
    let input = sine(amplitude, FUNDAMENTAL, SAMPLE_RATE, SETTLE + WINDOW);
    run(&mut rack, &input)
}

#[test]
fn adds_third_harmonic() {
    let output = &drive_output(100.0, 1.0)[SETTLE..];
    let fundamental = bin_mag(output, FUNDAMENTAL, SAMPLE_RATE);
    let third = bin_mag(output, 3.0 * FUNDAMENTAL, SAMPLE_RATE);
    assert!(
        db(third / fundamental) > -40.0,
        "third {} dB",
        db(third / fundamental)
    );
}

#[test]
fn output_bounded() {
    let output = drive_output(100.0, 1.0);
    assert!(
        peak(&output) <= 10.0_f32.powf(3.0 / 20.0),
        "peak {}",
        peak(&output)
    );
}

#[test]
fn clean_at_zero_drive() {
    let output = &drive_output(0.0, 0.1)[SETTLE..];
    let fundamental = bin_mag(output, FUNDAMENTAL, SAMPLE_RATE);
    let harmonic_energy: f32 = (2..=10)
        .map(|h| bin_mag(output, h as f32 * FUNDAMENTAL, SAMPLE_RATE).powi(2))
        .sum();
    let thd = db(harmonic_energy.sqrt() / fundamental);
    assert!(thd < -40.0, "thd {thd} dB");
}

#[test]
fn silence_in_silence_out() {
    let mut rack = rack_with(Box::new(Drive::new()));
    let output = run(&mut rack, &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale() {
    let mut rack = rack_with(Box::new(Drive::new()));
    let output = run(&mut rack, &full_scale_sine_then_noise());
    assert!(output.iter().all(|s| s.is_finite()));
}
