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
    let (freq, rate) = (f64::from(freq), f64::from(rate));
    let shape = |n: usize| {
        let phase = TAU * freq * n as f64 / rate;
        let decay = 10.0_f64.powf(-(n as f64) / rate);
        (phase.sin() + 0.8 * (2.0 * phase).sin() + 0.4 * (3.0 * phase).sin()) * decay
    };
    let first_peak = (0..(rate / freq).ceil() as usize)
        .map(|n| shape(n).abs())
        .fold(0.0, f64::max);
    let scale = 10.0_f64.powf(FIRST_PEAK_DB / 20.0) / first_peak;
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
fn tracking_failures(voice: usize, ratio: f32, range: f32, notes: &[f32]) -> Vec<String> {
    let mut failures = Vec::new();
    for rate in RATES {
        for &freq in notes {
            let settings = voice_alone(voice, &[(TONE, 2000.0), (RANGE, range)]);
            let output = run(&mut octave_at(rate, &settings), &plucked(freq, rate));
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
    let failures = tracking_failures(voice, ratio, range, notes);
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
fn dry_alone_after_reset_is_the_input() {
    let input = noise(0.5, SAMPLE_RATE as usize / 2);
    let settings = [(DRY, 100.0), (OCT1, 0.0), (OCT2, 0.0), (UP, 0.0)];
    let output = run(&mut prepared(Octave::new(), &settings), &input);
    assert_eq!(output, input);
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
