mod common;

use std::f32::consts::FRAC_1_SQRT_2;

use common::{
    BLOCK, SAMPLE_RATE, bin_mag, db, full_scale_sine_then_noise, noise, peak, prepared, sine,
};
use xguitare_dsp::svf::SvfCoefficients;
use xguitare_dsp::{AudioBuffers, Blend, Module};

const BLEND: usize = 0;
const DRIVE: usize = 1;
const LEVEL: usize = 2;
const BASS: usize = 3;
const BASS_FREQ: usize = 4;
const MID: usize = 5;
const MID_FREQ: usize = 6;
const TREBLE: usize = 7;
const PRESENCE: usize = 8;
const CROSSOVER: usize = 9;
const SETTLE: usize = 24_000;
const WINDOW: usize = 48_000;
const AMPLITUDE: f32 = 0.05;
const SHELF_Q: f32 = FRAC_1_SQRT_2;
const MID_Q: f32 = 0.7;
const TREBLE_HZ: f32 = 3_000.0;
/// Drive 0, presence 0, level 0 dB, blend 100 %: the driven path is linear for a quiet input.
const LINEAR_WET: [(usize, f32); 4] = [(DRIVE, 0.0), (PRESENCE, 0.0), (LEVEL, 0.0), (BLEND, 100.0)];

fn run(module: &mut Blend, input: &[f32]) -> Vec<f32> {
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

fn settled_output(settings: &[(usize, f32)], amplitude: f32, freq: f32) -> Vec<f32> {
    let input = sine(amplitude, freq, SAMPLE_RATE, SETTLE + WINDOW);
    run(&mut prepared(Blend::new(), settings), &input).split_off(SETTLE)
}

fn measured_db(settings: &[(usize, f32)], freq: f32) -> f32 {
    let output = settled_output(settings, AMPLITUDE, freq);
    db(bin_mag(&output, freq, SAMPLE_RATE) / AMPLITUDE)
}

fn assert_near(measured: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (measured - expected).abs() < tolerance,
        "{what}: measured {measured} dB, expected {expected} dB"
    );
}

fn with(base: &[(usize, f32)], more: &[(usize, f32)]) -> Vec<(usize, f32)> {
    base.iter().chain(more).copied().collect()
}

const FLAT_FREQS: [f32; 9] = [
    30.0, 60.0, 100.0, 400.0, 1_000.0, 3_000.0, 8_000.0, 12_000.0, 15_000.0,
];

#[test]
fn blend_zero_is_magnitude_flat_whatever_the_driven_path() {
    for driven in [
        vec![],
        vec![(DRIVE, 100.0), (LEVEL, 12.0), (PRESENCE, 100.0)],
    ] {
        let settings = with(&driven, &[(BLEND, 0.0)]);
        for freq in FLAT_FREQS {
            assert_near(
                measured_db(&settings, freq),
                0.0,
                0.1,
                &format!("{driven:?} at {freq} Hz"),
            );
        }
    }
}

#[test]
fn every_blend_is_flat_with_a_linear_driven_path() {
    let freqs = [20.0, 40.0, 80.0, 150.0, 300.0, 1_000.0, 5_000.0, 15_000.0];
    for crossover in [20.0, 150.0, 400.0] {
        for blend in [0.0, 50.0, 100.0] {
            let settings = with(&LINEAR_WET, &[(CROSSOVER, crossover), (BLEND, blend)]);
            for freq in freqs {
                assert_near(
                    measured_db(&settings, freq),
                    0.0,
                    0.5,
                    &format!("crossover {crossover} Hz, blend {blend} % at {freq} Hz"),
                );
            }
        }
    }
}

#[test]
fn blend_zero_equals_the_tone_eq_of_the_input() {
    let (bass, mid, treble) = (6.0, -5.0, 4.0);
    let settings = [
        (BLEND, 0.0),
        (BASS, bass),
        (BASS_FREQ, 0.0),
        (MID, mid),
        (MID_FREQ, 1.0),
        (TREBLE, treble),
    ];
    let tone = [
        SvfCoefficients::low_shelf(40.0, SHELF_Q, bass, SAMPLE_RATE),
        SvfCoefficients::bell(1_000.0, MID_Q, mid, SAMPLE_RATE),
        SvfCoefficients::high_shelf(TREBLE_HZ, SHELF_Q, treble, SAMPLE_RATE),
    ];
    for freq in FLAT_FREQS {
        let expected: f32 = tone.iter().map(|c| c.gain_db(freq, SAMPLE_RATE)).sum();
        assert_near(
            measured_db(&settings, freq),
            expected,
            0.1,
            &format!("{freq} Hz"),
        );
    }
}

