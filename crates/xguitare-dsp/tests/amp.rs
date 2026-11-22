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
const BASS_FUNDAMENTAL: f32 = 82.4;
const PERIODS: usize = 2400;
const SETTLE: usize = 9600;
const WINDOW: usize = 4 * PERIODS;
const PEAK_BOUNDED_MASTERS: [f32; 3] = [0.0, 50.0, 100.0];
const BURST_FREQ: f32 = 1000.0;

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

// The window nearest WINDOW that holds a whole number of periods, so the bins do not leak.
fn settled(settings: &[(usize, f32)], amplitude: f32, freq: f32) -> Vec<f32> {
    let periods = (WINDOW as f32 * freq / SAMPLE_RATE).round();
    let window = (periods * SAMPLE_RATE / freq).round() as usize;
    let input = sine(amplitude, freq, SAMPLE_RATE, SETTLE + window);
    amp_output(settings, &input)[SETTLE..].to_vec()
}

fn thd_ratio(output: &[f32], fundamental: f32) -> f32 {
    let harmonic_energy: f32 = (2..=10)
        .map(|h| bin_mag(output, h as f32 * fundamental, SAMPLE_RATE).powi(2))
        .sum();
    harmonic_energy.sqrt() / bin_mag(output, fundamental, SAMPLE_RATE)
}

fn is_bass(model: &AmpModel) -> bool {
    model.name.starts_with("bass-")
}

