use std::f32::consts::TAU;

use xguitare_dsp::resample::resample_offline;

const EDGE: usize = 64;

#[test]
fn identity_when_equal() {
    let input = [0.1, -0.4, 0.9, 0.0, 0.3];
    assert_eq!(resample_offline(&input, 48_000, 48_000), input);
}

#[test]
fn sine_44100_to_48000_keeps_peak_and_frequency() {
    let input: Vec<f32> = (0..44_100)
        .map(|n| (TAU * 1000.0 * n as f32 / 44_100.0).sin())
        .collect();
    let output = resample_offline(&input, 44_100, 48_000);
    assert_eq!(output.len(), 48_000);
    let interior = &output[EDGE..output.len() - EDGE];
    let peak = interior.iter().fold(0.0_f32, |max, s| max.max(s.abs()));
    assert!(20.0 * peak.log10().abs() <= 0.1, "peak {peak}");
    let crossings = interior
        .windows(2)
        .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
        .count();
    let seconds = interior.len() as f32 / 48_000.0;
    let frequency = crossings as f32 / 2.0 / seconds;
    assert!((frequency - 1000.0).abs() <= 1.0, "frequency {frequency}");
}

#[test]
fn dc_stays_flat() {
    let output = resample_offline(&[0.5; 1000], 44_100, 48_000);
    for (n, sample) in output[EDGE..output.len() - EDGE].iter().enumerate() {
        assert!((sample - 0.5).abs() <= 1e-3, "sample {n}: {sample}");
    }
}
