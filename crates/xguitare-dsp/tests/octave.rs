mod common;

use std::f64::consts::TAU;

use common::{
    BLOCK, SAMPLE_RATE, bin_mag, db, full_scale_sine_then_noise, noise, peak, prepared, rms, sine,
};
use xguitare_dsp::svf::{Svf, SvfCoefficients};
use xguitare_dsp::{AudioBuffers, Module, Octave};

const DRY: usize = 0;
const OCT1: usize = 1;
const OCT2: usize = 2;
const UP: usize = 3;
const TONE: usize = 4;
const RANGE: usize = 5;
const BASS: f32 = 0.0;
const GUITAR: f32 = 1.0;
const RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];
const BASS_NOTES: [f32; 7] = [31.0, 41.0, 55.0, 73.0, 98.0, 196.0, 392.0];
const GUITAR_NOTES: [f32; 8] = [82.0, 110.0, 147.0, 196.0, 247.0, 330.0, 659.0, 988.0];
const NOTE_SECONDS: f64 = 1.6;
const FIRST_PEAK_DB: f64 = -12.0;
const MEASURE_FROM_SECONDS: f64 = 0.1;
const MEASURE_TO_SECONDS: f64 = 1.5;
const FRAME_PERIODS: f32 = 4.0;
const CANDIDATES: [f32; 7] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

