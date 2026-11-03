mod common;

use common::{
    SAMPLE_RATE, bin_mag, db, full_scale_sine_then_noise, noise, peak, rack_with, run, sine,
};
use xguitare_dsp::Tonestack;

const BASS: usize = 0;
const MIDDLE: usize = 1;
const TREBLE: usize = 2;
const AMPLITUDE: f32 = 0.5;
const SETTLE: usize = 9600;
const WINDOW: usize = 48_000;

fn tonestack_output(settings: &[(usize, f32)], input: &[f32]) -> Vec<f32> {
    let mut rack = rack_with(Box::new(Tonestack::new()));
    for &(idx, value) in settings {
        rack.module_mut(0).set_param(idx, value);
    }
    run(&mut rack, input)
}

fn response_db(settings: &[(usize, f32)], freq: f32) -> f32 {
    let input = sine(AMPLITUDE, freq, SAMPLE_RATE, SETTLE + WINDOW);
    let output = tonestack_output(settings, &input);
    db(bin_mag(&output[SETTLE..], freq, SAMPLE_RATE) / AMPLITUDE)
}

#[test]
fn flat_at_center() {
    let center = [(BASS, 50.0), (MIDDLE, 50.0), (TREBLE, 50.0)];
    let responses = [100.0, 1_000.0, 5_000.0].map(|freq| response_db(&center, freq));
    let spread = responses.iter().fold(f32::MIN, |m, &r| m.max(r))
        - responses.iter().fold(f32::MAX, |m, &r| m.min(r));
    assert!(
        spread <= 10.0,
        "responses {responses:?} dB at 100 Hz, 1 kHz, 5 kHz"
    );
}

#[test]
fn bass_knob_moves_low_end() {
    let difference = response_db(&[(BASS, 100.0)], 100.0) - response_db(&[(BASS, 0.0)], 100.0);
    assert!(difference >= 6.0, "difference {difference} dB");
}

#[test]
fn treble_knob_moves_top() {
    let difference =
        response_db(&[(TREBLE, 100.0)], 5_000.0) - response_db(&[(TREBLE, 0.0)], 5_000.0);
    assert!(difference >= 6.0, "difference {difference} dB");
}

#[test]
fn middle_knob_moves_mids() {
    let difference = response_db(&[(MIDDLE, 100.0)], 500.0) - response_db(&[(MIDDLE, 0.0)], 500.0);
    assert!(difference >= 3.0, "difference {difference} dB");
}

#[test]
fn stable_under_noise() {
    let input = noise(1.0, 10 * SAMPLE_RATE as usize);
    for corner in 0..8 {
        let knob = |bit: usize| if corner & (1 << bit) == 0 { 0.0 } else { 100.0 };
        let settings = [(BASS, knob(0)), (MIDDLE, knob(1)), (TREBLE, knob(2))];
        let output = tonestack_output(&settings, &input);
        assert!(
            output.iter().all(|s| !s.is_nan()) && peak(&output) < 10.0,
            "corner {settings:?} peak {}",
            peak(&output)
        );
    }
}

#[test]
fn silence_in_silence_out() {
    let output = tonestack_output(&[], &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale() {
    let output = tonestack_output(&[], &full_scale_sine_then_noise());
    assert!(output.iter().all(|s| s.is_finite()));
}
