mod common;

use std::f32::consts::TAU;

use common::{SAMPLE_RATE, db, rms, sine};
use xguitare_dsp::oversample::Oversampler4x;

const FRAMES: usize = 4096;
// Each 31-tap stage delays by 15 samples at its own rate: 7.5 + 3.75 base samples up, 3.75 + 7.5 down.
const ROUND_TRIP_DELAY: f32 = 22.5;
const SETTLE: usize = 64;

#[test]
fn round_trip_is_identity() {
    let input = sine(1.0, 1_000.0, SAMPLE_RATE, FRAMES);
    let mut oversampler = Oversampler4x::new();
    oversampler.prepare(FRAMES);
    oversampler.upsample(&input);
    let mut output = vec![0.0; FRAMES];
    oversampler.downsample(&mut output);

    let expected: Vec<f32> = (0..FRAMES)
        .map(|n| (TAU * 1_000.0 * (n as f32 - ROUND_TRIP_DELAY) / SAMPLE_RATE).sin())
        .collect();
    let error: Vec<f32> = output[SETTLE..]
        .iter()
        .zip(&expected[SETTLE..])
        .map(|(o, e)| o - e)
        .collect();
    let error_db = db(rms(&error) / rms(&expected[SETTLE..]));
    assert!(error_db < -60.0, "error {error_db} dB");
}

fn downsampled_rms(freq_at_4x: f32) -> f32 {
    let mut oversampler = Oversampler4x::new();
    oversampler.prepare(FRAMES);
    oversampler.upsample(&[0.0; FRAMES]).copy_from_slice(&sine(
        1.0,
        freq_at_4x,
        4.0 * SAMPLE_RATE,
        4 * FRAMES,
    ));
    let mut output = vec![0.0; FRAMES];
    oversampler.downsample(&mut output);
    rms(&output[SETTLE..])
}

#[test]
fn rejects_alias() {
    let alias_db = db(downsampled_rms(70_000.0) / downsampled_rms(1_000.0));
    assert!(alias_db < -60.0, "alias {alias_db} dB");
}
