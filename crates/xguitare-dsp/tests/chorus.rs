mod common;

use common::{
    SAMPLE_RATE, frames, noise, peak, prepared, rack_after_cabinet, rms, run_rack_stereo, sine,
};
use xguitare_dsp::{Chorus, Rack};

const DEPTH: usize = 1;
const MIX: usize = 2;

fn chorus_rack(settings: &[(usize, f32)]) -> Rack {
    rack_after_cabinet(Box::new(prepared(Chorus::new(), settings)))
}

#[test]
fn mix_zero_is_identity() {
    let mut rack = chorus_rack(&[(MIX, 0.0)]);
    let input = sine(0.5, 220.0, SAMPLE_RATE, frames(1000.0));
    let (left, right) = run_rack_stereo(&mut rack, &input);
    for channel in [&left, &right] {
        let diff: Vec<f32> = channel.iter().zip(&input).map(|(o, i)| o - i).collect();
        assert!(peak(&diff) < 1e-6, "differs by {}", peak(&diff));
    }
}

#[test]
fn depth_makes_stereo_differ() {
    let mut rack = chorus_rack(&[(DEPTH, 50.0), (MIX, 100.0)]);
    let input = sine(0.5, 220.0, SAMPLE_RATE, frames(1000.0));
    let (left, right) = run_rack_stereo(&mut rack, &input);
    let diff: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
    assert!(rms(&diff) > 1e-3, "L-R rms {}", rms(&diff));
}

#[test]
fn finite_under_noise() {
    let mut rack = chorus_rack(&[]);
    let (left, right) = run_rack_stereo(&mut rack, &noise(1.0, frames(10_000.0)));
    for channel in [&left, &right] {
        assert!(channel.iter().all(|s| s.is_finite()), "non-finite output");
        assert!(peak(channel) < 10.0, "peak {}", peak(channel));
    }
}

#[test]
fn silence_in_silence_out() {
    let mut rack = chorus_rack(&[]);
    let (left, right) = run_rack_stereo(&mut rack, &vec![0.0; frames(1000.0)]);
    assert_eq!(peak(&left), 0.0);
    assert_eq!(peak(&right), 0.0);
}