#[test]
fn split_sums_flat_with_a_linear_driven_path() {
    for crossover in [20.0, 150.0, 400.0] {
        let settings = with(&LINEAR_WET, &[(CROSSOVER, crossover)]);
        for freq in [
            20.0, 40.0, 100.0, 150.0, 300.0, 400.0, 1_000.0, 5_000.0, 15_000.0, 18_000.0,
        ] {
            assert_near(
                measured_db(&settings, freq),
                0.0,
                0.5,
                &format!("crossover {crossover} Hz at {freq} Hz"),
            );
        }
    }
}

fn thd(settings: &[(usize, f32)], freq: f32) -> f32 {
    let output = settled_output(settings, 0.5, freq);
    let fundamental = bin_mag(&output, freq, SAMPLE_RATE);
    let harmonics: f32 = (2..=10)
        .map(|n| bin_mag(&output, n as f32 * freq, SAMPLE_RATE).powi(2))
        .sum();
    harmonics.sqrt() / fundamental
}

#[test]
fn crossover_keeps_the_lows_clean() {
    let thd_at = |crossover: f32| thd(&[(DRIVE, 100.0), (CROSSOVER, crossover)], 40.0);
    let (darkglass, sansamp) = (thd_at(150.0), thd_at(20.0));
    assert!(
        darkglass < sansamp / 4.0,
        "THD at crossover 150 Hz {darkglass}, at 20 Hz {sansamp}"
    );
}

#[test]
fn tone_controls_move_their_frequency_by_their_gain() {
    let bands = [
        ("bass 40 Hz", vec![(BASS_FREQ, 0.0)], BASS, 10.0),
        ("bass 80 Hz", vec![(BASS_FREQ, 1.0)], BASS, 20.0),
        ("mid 500 Hz", vec![(MID_FREQ, 0.0)], MID, 500.0),
        ("mid 1000 Hz", vec![(MID_FREQ, 1.0)], MID, 1_000.0),
        ("treble", vec![], TREBLE, 12_000.0),
    ];
    for (blend, name, base, gain_idx, freq) in [0.0, 100.0]
        .into_iter()
        .flat_map(|blend| bands.clone().map(|(n, b, i, f)| (blend, n, b, i, f)))
    {
        let base = with(&LINEAR_WET, &with(&base, &[(BLEND, blend)]));
        let flat = measured_db(&base, freq);
        for gain in [-12.0, 12.0] {
            let shift = measured_db(&with(&base, &[(gain_idx, gain)]), freq) - flat;
            assert_near(
                shift,
                gain,
                1.0,
                &format!("blend {blend} {name} {gain} dB at {freq} Hz"),
            );
        }
    }
}

#[test]
fn frequency_choices_switch_the_band() {
    let boost = |idx: usize, choice: f32, gain_idx: usize, freq: f32| {
        measured_db(&[(BLEND, 0.0), (idx, choice), (gain_idx, 12.0)], freq)
    };
    let (at_80, at_40) = (
        boost(BASS_FREQ, 1.0, BASS, 80.0),
        boost(BASS_FREQ, 0.0, BASS, 80.0),
    );
    assert!(at_80 - at_40 > 3.0, "80 Hz boost: {at_80} dB vs {at_40} dB");
    let (at_1000, at_500) = (
        boost(MID_FREQ, 1.0, MID, 2_000.0),
        boost(MID_FREQ, 0.0, MID, 2_000.0),
    );
    assert!(
        at_1000 - at_500 > 3.0,
        "2 kHz with mid 1000 Hz: {at_1000} dB vs 500 Hz: {at_500} dB"
    );
}

