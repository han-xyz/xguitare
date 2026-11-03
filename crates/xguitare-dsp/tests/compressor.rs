mod common;

use common::{SAMPLE_RATE, db, frames, full_scale_sine_then_noise, peak, rack_with, run, sine};
use xguitare_dsp::{Compressor, Rack};

const THRESHOLD: usize = 0;
const RATIO: usize = 1;
const ATTACK: usize = 2;
const RELEASE: usize = 3;
const MAKEUP: usize = 4;
const TONE_HZ: f32 = 1_000.0;

fn compressor_rack(settings: &[(usize, f32)]) -> Rack {
    let mut rack = rack_with(Box::new(Compressor::new()));
    for &(idx, value) in settings {
        rack.module_mut(0).set_param(idx, value);
    }
    rack
}

fn tone(level_db: f32, ms: f32) -> Vec<f32> {
    sine(
        10.0_f32.powf(level_db / 20.0),
        TONE_HZ,
        SAMPLE_RATE,
        frames(ms),
    )
}

fn reduction_db(input: &[f32], output: &[f32], from_ms: f32) -> f32 {
    let window = frames(from_ms)..frames(from_ms + 1.0);
    db(peak(&input[window.clone()])) - db(peak(&output[window]))
}

#[test]
fn reduces_above_threshold_by_ratio() {
    let mut rack = compressor_rack(&[
        (THRESHOLD, -20.0),
        (RATIO, 4.0),
        (ATTACK, 0.1),
        (RELEASE, 1000.0),
        (MAKEUP, 0.0),
    ]);
    let output = run(&mut rack, &tone(-8.0, 400.0));
    let level = db(peak(&output[frames(300.0)..]));
    let expected = -8.0 - 12.0 * (1.0 - 1.0 / 4.0);
    assert!((level - expected).abs() < 1.0, "level {level} dBFS");
}

#[test]
fn unity_below_threshold() {
    let mut rack = rack_with(Box::new(Compressor::new()));
    let input = tone(-30.0, 400.0);
    let output = run(&mut rack, &input);
    let settled = frames(300.0);
    let difference = db(peak(&output[settled..]) / peak(&input[settled..]));
    assert!(difference.abs() < 0.1, "difference {difference} dB");
}

#[test]
fn attack_and_release_timing() {
    let release_ms = 100.0;
    let mut rack = compressor_rack(&[
        (THRESHOLD, -20.0),
        (RATIO, 4.0),
        (ATTACK, 10.0),
        (RELEASE, release_ms),
    ]);
    let quiet_ms = 200.0;
    let loud_ms = 300.0;
    let mut input = tone(-40.0, quiet_ms);
    input.extend(tone(-8.0, loud_ms));
    input.extend(tone(-40.0, 5.0 * release_ms));
    let output = run(&mut rack, &input);

    let early_reduction = reduction_db(&input, &output, quiet_ms + 29.0);
    assert!(
        early_reduction >= 8.0,
        "reduction {early_reduction} dB at 30 ms"
    );

    let recovered = reduction_db(&input, &output, quiet_ms + loud_ms + 4.0 * release_ms - 1.0);
    assert!(
        recovered.abs() < 0.5,
        "reduction {recovered} dB after 4x release"
    );
}

#[test]
fn silence_in_silence_out() {
    let mut rack = rack_with(Box::new(Compressor::new()));
    let output = run(&mut rack, &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale() {
    let mut rack = rack_with(Box::new(Compressor::new()));
    let output = run(&mut rack, &full_scale_sine_then_noise());
    assert!(output.iter().all(|s| s.is_finite()));
}
