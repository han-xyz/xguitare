mod common;

use common::{BLOCK, SAMPLE_RATE};
use xguitare_dsp::smooth::{SMOOTHING_SECONDS, Smoother};
use xguitare_dsp::{AudioBuffers, Gain, Module};

const FRAMES: usize = 4800;

fn gain_output(from_db: f32, to_db: f32) -> Vec<f32> {
    let mut gain = Gain::new();
    gain.set_param(0, from_db);
    gain.prepare(SAMPLE_RATE, BLOCK);
    gain.reset();
    gain.set_param(0, to_db);
    let mut mono = vec![1.0; FRAMES];
    let (mut left, mut right) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    for block in mono.chunks_mut(BLOCK) {
        let frames = block.len();
        gain.process(&mut AudioBuffers {
            frames,
            mono: block,
            left: &mut left,
            right: &mut right,
        });
    }
    mono
}

#[test]
fn parameter_step_ramps_monotonic_bounded_to_target() {
    let smoothing_frames = SMOOTHING_SECONDS * SAMPLE_RATE;
    for (from_db, to_db, from, to) in [(0.0, 20.0, 1.0f32, 10.0f32), (20.0, 0.0, 10.0, 1.0)] {
        let output = gain_output(from_db, to_db);
        let step = to - from;
        let limit = step.abs() / smoothing_frames;
        let mut previous = from;
        for &sample in &output {
            let moved = sample - previous;
            assert!(
                moved * step >= 0.0,
                "{from_db}->{to_db}: ramp reversed at {sample}"
            );
            assert!(
                moved.abs() <= limit,
                "{from_db}->{to_db}: step {moved} above {limit}"
            );
            previous = sample;
        }
        let covered = (output[smoothing_frames as usize - 1] - from) / step;
        assert!(
            (covered - (1.0 - (-1.0f32).exp())).abs() < 0.01,
            "{from_db}->{to_db}: {covered} of the step after {SMOOTHING_SECONDS} s"
        );
        let settled = output[10 * smoothing_frames as usize - 1];
        assert!(
            ((settled - to) / step).abs() < 1e-4,
            "{from_db}->{to_db}: {settled} after ten time constants"
        );
    }
}

#[test]
fn settling_smoother_lands_on_its_target_within_a_bounded_time() {
    const SETTLE_BOUND: usize = 4_000;
    for (from, to) in [(12.0, 0.0), (-15.0, 15.0), (20_000.0, 1_000.0), (0.3, 10.0)] {
        let mut smoother = Smoother::new(from);
        smoother.prepare(SAMPLE_RATE);
        smoother.set_target(to);
        let landed = (0..SETTLE_BOUND)
            .position(|_| smoother.next_value_settling().to_bits() == to.to_bits())
            .unwrap_or_else(|| panic!("{from} -> {to} still moving"));
        assert!(landed > 100, "{from} -> {to} landed after {landed}");
        assert!((0..100).all(|_| smoother.next_value_settling().to_bits() == to.to_bits()));
    }
}
