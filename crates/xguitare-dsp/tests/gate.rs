mod common;

use common::{SAMPLE_RATE, db, full_scale_sine_then_noise, peak, rack_with, rms, run, sine};
use xguitare_dsp::{Gate, Rack};

const THRESHOLD: usize = 0;
const RELEASE: usize = 2;
const RELEASE_MS: f32 = 100.0;

fn gate_rack(threshold_db: f32) -> Rack {
    let mut rack = rack_with(Box::new(Gate::new()));
    rack.module_mut(0).set_param(THRESHOLD, threshold_db);
    rack.module_mut(0).set_param(RELEASE, RELEASE_MS);
    rack
}

fn frames(ms: f32) -> usize {
    (ms / 1000.0 * SAMPLE_RATE) as usize
}

#[test]
fn closes_below_threshold() {
    let mut rack = gate_rack(-40.0);
    let release = frames(RELEASE_MS);
    let mut input = sine(0.1, 1_000.0, SAMPLE_RATE, frames(50.0));
    input.extend(vec![0.0; 2 * release]);
    let quiet_start = input.len();
    input.extend(sine(0.001, 1_000.0, SAMPLE_RATE, 4 * release));

    let output = run(&mut rack, &input);
    let measured = &output[quiet_start + 2 * release..];
    assert!(db(rms(measured)) < -80.0, "rms {} dBFS", db(rms(measured)));
}

#[test]
fn passes_above_threshold() {
    let mut rack = gate_rack(-40.0);
    let input = sine(0.5, 1_000.0, SAMPLE_RATE, frames(100.0));
    let output = run(&mut rack, &input);
    let settled = frames(10.0);
    let difference = db(peak(&output[settled..]) / peak(&input[settled..]));
    assert!(difference.abs() < 0.1, "difference {difference} dB");
}

#[test]
fn hysteresis_holds_open() {
    let mut rack = gate_rack(-40.0);
    let mut input = sine(0.1, 1_000.0, SAMPLE_RATE, frames(100.0));
    let quiet_start = input.len();
    input.extend(sine(
        10.0_f32.powf(-43.0 / 20.0),
        1_000.0,
        SAMPLE_RATE,
        frames(500.0),
    ));

    let output = run(&mut rack, &input);
    let tail = output.len() - frames(100.0);
    let difference = db(peak(&output[tail..]) / peak(&input[quiet_start..]));
    assert!(difference.abs() < 0.5, "difference {difference} dB");
}

#[test]
fn silence_in_silence_out() {
    let mut rack = rack_with(Box::new(Gate::new()));
    let output = run(&mut rack, &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale() {
    let mut rack = rack_with(Box::new(Gate::new()));
    let output = run(&mut rack, &full_scale_sine_then_noise());
    assert!(output.iter().all(|s| s.is_finite()));
}
