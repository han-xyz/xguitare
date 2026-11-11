mod common;

use common::{
    BLOCK, SAMPLE_RATE, bin_mag, db, full_scale_sine_then_noise, noise, peak, prepared, sine,
};
use xguitare_dsp::{AudioBuffers, Eq, Module, eq_response_db};

const LOW_CUT: usize = 0;
const LOW_FREQ: usize = 1;
const LOW_GAIN: usize = 2;
const MID1_FREQ: usize = 3;
const MID1_GAIN: usize = 4;
const MID1_Q: usize = 5;
const MID2_FREQ: usize = 6;
const MID2_GAIN: usize = 7;
const MID2_Q: usize = 8;
const HIGH_FREQ: usize = 9;
const HIGH_GAIN: usize = 10;
const HIGH_CUT: usize = 11;
const OUTPUT: usize = 12;
const SETTLE: usize = 9_600;
const WINDOW: usize = 48_000;
const AMPLITUDE: f32 = 0.25;

fn run(module: &mut Eq, input: &[f32]) -> Vec<f32> {
    let mut output = input.to_vec();
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    for block in output.chunks_mut(BLOCK) {
        module.process(&mut AudioBuffers {
            frames: block.len(),
            mono: block,
            left: &mut left,
            right: &mut right,
        });
    }
    output
}

fn measured_db(settings: &[(usize, f32)], freq: f32) -> f32 {
    let input = sine(AMPLITUDE, freq, SAMPLE_RATE, SETTLE + WINDOW);
    let output = run(&mut prepared(Eq::new(), settings), &input);
    db(bin_mag(&output[SETTLE..], freq, SAMPLE_RATE) / AMPLITUDE)
}

fn params(settings: &[(usize, f32)]) -> Vec<f32> {
    let mut values: Vec<f32> = Eq::new().spec().params.iter().map(|p| p.default).collect();
    for &(idx, value) in settings {
        values[idx] = value;
    }
    values
}

fn assert_near(measured: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (measured - expected).abs() < tolerance,
        "{what}: measured {measured} dB, expected {expected} dB"
    );
}

#[test]
fn each_band_moves_its_frequency_by_its_gain() {
    let bands = [
        ("low", vec![(LOW_FREQ, 200.0)], LOW_GAIN, 25.0),
        (
            "mid1",
            vec![(MID1_FREQ, 400.0), (MID1_Q, 2.0)],
            MID1_GAIN,
            400.0,
        ),
        (
            "mid2",
            vec![(MID2_FREQ, 2500.0), (MID2_Q, 2.0)],
            MID2_GAIN,
            2500.0,
        ),
        ("high", vec![(HIGH_FREQ, 1500.0)], HIGH_GAIN, 12_000.0),
    ];
    for (name, base, gain_idx, freq) in bands {
        let flat = measured_db(&base, freq);
        for gain in [-12.0, 9.0] {
            let mut moved = base.clone();
            moved.push((gain_idx, gain));
            let shift = measured_db(&moved, freq) - flat;
            assert_near(shift, gain, 0.5, &format!("{name} {gain} dB at {freq} Hz"));
        }
    }
}

#[test]
fn output_shifts_every_frequency() {
    for freq in [100.0, 1_000.0, 5_000.0] {
        let shift = measured_db(&[(OUTPUT, -7.0)], freq) - measured_db(&[], freq);
        assert_near(shift, -7.0, 0.1, &format!("{freq} Hz"));
    }
}

#[test]
fn cuts_are_three_db_down_at_their_cutoff() {
    assert_near(
        measured_db(&[(LOW_CUT, 100.0)], 100.0),
        -3.0,
        0.5,
        "low cut",
    );
    assert_near(
        measured_db(&[(HIGH_CUT, 5_000.0)], 5_000.0),
        -3.0,
        0.5,
        "high cut",
    );
}

#[test]
fn defaults_are_flat() {
    for freq in [
        30.0, 60.0, 100.0, 400.0, 1_000.0, 2_500.0, 8_000.0, 14_000.0,
    ] {
        assert_near(measured_db(&[], freq), 0.0, 0.1, &format!("{freq} Hz"));
    }
    assert_near(measured_db(&[], 15_000.0), 0.0, 0.15, "15000 Hz");
}

