mod common;

use common::{
    SAMPLE_RATE, bin_mag, db, full_scale_sine_then_noise, peak, rack_with, rms, run, sine,
};
use xguitare_dsp::amp_model::{AmpModel, MAX_STAGES, MODEL_COUNT, MODEL_LABELS, MODELS, Stack};
use xguitare_dsp::modules::amp::SPEC;
use xguitare_dsp::{Amp, Module, Unit};

const MODEL: usize = 0;
const GAIN: usize = 1;
const BASS: usize = 2;
const MIDDLE: usize = 3;
const TREBLE: usize = 4;
const PRESENCE: usize = 5;
const MASTER: usize = 6;
const LEVEL: usize = 7;
const FUNDAMENTAL: f32 = 220.0;
const PERIODS: usize = 2400;
const SETTLE: usize = 9600;
const WINDOW: usize = 4 * PERIODS;
// `master` 100 is left out until the power amp (task 3) bounds the rest.
const PEAK_BOUNDED_MASTERS: [f32; 2] = [0.0, 50.0];

fn models() -> impl Iterator<Item = (usize, &'static AmpModel)> {
    MODELS.iter().enumerate()
}

fn amp_output(settings: &[(usize, f32)], input: &[f32]) -> Vec<f32> {
    let mut rack = rack_with(Box::new(Amp::new()));
    for &(idx, value) in settings {
        rack.module_mut(0).set_param(idx, value);
    }
    rack.module_mut(0).reset();
    run(&mut rack, input)
}

fn settled(settings: &[(usize, f32)], amplitude: f32, freq: f32) -> Vec<f32> {
    let input = sine(amplitude, freq, SAMPLE_RATE, SETTLE + WINDOW);
    amp_output(settings, &input)[SETTLE..].to_vec()
}

fn thd_ratio(output: &[f32]) -> f32 {
    let harmonic_energy: f32 = (2..=10)
        .map(|h| bin_mag(output, h as f32 * FUNDAMENTAL, SAMPLE_RATE).powi(2))
        .sum();
    harmonic_energy.sqrt() / bin_mag(output, FUNDAMENTAL, SAMPLE_RATE)
}

fn linear(dbfs: f32) -> f32 {
    10.0_f32.powf(dbfs / 20.0)
}

fn model_number(name: &str) -> f32 {
    MODELS.iter().position(|model| model.name == name).unwrap() as f32
}

#[test]
fn amp_labels_are_the_table_names() {
    let names: Vec<&str> = MODELS.iter().map(|model| model.name).collect();
    assert_eq!(MODEL_LABELS.as_slice(), names.as_slice());
    let model = &SPEC.params[MODEL];
    assert_eq!(model.labels, MODEL_LABELS.as_slice());
    assert_eq!(model.max, (MODEL_COUNT - 1) as f32);
}

#[test]
fn amp_models_keep_their_numbers() {
    let pinned = [(0, "tweed"), (1, "american")];
    for (number, name) in pinned {
        assert_eq!(MODELS[number].name, name, "model {number}");
    }
}

#[test]
fn amp_models_fit_the_stage_limit() {
    for (_, model) in models() {
        assert!(
            (1..=MAX_STAGES).contains(&model.stages.len()),
            "{}: {} stages",
            model.name,
            model.stages.len()
        );
        assert!(model.stack_after < model.stages.len(), "{}", model.name);
    }
}

#[test]
fn amp_spec_has_the_parameters() {
    assert_eq!(SPEC.type_name, "amp");
    let rows: Vec<(&str, f32, f32, f32, Unit, bool)> = SPEC
        .params
        .iter()
        .map(|p| (p.name, p.min, p.max, p.default, p.unit, p.integer))
        .collect();
    let last_model = (MODEL_COUNT - 1) as f32;
    assert_eq!(
        rows,
        [
            ("model", 0.0, last_model, 0.0, Unit::Linear, true),
            ("gain", 0.0, 100.0, 50.0, Unit::Percent, false),
            ("bass", 0.0, 100.0, 50.0, Unit::Percent, false),
            ("middle", 0.0, 100.0, 50.0, Unit::Percent, false),
            ("treble", 0.0, 100.0, 50.0, Unit::Percent, false),
            ("presence", 0.0, 100.0, 50.0, Unit::Percent, false),
            ("master", 0.0, 100.0, 50.0, Unit::Percent, false),
            ("level", -60.0, 12.0, -12.0, Unit::Db, false),
        ]
    );
}

#[test]
fn amp_clamps_and_rounds_its_parameters() {
    let mut amp = Amp::new();
    for (idx, spec) in SPEC.params.iter().enumerate() {
        assert_eq!(amp.param(idx), spec.default, "{}", spec.name);
        amp.set_param(idx, spec.max + 7.5);
        assert_eq!(amp.param(idx), spec.max, "{}", spec.name);
        amp.set_param(idx, spec.min - 7.5);
        assert_eq!(amp.param(idx), spec.min, "{}", spec.name);
    }
    amp.set_param(MODEL, 0.6);
    assert_eq!(amp.param(MODEL), 1.0);
}

#[test]
fn amp_silence_in_silence_out() {
    for (number, model) in models() {
        let output = amp_output(
            &[
                (MODEL, number as f32),
                (GAIN, 100.0),
                (MASTER, 100.0),
                (LEVEL, 12.0),
            ],
            &vec![0.0; SAMPLE_RATE as usize],
        );
        assert!(
            peak(&output) < linear(-100.0),
            "{}: {}",
            model.name,
            peak(&output)
        );
    }
}

