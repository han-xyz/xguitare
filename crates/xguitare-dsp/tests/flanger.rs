mod common;

use common::{
    SAMPLE_RATE, bin_mag, db, frames, noise, peak, prepared, rack_after_cabinet, run_rack_stereo,
    sine,
};
use xguitare_dsp::{Flanger, Rack};

const RATE: usize = 0;
const DEPTH: usize = 1;
const FEEDBACK: usize = 2;
const MIX: usize = 3;

fn flanger_rack(settings: &[(usize, f32)]) -> Rack {
    rack_after_cabinet(Box::new(prepared(Flanger::new(), settings)))
}

#[test]
fn mix_zero_is_identity() {
    let mut rack = flanger_rack(&[(MIX, 0.0)]);
    let input = sine(0.5, 220.0, SAMPLE_RATE, frames(1000.0));
    let (left, right) = run_rack_stereo(&mut rack, &input);
    for channel in [&left, &right] {
        let diff: Vec<f32> = channel.iter().zip(&input).map(|(o, i)| o - i).collect();
        assert!(peak(&diff) < 1e-6, "differs by {}", peak(&diff));
    }
}

#[test]
fn feedback_notch_at_expected_frequency() {
    let mut rack = flanger_rack(&[(RATE, 0.05), (DEPTH, 0.0), (FEEDBACK, 80.0), (MIX, 50.0)]);
    let input = noise(0.5, frames(2000.0));
    let (left, _) = run_rack_stereo(&mut rack, &input);
    let last_second = frames(1000.0)..;
    let notch_hz = 1.0 / (2.0 * 0.5e-3);
    let output = bin_mag(&left[last_second.clone()], notch_hz, SAMPLE_RATE);
    let flat = bin_mag(&input[last_second], notch_hz, SAMPLE_RATE);
    let depth_db = db(output / flat);
    assert!(depth_db <= -12.0, "notch {depth_db} dB");
}

#[test]
fn finite_under_noise() {
    let mut rack = flanger_rack(&[]);
    let (left, right) = run_rack_stereo(&mut rack, &noise(1.0, frames(10_000.0)));
    for channel in [&left, &right] {
        assert!(channel.iter().all(|s| s.is_finite()), "non-finite output");
        assert!(peak(channel) < 10.0, "peak {}", peak(channel));
    }
}

#[test]
fn silence_in_silence_out() {
    let mut rack = flanger_rack(&[]);
    let (left, right) = run_rack_stereo(&mut rack, &vec![0.0; frames(1000.0)]);
    assert_eq!(peak(&left), 0.0);
    assert_eq!(peak(&right), 0.0);
}
