mod common;

use common::{SAMPLE_RATE, db, frames, peak, prepared, run_stereo, sine};
use xguitare_dsp::Module;
use xguitare_dsp::modules::delay::Delay;

const TIME: usize = 0;
const FEEDBACK: usize = 1;
const MIX: usize = 2;
const PINGPONG: usize = 3;

fn impulse_response(settings: &[(usize, f32)], ms: f32) -> (Vec<f32>, Vec<f32>, usize) {
    let mut delay = prepared(Delay::new(), settings);
    let start = frames(200.0);
    let mut left = vec![0.0; start + frames(ms)];
    left[start] = 1.0;
    let mut right = left.clone();
    run_stereo(&mut delay, &mut left, &mut right);
    (left, right, start)
}

fn peak_index(samples: &[f32]) -> usize {
    samples
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .map(|(n, _)| n)
        .unwrap()
}

#[test]
fn echo_arrives_on_time() {
    let (left, right, start) =
        impulse_response(&[(TIME, 100.0), (FEEDBACK, 0.0), (MIX, 100.0)], 150.0);
    let expected = start + frames(100.0);
    for channel in [&left, &right] {
        let found = peak_index(&channel[start + 1..]) + start + 1;
        assert!(
            found.abs_diff(expected) <= 1,
            "echo at {found}, expected {expected}"
        );
    }
}

#[test]
fn feedback_second_echo() {
    let (left, _, start) =
        impulse_response(&[(TIME, 100.0), (FEEDBACK, 50.0), (MIX, 100.0)], 250.0);
    let echo = frames(100.0);
    let first = peak(&left[start + echo - 2..start + echo + 3]);
    let second = peak(&left[start + 2 * echo - 2..start + 2 * echo + 3]);
    let ratio_db = db(second / first);
    assert!((ratio_db + 6.0).abs() <= 0.5, "second echo {ratio_db} dB");
}

#[test]
fn pingpong_alternates() {
    let (left, right, start) = impulse_response(
        &[
            (TIME, 100.0),
            (FEEDBACK, 50.0),
            (MIX, 100.0),
            (PINGPONG, 1.0),
        ],
        250.0,
    );
    let echo = frames(100.0);
    let first = start + echo;
    let second = start + 2 * echo;
    assert!(right[first].abs() > 0.1, "first echo R {}", right[first]);
    assert!(
        db(left[first].abs() / right[first].abs()) < -60.0,
        "first echo L {}",
        left[first]
    );
    assert!(left[second].abs() > 0.05, "second echo L {}", left[second]);
    assert!(
        db(right[second].abs() / left[second].abs()) < -60.0,
        "second echo R {}",
        right[second]
    );
}

#[test]
fn time_change_is_smooth() {
    let mut delay = prepared(Delay::new(), &[(FEEDBACK, 0.0), (MIX, 100.0)]);
    let steady = frames(1000.0) + 27;
    let mut left = sine(0.5, 442.5, SAMPLE_RATE, steady + frames(100.0));
    let mut right = left.clone();
    let (before_l, after_l) = left.split_at_mut(steady);
    let (before_r, after_r) = right.split_at_mut(steady);
    run_stereo(&mut delay, before_l, before_r);
    delay.set_param(TIME, 200.0);
    run_stereo(&mut delay, after_l, after_r);
    for channel in [&left, &right] {
        let jump = channel[steady - 1..]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(jump <= 0.5, "jump {jump}");
    }
}