#[test]
fn silence_in_silence_out() {
    let output = run(
        &mut prepared(Blend::new(), &[]),
        &vec![0.0; SAMPLE_RATE as usize],
    );
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale_at_the_extremes() {
    let input = full_scale_sine_then_noise();
    let extremes = [
        vec![
            (DRIVE, 100.0),
            (LEVEL, 12.0),
            (BASS, 12.0),
            (BASS_FREQ, 0.0),
            (MID, 12.0),
            (TREBLE, 12.0),
            (PRESENCE, 100.0),
            (CROSSOVER, 400.0),
        ],
        vec![
            (BLEND, 0.0),
            (DRIVE, 0.0),
            (LEVEL, -24.0),
            (BASS, -12.0),
            (MID, -12.0),
            (MID_FREQ, 1.0),
            (TREBLE, -12.0),
            (PRESENCE, 0.0),
            (CROSSOVER, 20.0),
        ],
    ];
    for setting in extremes {
        let output = run(&mut prepared(Blend::new(), &setting), &input);
        assert!(output.iter().all(|s| s.is_finite()), "{setting:?}");
        assert!(peak(&output) < 100.0, "{setting:?} peak {}", peak(&output));
    }
}

fn largest_curvature(samples: &[f32]) -> f32 {
    samples
        .windows(3)
        .fold(0.0_f32, |m, w| m.max((w[0] - 2.0 * w[1] + w[2]).abs()))
}

/// Name, base settings, Parameter, from, to, input amplitude, input frequency.
type JumpCase<'a> = (&'a str, &'a [(usize, f32)], usize, f32, f32, f32, f32);

/// A jump lands on a peak of the input sine; an unsmoothed jump bends the output there far more than the settled output at the new value ever bends.
#[test]
fn every_parameter_jump_is_smoothed() {
    let clipping = [(DRIVE, 100.0), (CROSSOVER, 20.0)];
    let linear = [(DRIVE, 0.0), (CROSSOVER, 20.0)];
    let half_drive = [(DRIVE, 50.0)];
    let cases: [JumpCase; 8] = [
        ("blend", &clipping, BLEND, 0.0, 100.0, 0.5, 50.0),
        ("drive", &linear, DRIVE, 0.0, 100.0, 0.05, 50.0),
        ("level", &linear, LEVEL, -24.0, 12.0, 0.5, 50.0),
        ("bass", &[(BASS_FREQ, 1.0)], BASS, -12.0, 12.0, 0.5, 50.0),
        ("mid", &linear, MID, -12.0, 12.0, 0.2, 500.0),
        ("treble", &linear, TREBLE, -12.0, 12.0, 0.2, 1_500.0),
        ("presence", &linear, PRESENCE, 0.0, 100.0, 0.2, 2_500.0),
        ("crossover", &half_drive, CROSSOVER, 20.0, 400.0, 0.5, 50.0),
    ];
    let mut failures = Vec::new();
    for (name, base, idx, from, to, amplitude, freq) in cases {
        let period = SAMPLE_RATE / freq;
        let jump_at = ((SETTLE as f32 / period).round() * period + period / 4.0) as usize;
        let input = sine(amplitude, freq, SAMPLE_RATE, jump_at + BLOCK * 20);
        let window = jump_at - 2..jump_at + 96;

        let mut module = prepared(Blend::new(), &with(base, &[(idx, from)]));
        let mut jumped = run(&mut module, &input[..jump_at]);
        module.set_param(idx, to);
        jumped.extend(run(&mut module, &input[jump_at..]));
        let settled = run(
            &mut prepared(Blend::new(), &with(base, &[(idx, to)])),
            &input,
        );

        let bound = 2.0 * largest_curvature(&settled[window.clone()]) + 0.01;
        let bend = largest_curvature(&jumped[window]);
        if bend >= bound {
            failures.push(format!("{name}: {bend} >= {bound}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn prepare_clears_the_state() {
    let mut module = prepared(Blend::new(), &[(DRIVE, 100.0), (BASS, 12.0)]);
    run(&mut module, &noise(1.0, SAMPLE_RATE as usize / 10));
    module.prepare(SAMPLE_RATE, BLOCK);
    let output = run(&mut module, &vec![0.0; BLOCK]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn params_follow_the_spec_order_with_their_labels() {
    let module = Blend::new();
    let params = module.spec().params;
    let names: Vec<&str> = params.iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        [
            "blend",
            "drive",
            "level",
            "bass",
            "bass_freq",
            "mid",
            "mid_freq",
            "treble",
            "presence",
            "crossover"
        ]
    );
    assert_eq!(params[BASS_FREQ].labels, ["40 Hz", "80 Hz"]);
    assert_eq!(params[BASS_FREQ].default, 1.0);
    assert_eq!(params[MID_FREQ].labels, ["500 Hz", "1000 Hz"]);
    assert_eq!(params[MID_FREQ].default, 0.0);
}
