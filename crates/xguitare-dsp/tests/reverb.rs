mod common;

use common::{SAMPLE_RATE, frames, noise, prepared, run_stereo, sine};
use xguitare_dsp::modules::reverb::Reverb;

const SIZE: usize = 0;
const DAMPING: usize = 1;
const MIX: usize = 2;
const WIDTH: usize = 3;

#[test]
fn mix_zero_is_identity() {
    let mut reverb = prepared(Reverb::new(), &[(MIX, 0.0)]);
    let input = sine(0.5, 440.0, SAMPLE_RATE, frames(500.0));
    let (mut left, mut right) = (input.clone(), noise(0.5, input.len()));
    let right_input = right.clone();
    run_stereo(&mut reverb, &mut left, &mut right);
    for (out, inp) in left
        .iter()
        .zip(&input)
        .chain(right.iter().zip(&right_input))
    {
        assert!((out - inp).abs() <= 1e-6, "{out} != {inp}");
    }
}

#[test]
fn width_zero_is_mono() {
    let mut reverb = prepared(Reverb::new(), &[(WIDTH, 0.0), (MIX, 100.0)]);
    let mut left = sine(0.5, 440.0, SAMPLE_RATE, frames(500.0));
    let mut right = noise(0.2, left.len());
    run_stereo(&mut reverb, &mut left, &mut right);
    for (l, r) in left.iter().zip(&right) {
        assert!((l - r).abs() <= 1e-6, "{l} != {r}");
    }
}

#[test]
fn no_nan_under_noise() {
    let mut reverb = prepared(Reverb::new(), &[(SIZE, 100.0), (DAMPING, 0.0)]);
    let mut left = noise(1.0, frames(10_000.0));
    let mut right = left.iter().rev().copied().collect::<Vec<_>>();
    run_stereo(&mut reverb, &mut left, &mut right);
    for sample in left.iter().chain(&right) {
        assert!(sample.is_finite(), "non-finite output");
        assert!(sample.abs() < 10.0, "output {sample}");
    }
}