fn test_tone(model: &AmpModel) -> f32 {
    if is_bass(model) {
        BASS_FUNDAMENTAL
    } else {
        FUNDAMENTAL
    }
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
    let pinned = [
        (0, "tweed"),
        (1, "american"),
        (2, "chime"),
        (3, "crunch"),
        (4, "lead"),
        (5, "modern"),
        (6, "bass-valve"),
        (7, "bass-solid"),
    ];
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
        let output = settled(&[(MODEL, number as f32)], linear(-20.0), test_tone(model));
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
    for (number, model) in models().filter(|(_, model)| model.name != "bass-solid") {
        let tone = test_tone(model);
        let at = |gain: f32| {
            let settings = [(MODEL, number as f32), (GAIN, gain), (MASTER, 20.0)];
            db(thd_ratio(&settled(&settings, linear(-12.0), tone), tone))
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
    let thd = thd_ratio(&settled(&settings, linear(-20.0), FUNDAMENTAL), FUNDAMENTAL);
    assert!(thd < 0.05, "thd {:.2} %", 100.0 * thd);
}

#[test]
fn modern_distorts_more_than_american_at_full_gain() {
    let thd_at_full_gain = |name: &str| {
        let settings = [(MODEL, model_number(name)), (GAIN, 100.0), (MASTER, 20.0)];
        db(thd_ratio(
            &settled(&settings, linear(-12.0), FUNDAMENTAL),
            FUNDAMENTAL,
        ))
    };
    let (modern, american) = (thd_at_full_gain("modern"), thd_at_full_gain("american"));
    assert!(
        modern > american,
        "thd modern {modern} dB, american {american} dB"
    );
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
        let low = if is_bass(model) { 60.0 } else { 100.0 };
        let bass = rise(BASS, low);
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
            _ => assert!(middle >= 3.0, "{}: middle {middle} dB", model.name),
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

#[test]
fn amp_master_scales_the_output_over_its_range() {
    for (number, model) in models() {
        let at = |master: f32| {
            let settings = [(MODEL, number as f32), (GAIN, 10.0), (MASTER, master)];
            db(rms(&settled(&settings, linear(-70.0), FUNDAMENTAL)))
        };
        let range = at(100.0) - at(0.0);
        assert!((range - 42.0).abs() <= 3.0, "{}: {range} dB", model.name);
    }
}

#[test]
fn amp_bright_cap_favours_the_highs_at_low_gain() {
    for (number, model) in models() {
        let tilt = |gain: f32| {
            let level_at = |freq: f32| {
                let settings = [(MODEL, number as f32), (GAIN, gain), (MASTER, 20.0)];
                db(bin_mag(
                    &settled(&settings, linear(-70.0), freq),
                    freq,
                    SAMPLE_RATE,
                ))
            };
            level_at(4000.0) - level_at(FUNDAMENTAL)
        };
        let favoured = tilt(10.0) - tilt(100.0);
        if model.bright_hz.is_some() {
            assert!(favoured >= 3.0, "{}: {favoured} dB", model.name);
        } else {
            assert!(favoured.abs() <= 1.0, "{}: {favoured} dB", model.name);
        }
    }
}

#[test]
fn amp_float_parameters_glide_to_their_value() {
    // One period of 220 Hz past the oversampler's delay. A quiet input keeps `gain` out of
    // saturation, and `bass` 0 shortens the tone stack's memory: a snapped `gain` leaves
    // under 20 times the settled level there, a 5 ms glide more than 50 times.
    let start = 16;
    let window = start..start + 218;
    for (number, model) in models() {
        for (idx, from, to) in [
            (LEVEL, 0.0, -60.0),
            (GAIN, 100.0, 0.0),
            (MASTER, 100.0, 0.0),
        ] {
            let input = sine(0.01, FUNDAMENTAL, SAMPLE_RATE, 2 * SETTLE);
            let mut rack = rack_with(Box::new(Amp::new()));
            for (idx, value) in [
                (MODEL, number as f32),
                (LEVEL, 0.0),
                (BASS, 0.0),
                (idx, from),
            ] {
                rack.module_mut(0).set_param(idx, value);
            }
            run(&mut rack, &input[..SETTLE]);
            rack.module_mut(0).set_param(idx, to);
            let after = run(&mut rack, &input[SETTLE..]);
            let first = rms(&after[window.clone()]);
            let last = rms(&after[SETTLE - window.len()..]);
            assert!(
                first > 30.0 * last,
                "{} {}: {first} then {last}",
                model.name,
                SPEC.params[idx].name
            );
        }
    }
}

#[test]
fn amp_model_change_leaves_nothing_of_the_old_model() {
    const CHANGE: usize = 188 * common::BLOCK;
    let first_10_ms = (0.01 * SAMPLE_RATE) as usize;
    let input = sine(0.1, FUNDAMENTAL, SAMPLE_RATE, CHANGE + first_10_ms);
    let after_change = |from: usize, to: usize| {
        let mut rack = rack_with(Box::new(Amp::new()));
        for (idx, value) in [(MODEL, from as f32), (GAIN, 20.0), (MASTER, 50.0)] {
            rack.module_mut(0).set_param(idx, value);
        }
        run(&mut rack, &input[..CHANGE]);
        rack.module_mut(0).set_param(MODEL, to as f32);
        run(&mut rack, &input[CHANGE..])
    };
    for (to, model) in models() {
        let mut froms = models().map(|(from, _)| from).filter(|&from| from != to);
        let reference = after_change(froms.next().unwrap(), to);
        for from in froms {
            let gap = after_change(from, to)
                .iter()
                .zip(&reference)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(gap < 1e-4, "{} from {from}: {gap}", model.name);
        }
    }
}

#[test]
fn amp_model_change_is_back_to_full_level_after_the_fade() {
    const CHANGE: usize = 188 * common::BLOCK;
    let window = (0.03 * SAMPLE_RATE) as usize..(0.06 * SAMPLE_RATE) as usize;
    let input = sine(0.3, FUNDAMENTAL, SAMPLE_RATE, CHANGE + SETTLE);
    for (from, _) in models() {
        for (to, model) in models().filter(|&(to, _)| to != from) {
            let mut rack = rack_with(Box::new(Amp::new()));
            for (idx, value) in [(MODEL, from as f32), (LEVEL, 0.0)] {
                rack.module_mut(0).set_param(idx, value);
            }
            run(&mut rack, &input[..CHANGE]);
            rack.module_mut(0).set_param(MODEL, to as f32);
            let after = run(&mut rack, &input[CHANGE..]);
            let gap = db(rms(&after[window.clone()])) - db(rms(&after[SETTLE - 2400..]));
            assert!(gap.abs() <= 1.5, "{} from {from}: {gap} dB", model.name);
        }
    }
}

fn quiet_level_at(number: usize, extra: (usize, f32), freq: f32) -> f32 {
    let settings = [(MODEL, number as f32), (GAIN, 10.0), (MASTER, 20.0), extra];
    db(bin_mag(
        &settled(&settings, linear(-50.0), freq),
        freq,
        SAMPLE_RATE,
    ))
}

#[test]
fn amp_bands_follow_their_pots() {
    for (number, model) in models() {
        let Stack::Bands { low, mid, high } = &model.stack else {
            continue;
        };
        let points = [
            (BASS, low, (low.hz / 4.0).max(20.0)),
            (MIDDLE, mid, mid.hz),
            (TREBLE, high, (high.hz * 4.0).min(16_000.0)),
        ];
        for &(idx, band, _) in &points {
            let pot = SPEC.params[idx].name;
            for &(other, _, freq) in &points {
                let flat = quiet_level_at(number, (idx, 50.0), freq);
                let boost = quiet_level_at(number, (idx, 100.0), freq) - flat;
                let cut = quiet_level_at(number, (idx, 0.0), freq) - flat;
                let case = format!("{} {pot} at {freq} Hz: {boost} / {cut} dB", model.name);
                if other == idx {
                    assert!((boost - band.boost_db).abs() <= 1.5, "{case}");
                    assert!((cut + band.cut_db).abs() <= 1.5, "{case}");
                } else {
                    assert!(boost.abs() < 1.5 && cut.abs() < 1.5, "{case}");
                }
            }
        }
    }
}

#[test]
fn amp_bands_are_linear_in_db_between_their_pots() {
    for (number, model) in models() {
        let Stack::Bands { low, mid, high } = &model.stack else {
            continue;
        };
        let points = [
            (BASS, low, (low.hz / 4.0).max(20.0)),
            (MIDDLE, mid, mid.hz),
            (TREBLE, high, (high.hz * 4.0).min(16_000.0)),
        ];
        for &(idx, band, freq) in &points {
            let flat = quiet_level_at(number, (idx, 50.0), freq);
            let upper = quiet_level_at(number, (idx, 75.0), freq) - flat;
            let lower = quiet_level_at(number, (idx, 25.0), freq) - flat;
            let case = format!(
                "{} {} at {freq} Hz: {upper} / {lower} dB",
                model.name, SPEC.params[idx].name
            );
            assert!((upper - band.boost_db / 2.0).abs() <= 1.5, "{case}");
            assert!((lower + band.cut_db / 2.0).abs() <= 1.5, "{case}");
        }
    }
}

#[test]
fn amp_shelves_have_reached_their_gain_at_their_hz() {
    for (number, model) in models() {
        let Stack::Bands { low, high, .. } = &model.stack else {
            continue;
        };
        for (idx, band) in [(BASS, low), (TREBLE, high)] {
            let flat = quiet_level_at(number, (idx, 50.0), band.hz);
            let boost = quiet_level_at(number, (idx, 100.0), band.hz) - flat;
            let cut = quiet_level_at(number, (idx, 0.0), band.hz) - flat;
            let case = format!(
                "{} {}: {boost} / {cut} dB",
                model.name, SPEC.params[idx].name
            );
            assert!(boost >= band.boost_db - 3.0, "{case}");
            assert!(cut <= -band.cut_db + 3.0, "{case}");
        }
    }
}

#[test]
fn amp_band_pots_glide_in_long_blocks() {
    const LONG_BLOCK: usize = 1024;
    const CHANGE: usize = 12 * LONG_BLOCK;
    for (number, model) in models() {
        let Stack::Bands { mid, .. } = &model.stack else {
            continue;
        };
        let input = sine(0.05, mid.hz, SAMPLE_RATE, 4 * CHANGE);
        let rack_at = |pot: f32| {
            let mut rack = rack_with(Box::new(Amp::new()));
            rack.prepare(SAMPLE_RATE, LONG_BLOCK);
            for (idx, value) in [
                (MODEL, number as f32),
                (GAIN, 30.0),
                (MASTER, 30.0),
                (LEVEL, 0.0),
                (MIDDLE, pot),
            ] {
                rack.module_mut(0).set_param(idx, value);
            }
            rack.module_mut(0).reset();
            rack
        };
        let mut gliding = rack_at(0.0);
        run_in_blocks(&mut gliding, &input[..CHANGE], LONG_BLOCK);
        gliding.module_mut(0).set_param(MIDDLE, 100.0);
        let after = run_in_blocks(&mut gliding, &input[CHANGE..], LONG_BLOCK);
        let low = run_in_blocks(&mut rack_at(0.0), &input, LONG_BLOCK);
        let high = run_in_blocks(&mut rack_at(100.0), &input, LONG_BLOCK);
        let early = 16..16 + 48;
        let level = |samples: &[f32]| db(rms(&samples[early.clone()]));
        let travelled = level(&after) - level(&low[CHANGE..]);
        let whole = level(&high[CHANGE..]) - level(&low[CHANGE..]);
        assert!(
            travelled.abs() < 0.5 * whole.abs(),
            "{}: {travelled} dB of {whole} dB after 1 ms",
            model.name
        );
    }
}

// With every band at 50 and `presence` at 0 the `Bands` stack and the presence shelf are exact
// identities, so between `resonance_hz` and a decade above it only the bump differs.
#[test]
fn amp_resonance_bump_follows_feedback() {
    for (number, model) in models() {
        let (Stack::Bands { .. }, Some(hz)) = (&model.stack, model.power.resonance_hz) else {
            continue;
        };
        let bump = quiet_level_at(number, (PRESENCE, 0.0), hz)
            - quiet_level_at(number, (PRESENCE, 0.0), 10.0 * hz);
        let expected = 4.0 * (1.0 - model.power.feedback);
        assert!(
            (bump - expected).abs() <= 0.3,
            "{}: bump {bump} dB, expected {expected}",
            model.name
        );
    }
}

#[test]
fn amp_presence_raises_the_highs() {
    for (number, model) in models() {
        let rise = quiet_level_at(number, (PRESENCE, 100.0), 5000.0)
            - quiet_level_at(number, (PRESENCE, 0.0), 5000.0);
        assert!((3.0..=15.0).contains(&rise), "{}: {rise} dB", model.name);
    }
}

#[test]
fn amp_level_changes_the_output_by_its_value() {
    for (number, model) in models() {
        let reference = quiet_level_at(number, (LEVEL, 0.0), FUNDAMENTAL);
        for level in [-60.0, -12.0, 12.0] {
            let change = quiet_level_at(number, (LEVEL, level), FUNDAMENTAL) - reference;
            assert!(
                (change - level).abs() <= 0.1,
                "{} at level {level}: {change} dB",
                model.name
            );
        }
    }
}

fn burst_output(number: usize, master: f32) -> Vec<f32> {
    let input = sine(linear(-6.0), BURST_FREQ, SAMPLE_RATE, common::frames(300.0));
    amp_output(
        &[(MODEL, number as f32), (GAIN, 30.0), (MASTER, master)],
        &input,
    )
}

#[test]
fn amp_master_lowers_the_crest_factor() {
    for (number, model) in models() {
        let crest = |master: f32| {
            let output = burst_output(number, master);
            db(peak(&output) / rms(&output))
        };
        let (low, high) = (crest(20.0), crest(100.0));
        assert!(
            high < low,
            "{}: crest {low} dB at master 20, {high} dB at master 100",
            model.name
        );
    }
}

#[test]
fn amp_sag_lowers_a_held_note() {
    let sags: Vec<f32> = models().map(|(_, model)| model.power.sag).collect();
    assert!(
        sags.contains(&0.0) && sags.iter().any(|&sag| sag >= 0.3),
        "{sags:?}"
    );
    for (number, model) in models() {
        let output = burst_output(number, 40.0);
        let start = output.iter().position(|s| s.abs() > 1e-3 * peak(&output));
        let output = &output[start.unwrap()..];
        let onset = db(rms(&output[..common::frames(5.0)]));
        let held = db(rms(&output[common::frames(200.0)..common::frames(250.0)]));
        let drop = onset - held;
        let sag = model.power.sag;
        if sag >= 0.3 {
            assert!(drop >= 0.5, "{}: sag {sag}, drop {drop} dB", model.name);
        } else if sag == 0.0 {
            assert!(drop.abs() <= 0.3, "{}: no sag, drop {drop} dB", model.name);
        }
    }
}

// The quiet tone keeps the power amp linear, so any sag left from the loud note shows at full
// size; the 1 kHz bin ignores the low-frequency settling of the stages' high-passes.
#[test]
fn amp_without_sag_recovers_at_once_after_a_loud_note() {
    let loud = common::frames(300.0);
    let mut input = sine(
        linear(-6.0),
        BURST_FREQ,
        SAMPLE_RATE,
        loud + common::frames(600.0),
    );
    for sample in &mut input[loud..] {
        *sample *= linear(-24.0);
    }
    for (number, model) in models().filter(|(_, model)| model.power.sag == 0.0) {
        let output = amp_output(
            &[(MODEL, number as f32), (GAIN, 30.0), (MASTER, 40.0)],
            &input,
        );
        let level_at = |ms: f32| {
            let window = &output[loud + common::frames(ms)..loud + common::frames(ms + 20.0)];
            db(bin_mag(window, BURST_FREQ, SAMPLE_RATE))
        };
        let change = level_at(30.0) - level_at(500.0);
        assert!(change.abs() <= 0.06, "{}: {change} dB", model.name);
    }
}

fn onset_peak(output: &[f32]) -> f32 {
    let start = output.iter().position(|s| s.abs() > 1e-3 * peak(output));
    let start = start.unwrap();
    peak(&output[start..start + common::frames(1.5)])
}

// Over the first 1.5 ms the sag has not yet built up, and the earlier stages are linear at
// -20 dBFS and `gain` 10, so the ratio to the linear prediction is the shaper's alone: a
// `tanh` shaper gives about 0.83 at this drive, a hard clip about 1.
#[test]
fn amp_shaper_knee_hardens_with_feedback() {
    const TARGET_DB: f32 = -3.0;
    const TANH_RATIO: f32 = 0.83;
    const MASTER_DB_PER_PERCENT: f32 = 0.42;
    let input = sine(
        linear(-20.0),
        BURST_FREQ,
        SAMPLE_RATE,
        common::frames(300.0),
    );
    for (number, model) in models() {
        let at = |master: f32| {
            let settings = [
                (MODEL, number as f32),
                (GAIN, 10.0),
                (MASTER, master),
                (LEVEL, 0.0),
            ];
            onset_peak(&amp_output(&settings, &input))
        };
        let master = (TARGET_DB - db(at(0.0))) / MASTER_DB_PER_PERCENT;
        assert!(master <= 100.0, "{}: master {master}", model.name);
        let ratio = at(master) / linear(TARGET_DB);
        let feedback = model.power.feedback;
        let lower = TANH_RATIO + 0.75 * feedback * (1.0 - TANH_RATIO);
        let upper = TANH_RATIO + (feedback + 0.3) * (1.0 - TANH_RATIO);
        assert!(
            (lower..=upper).contains(&ratio),
            "{}: feedback {feedback}, ratio {ratio} outside {lower}..{upper}",
            model.name
        );
    }
}

#[test]
fn amp_power_amp_holds_its_clip_for_a_hot_input() {
    for (number, model) in models() {
        let output_at = |master: f32| {
            let settings = [(MODEL, number as f32), (GAIN, 100.0), (MASTER, master)];
            settled(&settings, 1000.0, FUNDAMENTAL)
        };
        let (half, full) = (output_at(50.0), output_at(100.0));
        let bound = 1.5 * linear(SPEC.params[LEVEL].default);
        assert!(
            db(rms(&full)) >= db(rms(&half)) - 1.0 && peak(&full) <= bound,
            "{}: rms {} at master 50, {} at master 100, peak {}",
            model.name,
            rms(&half),
            rms(&full),
            peak(&full)
        );
    }
}

fn run_in_blocks(rack: &mut xguitare_dsp::Rack, input: &[f32], block: usize) -> Vec<f32> {
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; block];
    for (chunk, out) in input.chunks(block).zip(left.chunks_mut(block)) {
        rack.process(chunk, out, &mut right[..chunk.len()]);
    }
    left
}

#[test]
fn amp_presence_glides_in_long_blocks() {
    const LONG_BLOCK: usize = 1024;
    const CHANGE: usize = 12 * LONG_BLOCK;
    let input = sine(0.05, 2000.0, SAMPLE_RATE, 4 * CHANGE);
    for (number, model) in models() {
        let rack_at = |presence: f32| {
            let mut rack = rack_with(Box::new(Amp::new()));
            rack.prepare(SAMPLE_RATE, LONG_BLOCK);
            for (idx, value) in [
                (MODEL, number as f32),
                (GAIN, 30.0),
                (MASTER, 30.0),
                (LEVEL, 0.0),
                (PRESENCE, presence),
            ] {
                rack.module_mut(0).set_param(idx, value);
            }
            rack.module_mut(0).reset();
            rack
        };
        let mut gliding = rack_at(0.0);
        run_in_blocks(&mut gliding, &input[..CHANGE], LONG_BLOCK);
        gliding.module_mut(0).set_param(PRESENCE, 100.0);
        let after = run_in_blocks(&mut gliding, &input[CHANGE..], LONG_BLOCK);
        let steady = run_in_blocks(&mut rack_at(100.0), &input, LONG_BLOCK);
        let (step, steady_step) = (largest_step(&after), largest_step(&steady[CHANGE..]));
        assert!(
            step <= 1.2 * steady_step,
            "{}: step {step}, steady {steady_step}",
            model.name
        );
    }
}