#[test]
fn amp_output_is_bounded_at_every_corner() {
    let corner_params = [GAIN, BASS, MIDDLE, TREBLE, PRESENCE, LEVEL];
    let input = full_scale_sine_then_noise();
    let middle = SAMPLE_RATE as usize;
    let input = &input[middle - PERIODS * 2..middle + PERIODS * 2];
    for (number, model) in models() {
        for master in [0.0, 50.0, 100.0] {
            for corner in 0..1_u32 << corner_params.len() {
                let mut settings = vec![(MODEL, number as f32), (MASTER, master)];
                for (bit, &idx) in corner_params.iter().enumerate() {
                    let spec = &SPEC.params[idx];
                    let value = if corner >> bit & 1 == 1 {
                        spec.max
                    } else {
                        spec.min
                    };
                    settings.push((idx, value));
                }
                let level = settings.last().unwrap().1;
                let output = amp_output(&settings, input);
                let case = format!("{} {settings:?}", model.name);
                assert!(output.iter().all(|s| s.is_finite()), "{case}");
                if PEAK_BOUNDED_MASTERS.contains(&master) {
                    let bound = 1.5 * linear(level);
                    assert!(peak(&output) <= bound, "{case}: peak {}", peak(&output));
                }
            }
        }
    }
}

#[test]
fn amp_defaults_are_in_the_loudness_band() {
    for (number, model) in models() {
        let output = settled(&[(MODEL, number as f32)], linear(-20.0), FUNDAMENTAL);
        let loudness = db(rms(&output));
        assert!(
            (-26.0..=-14.0).contains(&loudness),
            "{}: {loudness} dBFS RMS",
            model.name
        );
    }
}

#[test]
fn amp_gain_raises_distortion() {
    for (number, model) in models() {
        let at = |gain: f32| {
            let settings = [(MODEL, number as f32), (GAIN, gain), (MASTER, 20.0)];
            db(thd_ratio(&settled(&settings, linear(-12.0), FUNDAMENTAL)))
        };
        let (low, high) = (at(10.0), at(100.0));
        assert!(
            high - low >= 10.0,
            "{}: thd {low} dB at gain 10, {high} dB at gain 100",
            model.name
        );
    }
}

#[test]
fn american_is_clean_at_defaults() {
    let settings = [(MODEL, model_number("american"))];
    let thd = thd_ratio(&settled(&settings, linear(-20.0), FUNDAMENTAL));
    assert!(thd < 0.05, "thd {:.2} %", 100.0 * thd);
}

#[test]
fn amp_tone_controls_shape_the_response() {
    let level_at = |number: usize, idx: usize, value: f32, freq: f32| {
        let settings = [
            (MODEL, number as f32),
            (GAIN, 10.0),
            (MASTER, 20.0),
            (idx, value),
        ];
        db(bin_mag(
            &settled(&settings, linear(-50.0), freq),
            freq,
            SAMPLE_RATE,
        ))
    };
    for (number, model) in models() {
        let rise = |idx: usize, freq: f32| {
            level_at(number, idx, 100.0, freq) - level_at(number, idx, 0.0, freq)
        };
        let bass = rise(BASS, 100.0);
        assert!(bass >= 6.0, "{}: bass {bass} dB", model.name);
        let treble = rise(TREBLE, 4000.0);
        assert!(treble >= 6.0, "{}: treble {treble} dB", model.name);
        let middle = rise(MIDDLE, 700.0);
        match model.stack {
            Stack::Fmv {
                middle: Some(_), ..
            } => {
                assert!(
                    middle.abs() <= 0.1,
                    "{}: fixed middle {middle} dB",
                    model.name
                );
            }
            Stack::Fmv { middle: None, .. } => {
                assert!(middle >= 3.0, "{}: middle {middle} dB", model.name);
            }
        }
    }
}

fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

// 2400 frames hold exactly 11 periods of 220 Hz at 48 kHz, so the settled window,
// shifted by a multiple of it, sees the same phase as the window after the change.
#[test]
fn amp_model_change_under_signal() {
    const CHANGE: usize = 188 * common::BLOCK;
    const SETTLED: usize = 5 * PERIODS;
    let first_10_ms = (0.01 * SAMPLE_RATE) as usize;
    let input = sine(0.3, FUNDAMENTAL, SAMPLE_RATE, CHANGE + SETTLED + PERIODS);
    for gain in [20.0, 100.0] {
        for (from, _) in models() {
            for (to, _) in models().filter(|&(to, _)| to != from) {
                let mut rack = rack_with(Box::new(Amp::new()));
                for (idx, value) in [(MODEL, from as f32), (GAIN, gain), (LEVEL, 0.0)] {
                    rack.module_mut(0).set_param(idx, value);
                }
                run(&mut rack, &input[..CHANGE]);
                rack.module_mut(0).set_param(MODEL, to as f32);
                let after = run(&mut rack, &input[CHANGE..]);
                let case = format!("gain {gain}, model {from} -> {to}");
                assert!(after.iter().all(|s| s.is_finite()), "{case}");
                let step = largest_step(&after[..first_10_ms]);
                let settled_step = largest_step(&after[SETTLED..]);
                assert!(
                    step <= 3.0 * settled_step,
                    "{case}: step {step}, settled {settled_step}"
                );
            }
        }
    }
}
