mod common;

use common::{BLOCK, SAMPLE_RATE, full_scale_sine_then_noise, noise, peak, prepared, rms};
use xguitare_dsp::{AudioBuffers, Autowah, Module};

const MODE: usize = 0;
const DIRECTION: usize = 1;
const FREQ_LOW: usize = 5;
const FREQ_HIGH: usize = 6;
const DRY: usize = 8;
const WET: usize = 9;
const BAND_PASS: f32 = 1.0;
const DOWN: f32 = 1.0;
const QUIET: f32 = 0.02;
const LOUD: f32 = 1.0;

fn run(module: &mut Autowah, input: &[f32]) -> Vec<f32> {
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

fn wet_only(settings: &[(usize, f32)]) -> Autowah {
    let mut all = vec![(MODE, BAND_PASS), (DRY, 0.0), (WET, 100.0)];
    all.extend_from_slice(settings);
    prepared(Autowah::new(), &all)
}

/// How much of a signal's energy sits high in the spectrum: the RMS of its first difference over its RMS, which grows with the frequency of the energy.
fn brightness(signal: &[f32]) -> f32 {
    let difference: Vec<f32> = signal.windows(2).map(|pair| pair[1] - pair[0]).collect();
    rms(&difference) / rms(signal)
}

fn brightness_at(level: f32, settings: &[(usize, f32)]) -> f32 {
    let input = noise(level, SAMPLE_RATE as usize);
    let output = run(&mut wet_only(settings), &input);
    brightness(&output[SAMPLE_RATE as usize / 4..])
}

#[test]
fn louder_input_sweeps_cutoff_up() {
    let (quiet, loud) = (brightness_at(QUIET, &[]), brightness_at(LOUD, &[]));
    assert!(loud > 1.5 * quiet, "quiet {quiet}, loud {loud}");
}

#[test]
fn down_mirrors_up() {
    let down = [(DIRECTION, DOWN)];
    let (up_quiet, up_loud) = (brightness_at(QUIET, &[]), brightness_at(LOUD, &[]));
    let (down_quiet, down_loud) = (brightness_at(QUIET, &down), brightness_at(LOUD, &down));
    assert!(
        down_loud < down_quiet / 1.5,
        "down: quiet {down_quiet}, loud {down_loud}"
    );
    assert!(
        down_quiet > 1.5 * up_quiet,
        "quiet: up {up_quiet}, down {down_quiet}"
    );
    assert!(
        up_loud > 1.5 * down_loud,
        "loud: up {up_loud}, down {down_loud}"
    );
}

#[test]
fn freq_high_below_freq_low_swaps() {
    let input = noise(0.5, SAMPLE_RATE as usize / 2);
    let swapped = run(
        &mut wet_only(&[(FREQ_LOW, 800.0), (FREQ_HIGH, 400.0)]),
        &input,
    );
    let ordered = run(
        &mut wet_only(&[(FREQ_LOW, 400.0), (FREQ_HIGH, 800.0)]),
        &input,
    );
    let difference = swapped
        .iter()
        .zip(&ordered)
        .fold(0.0_f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(difference < 1e-5, "difference {difference}");
}

#[test]
fn dry_and_wet_mix() {
    let input = noise(0.5, SAMPLE_RATE as usize / 2);
    let dry_only = run(
        &mut prepared(Autowah::new(), &[(DRY, 100.0), (WET, 0.0)]),
        &input,
    );
    assert_eq!(dry_only, input);
    let wet = run(
        &mut prepared(Autowah::new(), &[(DRY, 0.0), (WET, 100.0)]),
        &input,
    );
    let mixed = run(
        &mut prepared(Autowah::new(), &[(DRY, 50.0), (WET, 100.0)]),
        &input,
    );
    let difference = mixed
        .iter()
        .zip(&wet)
        .zip(&input)
        .fold(0.0_f32, |m, ((mix, w), x)| m.max((mix - w - 0.5 * x).abs()));
    assert!(difference < 1e-5, "difference {difference}");
}

#[test]
fn silence_in_silence_out() {
    let output = run(&mut Autowah::new(), &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale_at_every_extreme() {
    let input = full_scale_sine_then_noise();
    for (mode, q, sensitivity) in [(0.0, 12.0, 36.0), (1.0, 12.0, 36.0), (2.0, 0.5, -24.0)] {
        let mut module = prepared(Autowah::new(), &[(MODE, mode), (7, q), (2, sensitivity)]);
        let output = run(&mut module, &input);
        assert!(output.iter().all(|s| s.is_finite()), "mode {mode}");
        assert!(peak(&output) < 50.0, "mode {mode} peak {}", peak(&output));
    }
}

#[test]
fn parameter_jump_is_smoothed() {
    let mut module = prepared(Autowah::new(), &[(DRY, 0.0), (WET, 0.0)]);
    module.set_param(DRY, 100.0);
    let output = run(&mut module, &vec![0.5; BLOCK * 20]);
    let largest_step = output
        .windows(2)
        .fold(output[0].abs(), |m, pair| m.max((pair[1] - pair[0]).abs()));
    assert!(largest_step < 0.01, "step {largest_step}");
    assert!((output[output.len() - 1] - 0.5).abs() < 0.01);
}

#[test]
fn prepare_clears_filter_and_envelope() {
    let mut module = prepared(Autowah::new(), &[]);
    run(&mut module, &noise(1.0, SAMPLE_RATE as usize / 10));
    module.prepare(SAMPLE_RATE, BLOCK);
    let output = run(&mut module, &vec![0.0; BLOCK]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn choice_params_carry_labels() {
    let params = Autowah::new().spec().params;
    let labelled: Vec<(&str, &[&str])> = params
        .iter()
        .filter(|param| !param.labels.is_empty())
        .map(|param| (param.name, param.labels))
        .collect();
    assert_eq!(
        labelled,
        [
            ("mode", &["lp", "bp", "hp"][..]),
            ("direction", &["up", "down"][..])
        ]
    );
}
