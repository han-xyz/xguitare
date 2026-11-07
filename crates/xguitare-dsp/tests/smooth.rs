mod common;

use common::{BLOCK, SAMPLE_RATE};
use xguitare_dsp::smooth::SMOOTHING_SECONDS;
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