#[test]
fn response_fn_equals_measured_sine_gain() {
    let settings = [
        vec![
            (LOW_CUT, 40.0),
            (LOW_GAIN, 4.0),
            (MID1_GAIN, -3.0),
            (MID2_GAIN, 2.0),
            (HIGH_CUT, 12_000.0),
        ],
        vec![
            (LOW_CUT, 80.0),
            (LOW_FREQ, 150.0),
            (LOW_GAIN, -9.0),
            (MID1_FREQ, 700.0),
            (MID1_GAIN, 12.0),
            (MID1_Q, 4.0),
            (MID2_FREQ, 3_000.0),
            (MID2_GAIN, -15.0),
            (MID2_Q, 0.5),
            (HIGH_FREQ, 6_000.0),
            (HIGH_GAIN, 8.0),
            (HIGH_CUT, 9_000.0),
            (OUTPUT, 5.0),
        ],
    ];
    let freqs = [
        35.0, 90.0, 160.0, 400.0, 700.0, 1_200.0, 3_000.0, 6_000.0, 9_000.0, 14_000.0,
    ];
    for setting in settings {
        let values = params(&setting);
        for freq in freqs {
            let analytic = eq_response_db(&values, freq, SAMPLE_RATE);
            assert_near(
                measured_db(&setting, freq),
                analytic,
                0.5,
                &format!("{setting:?} at {freq} Hz"),
            );
        }
    }
}

#[test]
fn silence_in_silence_out() {
    let output = run(&mut Eq::new(), &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale_at_the_extremes() {
    let input = full_scale_sine_then_noise();
    let extremes = [
        vec![
            (LOW_GAIN, 15.0),
            (MID1_GAIN, 15.0),
            (MID1_Q, 10.0),
            (MID2_GAIN, 15.0),
            (MID2_Q, 10.0),
            (HIGH_GAIN, 15.0),
            (OUTPUT, 12.0),
        ],
        vec![
            (LOW_CUT, 300.0),
            (LOW_GAIN, -15.0),
            (MID1_FREQ, 40.0),
            (MID1_Q, 0.3),
            (MID2_FREQ, 10_000.0),
            (HIGH_FREQ, 20_000.0),
            (HIGH_CUT, 1_000.0),
            (OUTPUT, -12.0),
        ],
    ];
    for setting in extremes {
        let output = run(&mut prepared(Eq::new(), &setting), &input);
        assert!(output.iter().all(|s| s.is_finite()), "{setting:?}");
        assert!(peak(&output) < 200.0, "{setting:?} peak {}", peak(&output));
    }
}

#[test]
fn parameter_jump_is_smoothed() {
    let jump_at = SETTLE + SAMPLE_RATE as usize / 50 / 4;
    let input = sine(0.5, 50.0, SAMPLE_RATE, jump_at + BLOCK * 20);
    for (idx, from, to) in [(OUTPUT, -12.0, 12.0), (MID1_GAIN, -15.0, 15.0)] {
        let mut module = prepared(Eq::new(), &[(MID1_FREQ, 50.0), (idx, from)]);
        let mut output = run(&mut module, &input[..jump_at]).split_off(jump_at - 1);
        module.set_param(idx, to);
        output.extend(run(&mut module, &input[jump_at..]));
        let largest_step = output
            .windows(2)
            .fold(0.0_f32, |m, pair| m.max((pair[1] - pair[0]).abs()));
        assert!(largest_step < 0.05, "param {idx}: step {largest_step}");
    }
}

#[test]
fn prepare_clears_the_filters() {
    let mut module = prepared(Eq::new(), &[(MID1_GAIN, 12.0), (MID1_Q, 10.0)]);
    run(&mut module, &noise(1.0, SAMPLE_RATE as usize / 10));
    module.prepare(SAMPLE_RATE, BLOCK);
    let output = run(&mut module, &vec![0.0; BLOCK]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn param_names_follow_the_spec_order() {
    let names: Vec<&str> = Eq::new().spec().params.iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        [
            "low_cut",
            "low_freq",
            "low_gain",
            "mid1_freq",
            "mid1_gain",
            "mid1_q",
            "mid2_freq",
            "mid2_gain",
            "mid2_q",
            "high_freq",
            "high_gain",
            "high_cut",
            "output"
        ]
    );
}