fn run(module: &mut Octave, input: &[f32]) -> Vec<f32> {
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

fn octave_at(rate: f32, settings: &[(usize, f32)]) -> Octave {
    let mut module = Octave::new();
    for &(idx, value) in settings {
        module.set_param(idx, value);
    }
    module.prepare(rate, BLOCK);
    module.reset();
    module
}

fn voice_alone(voice: usize, settings: &[(usize, f32)]) -> Vec<(usize, f32)> {
    let mut all = vec![
        (DRY, 0.0),
        (OCT1, 0.0),
        (OCT2, 0.0),
        (UP, 0.0),
        (voice, 100.0),
    ];
    all.extend_from_slice(settings);
    all
}

/// The R3 note: fundamental with 2f at 0.8 and 3f at 0.4, sine phase, first peak at −12 dBFS, decaying 20 dB per second.
fn plucked(freq: f32, rate: f32) -> Vec<f32> {
    shaped_note(freq, rate, FIRST_PEAK_DB, 0.8, 0.0, 0.0)
}

fn shaped_note(
    freq: f32,
    rate: f32,
    first_peak_db: f64,
    second: f64,
    phase_shift: f64,
    second_phase: f64,
) -> Vec<f32> {
    let (freq, rate) = (f64::from(freq), f64::from(rate));
    let shape = |n: usize| {
        let phase = TAU * freq * n as f64 / rate + phase_shift;
        let decay = 10.0_f64.powf(-(n as f64) / rate);
        (phase.sin() + second * (2.0 * phase + second_phase).sin() + 0.4 * (3.0 * phase).sin())
            * decay
    };
    let first_peak = (0..(rate / freq).ceil() as usize)
        .map(|n| shape(n).abs())
        .fold(0.0, f64::max);
    let scale = 10.0_f64.powf(first_peak_db / 20.0) / first_peak;
    (0..(NOTE_SECONDS * rate) as usize)
        .map(|n| (scale * shape(n)) as f32)
        .collect()
}

fn hann(frame: &[f32]) -> Vec<f32> {
    let last = (frame.len() - 1) as f64;
    frame
        .iter()
        .enumerate()
        .map(|(n, &s)| s * (0.5 - 0.5 * (TAU * n as f64 / last).cos()) as f32)
        .collect()
}

/// Every frame, per R3, in which a candidate other than `ratio`·f is at least as strong as the target.
fn tracking_failures(
    voice: usize,
    ratio: f32,
    range: f32,
    notes: &[f32],
    rates: &[f32],
    note: &dyn Fn(f32, f32) -> Vec<f32>,
) -> Vec<String> {
    let mut failures = Vec::new();
    for &rate in rates {
        for &freq in notes {
            let settings = voice_alone(voice, &[(TONE, 2000.0), (RANGE, range)]);
            let output = run(&mut octave_at(rate, &settings), &note(freq, rate));
            let target = ratio * freq;
            let frame = (FRAME_PERIODS * rate / target).ceil() as usize;
            let end = (MEASURE_TO_SECONDS * f64::from(rate)) as usize;
            let mut start = (MEASURE_FROM_SECONDS * f64::from(rate)) as usize;
            while start + frame <= end {
                let windowed = hann(&output[start..start + frame]);
                let mags: Vec<f32> = CANDIDATES
                    .iter()
                    .map(|c| bin_mag(&windowed, c * freq, rate))
                    .collect();
                let target_mag = bin_mag(&windowed, target, rate);
                let beaten = CANDIDATES
                    .iter()
                    .zip(&mags)
                    .any(|(&c, &mag)| (c - ratio).abs() > f32::EPSILON && mag >= target_mag);
                if beaten {
                    failures.push(format!(
                        "{freq} Hz at {rate}: frame at {:.3} s, mags {mags:?}",
                        start as f32 / rate
                    ));
                }
                start += frame / 2;
            }
        }
    }
    failures
}

fn assert_tracks(voice: usize, ratio: f32, range: f32, notes: &[f32]) {
    assert_tracks_note(voice, ratio, range, notes, &plucked);
}

fn assert_tracks_note(
    voice: usize,
    ratio: f32,
    range: f32,
    notes: &[f32],
    note: &dyn Fn(f32, f32) -> Vec<f32>,
) {
    let failures = tracking_failures(voice, ratio, range, notes, &RATES, note);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn oct1_tracks_bass_notes() {
    assert_tracks(OCT1, 0.5, BASS, &BASS_NOTES);
}

#[test]
fn oct1_tracks_guitar_notes() {
    assert_tracks(OCT1, 0.5, GUITAR, &GUITAR_NOTES);
}

#[test]
fn oct2_tracks_bass_notes() {
    assert_tracks(OCT2, 0.25, BASS, &BASS_NOTES);
}

#[test]
fn oct2_tracks_guitar_notes() {
    assert_tracks(OCT2, 0.25, GUITAR, &GUITAR_NOTES);
}

#[test]
fn silence_in_silence_out() {
    for range in [BASS, GUITAR] {
        let settings = [(OCT1, 100.0), (OCT2, 100.0), (UP, 100.0), (RANGE, range)];
        let output = run(
            &mut prepared(Octave::new(), &settings),
            &vec![0.0; SAMPLE_RATE as usize],
        );
        assert_eq!(peak(&output), 0.0, "range {range}");
    }
}

#[test]
fn finite_and_bounded_under_loud_input() {
    let input = full_scale_sine_then_noise();
    for range in [BASS, GUITAR] {
        let settings = [
            (OCT1, 100.0),
            (OCT2, 100.0),
            (UP, 100.0),
            (TONE, 2000.0),
            (RANGE, range),
        ];
        let output = run(&mut prepared(Octave::new(), &settings), &input);
        assert!(output.iter().all(|s| s.is_finite()), "range {range}");
        assert!(peak(&output) < 8.0, "range {range} peak {}", peak(&output));
    }
}

#[test]
fn output_stays_finite_at_8_khz() {
    let input: Vec<f32> = (0..8000)
        .map(|n| 0.25 * (TAU * 110.0 * f64::from(n) / 8000.0).sin() as f32)
        .collect();
    for range in [BASS, GUITAR] {
        for settings in [
            vec![(UP, 100.0), (RANGE, range)],
            vec![
                (OCT1, 100.0),
                (OCT2, 100.0),
                (UP, 100.0),
                (TONE, 2000.0),
                (RANGE, range),
            ],
        ] {
            let output = run(&mut octave_at(8000.0, &settings), &input);
            assert!(output.iter().all(|s| s.is_finite()), "range {range}");
        }
    }
}

#[test]
fn dry_alone_after_reset_is_the_input() {
    for rate in RATES {
        let loud = noise(0.9, rate as usize / 4);
        let mut module = octave_at(rate, &[(OCT1, 100.0), (OCT2, 100.0), (UP, 100.0)]);
        run(&mut module, &loud);
        for (idx, value) in [(DRY, 100.0), (OCT1, 0.0), (OCT2, 0.0), (UP, 0.0)] {
            module.set_param(idx, value);
        }
        module.reset();
        let input = noise(0.5, rate as usize / 2);
        assert_eq!(run(&mut module, &input), input, "rate {rate}");
    }
}

#[test]
fn level_jump_is_smoothed() {
    let input = sine(0.5, 110.0, SAMPLE_RATE, BLOCK * 300);
    let mut module = prepared(
        Octave::new(),
        &[(DRY, 0.0), (OCT1, 0.0), (OCT2, 0.0), (UP, 0.0)],
    );
    let jump_at = BLOCK * 188;
    let mut output = run(&mut module, &input[..jump_at]);
    for voice in [DRY, OCT1, OCT2, UP] {
        module.set_param(voice, 100.0);
    }
    output.extend(run(&mut module, &input[jump_at..]));
    let largest_step = output[jump_at - 1..]
        .windows(2)
        .fold(0.0_f32, |m, pair| m.max((pair[1] - pair[0]).abs()));
    assert!(largest_step < 0.1 * 0.5, "step {largest_step}");
}

#[test]
fn output_falls_silent_after_input_stops() {
    for range in [BASS, GUITAR] {
        let mut input = sine(0.5, 82.0, SAMPLE_RATE, SAMPLE_RATE as usize / 2);
        let stop = input.len();
        input.extend(vec![0.0; SAMPLE_RATE as usize / 2]);
        let settings = [(OCT1, 100.0), (OCT2, 100.0), (UP, 100.0), (RANGE, range)];
        let output = run(&mut prepared(Octave::new(), &settings), &input);
        let after = peak(&output[stop + SAMPLE_RATE as usize / 5..]);
        assert!(db(after) < -60.0, "range {range}: {} dBFS", db(after));
    }
}

#[test]
fn oct1_polarity_switch_makes_no_step() {
    let input = sine(0.5, 55.0, SAMPLE_RATE, SAMPLE_RATE as usize);
    let settings = voice_alone(OCT1, &[(TONE, 2000.0)]);
    let output = run(&mut prepared(Octave::new(), &settings), &input);
    let settled = &output[SAMPLE_RATE as usize / 5..];
    let largest_step = settled
        .windows(2)
        .fold(0.0_f32, |m, pair| m.max((pair[1] - pair[0]).abs()));
    assert!(
        largest_step < 0.02 * peak(settled),
        "step {largest_step}, peak {}",
        peak(settled)
    );
}

#[test]
fn up_voice_doubles_the_note() {
    for freq in [220.0, 440.0, 880.0] {
        let input = sine(0.25, freq, SAMPLE_RATE, SAMPLE_RATE as usize * 3 / 5);
        let output = run(&mut prepared(Octave::new(), &voice_alone(UP, &[])), &input);
        let windowed = hann(&output[SAMPLE_RATE as usize / 10..]);
        let doubled = bin_mag(&windowed, 2.0 * freq, SAMPLE_RATE);
        for other in [0.5 * freq, freq, 3.0 * freq] {
            let mag = bin_mag(&windowed, other, SAMPLE_RATE);
            assert!(
                doubled > mag,
                "{freq} Hz: {doubled} at 2f, {mag} at {other}"
            );
        }
        let fundamental = db(bin_mag(&windowed, freq, SAMPLE_RATE) / doubled);
        assert!(
            fundamental <= -20.0,
            "{freq} Hz: fundamental {fundamental} dB"
        );
    }
}

fn steady_voice(voice: usize, amplitude: f32, tone: f32) -> Vec<f32> {
    let input = sine(
        amplitude,
        110.0,
        SAMPLE_RATE,
        SAMPLE_RATE as usize * 13 / 10,
    );
    let settings = voice_alone(voice, &[(TONE, tone)]);
    let output = run(&mut prepared(Octave::new(), &settings), &input);
    output[SAMPLE_RATE as usize * 3 / 10..].to_vec()
}

#[test]
fn subs_follow_the_input_level() {
    for voice in [OCT1, OCT2] {
        let loud = rms(&steady_voice(voice, 0.25, 500.0));
        let quiet = rms(&steady_voice(voice, 0.0625, 500.0));
        let drop = db(loud / quiet);
        assert!((drop - 12.0).abs() <= 1.0, "voice {voice}: {drop} dB");
    }
}

fn above_1k(signal: &[f32]) -> Vec<f32> {
    let coefficients =
        SvfCoefficients::high_pass(1000.0, std::f32::consts::FRAC_1_SQRT_2, SAMPLE_RATE);
    let (mut first, mut second) = (Svf::new(), Svf::new());
    signal
        .iter()
        .map(|&s| second.process(&coefficients, first.process(&coefficients, s)))
        .collect()
}

#[test]
fn tone_darkens_the_subs() {
    let dark = rms(&above_1k(&steady_voice(OCT2, 0.25, 200.0)));
    let bright = rms(&above_1k(&steady_voice(OCT2, 0.25, 2000.0)));
    assert!(db(bright / dark) >= 10.0, "{} dB", db(bright / dark));
}

#[test]
fn range_is_a_choice_of_bass_and_guitar() {
    let params = Octave::new().spec().params;
    let labelled: Vec<(&str, &[&str])> = params
        .iter()
        .filter(|param| !param.labels.is_empty())
        .map(|param| (param.name, param.labels))
        .collect();
    assert_eq!(labelled, [("range", &["bass", "guitar"][..])]);
}

fn assert_tracks_variant(first_peak_db: f64, second: f64, phase_shift: f64) {
    let note = move |f, r| shaped_note(f, r, first_peak_db, second, phase_shift, 0.0);
    for (voice, ratio) in [(OCT1, 0.5), (OCT2, 0.25)] {
        assert_tracks_note(voice, ratio, BASS, &[41.0, 98.0], &note);
        assert_tracks_note(voice, ratio, GUITAR, &[110.0, 330.0], &note);
    }
}

#[test]
fn tracking_holds_in_cosine_phase() {
    assert_tracks_variant(FIRST_PEAK_DB, 0.8, std::f64::consts::FRAC_PI_2);
}

#[test]
fn tracking_holds_at_minus_30_dbfs() {
    assert_tracks_variant(-30.0, 0.8, 0.0);
}

#[test]
fn tracking_holds_at_minus_3_dbfs() {
    assert_tracks_variant(-3.0, 0.8, 0.0);
}

fn strongest_candidate(frame: &[f32], freq: f32, rate: f32) -> f32 {
    let windowed = hann(frame);
    let mags: Vec<f32> = CANDIDATES
        .iter()
        .map(|c| bin_mag(&windowed, c * freq, rate))
        .collect();
    let (best, _) =
        mags.iter().enumerate().fold(
            (0, 0.0_f32),
            |(bi, bm), (i, &m)| if m > bm { (i, m) } else { (bi, bm) },
        );
    CANDIDATES[best]
}

#[test]
fn legato_change_of_pitch_moves_the_sub_within_100_ms() {
    let rate = SAMPLE_RATE;
    let half = rate as usize / 2;
    for (first, second) in [(110.0, 220.0), (220.0, 110.0)] {
        let mut input = plucked(first, rate)[..half].to_vec();
        input.extend_from_slice(&plucked(second, rate)[..half]);
        let settings = voice_alone(OCT1, &[(TONE, 2000.0), (RANGE, BASS)]);
        let output = run(&mut octave_at(rate, &settings), &input);
        let from = half + rate as usize / 10;
        let frame = (FRAME_PERIODS * rate / (0.5 * second)).ceil() as usize;
        let best = strongest_candidate(&output[from..from + frame], second, rate);
        assert!(
            (best - 0.5).abs() < f32::EPSILON,
            "{first} to {second} Hz: strongest at {best} x the new note"
        );
    }
}

#[test]
fn tracking_holds_at_the_range_edges() {
    assert_tracks(OCT1, 0.5, BASS, &[28.0, 30.0]);
    assert_tracks(OCT1, 0.5, GUITAR, &[1100.0]);
}

#[test]
fn output_does_not_depend_on_the_block_size() {
    let input = plucked(110.0, SAMPLE_RATE);
    let settings = [
        (DRY, 100.0),
        (OCT1, 100.0),
        (OCT2, 100.0),
        (UP, 100.0),
        (TONE, 800.0),
    ];
    let render = |block: usize| {
        let mut module = Octave::new();
        for &(idx, value) in &settings {
            module.set_param(idx, value);
        }
        module.prepare(SAMPLE_RATE, 256);
        module.reset();
        let mut output = input.clone();
        let (mut left, mut right) = ([0.0; 256], [0.0; 256]);
        for chunk in output.chunks_mut(block) {
            module.process(&mut AudioBuffers {
                frames: chunk.len(),
                mono: chunk,
                left: &mut left,
                right: &mut right,
            });
        }
        output
    };
    let reference = render(BLOCK);
    for block in [1, 64, 97, 256] {
        assert_eq!(render(block), reference, "block {block}");
    }
}

#[test]
fn range_switched_while_a_note_rings_stays_bounded() {
    let rate = SAMPLE_RATE;
    let mut input = plucked(98.0, rate)[..rate as usize].to_vec();
    input.extend(vec![0.0; rate as usize / 2]);
    let settings = [(DRY, 100.0), (OCT1, 100.0), (OCT2, 100.0), (UP, 100.0)];
    let mut module = octave_at(rate, &settings);
    let mut output = Vec::new();
    for (n, chunk) in input.chunks(rate as usize / 20).enumerate() {
        module.set_param(RANGE, (n % 2) as f32);
        output.extend(run(&mut module, chunk));
    }
    assert!(output.iter().all(|s| s.is_finite()));
    assert!(peak(&output) < 8.0, "peak {}", peak(&output));
    let after = peak(&output[rate as usize + rate as usize / 5..]);
    assert!(db(after) < -60.0, "{} dBFS", db(after));
}

#[test]
fn a_note_under_minus_70_dbfs_makes_no_sub() {
    let sub_peak = |level_db: f32| {
        let input = sine(
            10.0_f32.powf(level_db / 20.0),
            82.0,
            SAMPLE_RATE,
            SAMPLE_RATE as usize,
        );
        let settings = voice_alone(OCT1, &[(OCT2, 100.0), (TONE, 2000.0)]);
        peak(&run(&mut octave_at(SAMPLE_RATE, &settings), &input))
    };
    assert!(db(sub_peak(-60.0)) > -80.0, "a -60 dBFS note makes a sub");
    assert!(db(sub_peak(-75.0)) < -120.0, "{} dBFS", db(sub_peak(-75.0)));
}

#[test]
fn oct1_holds_half_the_note_whatever_the_second_harmonic() {
    let shapes = [
        ("+0.8 sine", 0.8, 0.0),
        ("-0.8 sine", -0.8, 0.0),
        ("0.8 cosine", 0.8, std::f64::consts::FRAC_PI_2),
    ];
    let mut failures = Vec::new();
    for (name, second, second_phase) in shapes {
        let note = move |f, r| shaped_note(f, r, FIRST_PEAK_DB, second, 0.0, second_phase);
        for (range, notes) in [(BASS, &BASS_NOTES[..]), (GUITAR, &GUITAR_NOTES[..])] {
            for failure in tracking_failures(OCT1, 0.5, range, notes, &[SAMPLE_RATE], &note) {
                failures.push(format!("2f {name}: {failure}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A plucked string: eight partials at 1/k^0.9 and k·f·√(1 + b·k²), each decaying (15 + 3k) dB per second, in phases from `seed`, first peak at −12 dBFS.
fn inharmonic_note(freq: f32, rate: f32, b: f64, seed: u64) -> Vec<f32> {
    let mut state = seed;
    let mut next_phase = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        TAU * (state >> 11) as f64 / (1_u64 << 53) as f64
    };
    let partials: Vec<(f64, f64, f64, f64)> = (1..=8)
        .map(|k| {
            let k = f64::from(k);
            let freq = f64::from(freq) * k * (1.0 + b * k * k).sqrt();
            (freq, k.powf(-0.9), 15.0 + 3.0 * k, next_phase())
        })
        .collect();
    let rate = f64::from(rate);
    let note: Vec<f64> = (0..(NOTE_SECONDS * rate) as usize)
        .map(|n| {
            let t = n as f64 / rate;
            partials
                .iter()
                .map(|&(freq, amp, decay_db, phase)| {
                    amp * 10.0_f64.powf(-decay_db * t / 20.0) * (TAU * freq * t + phase).sin()
                })
                .sum()
        })
        .collect();
    let scale =
        10.0_f64.powf(FIRST_PEAK_DB / 20.0) / note.iter().fold(0.0_f64, |m, s| m.max(s.abs()));
    note.iter().map(|s| (scale * s) as f32).collect()
}

#[test]
fn oct1_holds_half_the_note_on_inharmonic_strings() {
    let mut failures = Vec::new();
    for (b, seed) in [(4e-4, 1), (8e-4, 2), (4e-4, 3)] {
        let note = move |f, r| inharmonic_note(f, r, b, seed);
        for (range, notes) in [
            (BASS, &[41.0, 55.0, 98.0][..]),
            (GUITAR, &[82.0, 110.0][..]),
        ] {
            for failure in tracking_failures(OCT1, 0.5, range, notes, &[SAMPLE_RATE], &note) {
                failures.push(format!("b {b} seed {seed}: {failure}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn tone_jump_is_smoothed() {
    let input = sine(0.5, 110.0, SAMPLE_RATE, BLOCK * 300);
    let mut module = prepared(Octave::new(), &voice_alone(OCT1, &[(TONE, 100.0)]));
    let jump_at = BLOCK * 188;
    let mut output = run(&mut module, &input[..jump_at]);
    module.set_param(TONE, 2000.0);
    output.extend(run(&mut module, &input[jump_at..]));
    let largest_step = |part: &[f32]| {
        part.windows(2)
            .fold(0.0_f32, |m, pair| m.max((pair[1] - pair[0]).abs()))
    };
    let settled = largest_step(&output[output.len() - BLOCK * 40..]);
    let jump = largest_step(&output[jump_at - 1..jump_at + BLOCK * 8]);
    assert!(
        jump <= 3.0 * settled,
        "jump step {jump}, settled step {settled}"
    );
}

#[test]
fn oct1_level_follows_the_note_to_the_top_of_the_guitar_range() {
    let half_note = |freq: f32| {
        let input = sine(0.25, freq, SAMPLE_RATE, SAMPLE_RATE as usize);
        let settings = voice_alone(OCT1, &[(TONE, 2000.0), (RANGE, GUITAR)]);
        let output = run(&mut prepared(Octave::new(), &settings), &input);
        bin_mag(
            &hann(&output[SAMPLE_RATE as usize * 3 / 10..]),
            0.5 * freq,
            SAMPLE_RATE,
        )
    };
    let pre_filter = SvfCoefficients::low_pass(400.0, std::f32::consts::FRAC_1_SQRT_2, SAMPLE_RATE);
    let (low, high) = (659.0, 988.0);
    let expected = pre_filter.gain_db(high, SAMPLE_RATE) - pre_filter.gain_db(low, SAMPLE_RATE);
    let measured = db(half_note(high) / half_note(low));
    assert!(
        (measured - expected).abs() <= 2.0,
        "{low} to {high} Hz: {measured} dB, the pre-filter alone {expected} dB"
    );
}

#[test]
fn unprepared_module_outputs_finite_samples() {
    let input = common::sine(0.25, 110.0, SAMPLE_RATE, 4800);
    let output = run(&mut Octave::new(), &input);
    assert!(output.iter().all(|sample| sample.is_finite()));
}
