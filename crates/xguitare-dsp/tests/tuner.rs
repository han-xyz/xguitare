mod common;

use common::{noise, sine};
use xguitare_dsp::tuner::{Detector, Pitch, note_name};

fn detect(samples: &[f32], rate: f32) -> Option<Pitch> {
    let mut detector = Detector::new(rate);
    detector.push(samples);
    detector.analyse()
}

fn cents_between(found: f32, expected: f32) -> f32 {
    1200.0 * (found / expected).log2()
}

fn sawtooth(freq: f32, rate: f32, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames];
    for k in 1..=12 {
        let harmonic = sine(0.5 / k as f32, freq * k as f32, rate, frames);
        for (o, h) in out.iter_mut().zip(harmonic) {
            *o += h;
        }
    }
    out
}

#[test]
fn detects_sines_within_one_cent() {
    for rate in [48_000.0, 44_100.0] {
        for hz in [41.2, 55.0, 82.4, 110.0, 220.0, 440.0, 1318.5] {
            let pitch = detect(&sine(0.5, hz, rate, 8192), rate)
                .unwrap_or_else(|| panic!("{hz} Hz at {rate} not detected"));
            assert!(pitch.confident, "{hz} Hz at {rate}: {pitch:?}");
            let off = cents_between(pitch.hz, hz);
            assert!(off.abs() <= 1.0, "{hz} Hz at {rate}: {off} cents off");
        }
    }
}

#[test]
fn silence_and_noise_are_not_confident() {
    assert!(detect(&vec![0.0; 8192], 48_000.0).is_none());
    let quiet = detect(&noise(0.0003, 8192), 48_000.0);
    assert!(quiet.is_none_or(|pitch| !pitch.confident), "{quiet:?}");
}

#[test]
fn sawtooth_reports_fundamental() {
    let pitch = detect(&sawtooth(55.0, 48_000.0, 8192), 48_000.0).expect("detected");
    let off = cents_between(pitch.hz, 55.0);
    assert!(off.abs() <= 5.0, "found {} Hz", pitch.hz);
}

#[test]
fn out_of_range_is_none() {
    for hz in [20.0, 2000.0] {
        let found = detect(&sine(0.5, hz, 48_000.0, 8192), 48_000.0);
        assert!(found.is_none(), "{hz} Hz: {found:?}");
    }
}

#[test]
fn note_names_and_cents() {
    for (hz, name, cents) in [
        (440.0, "A4", 0),
        (41.203, "E1", 0),
        (41.0, "E1", -9),
        (466.16, "A#4", 0),
        (452.89, "A4", 50),
        (1318.5, "E6", 0),
        (261.63, "C4", 0),
        (246.94, "B3", 0),
    ] {
        let note = note_name(hz, 440.0);
        assert_eq!(
            (note.to_string().as_str(), note.cents),
            (name, cents),
            "{hz} Hz"
        );
    }
}

#[test]
fn reference_pitch_shifts_cents() {
    let note = note_name(440.0, 442.0);
    assert_eq!((note.to_string().as_str(), note.cents), ("A4", -8));
}
