mod common;

use std::ops::Range;

use common::{
    BLOCK, SAMPLE_RATE, bin_mag, db, frames, full_scale_sine_then_noise, noise, peak, prepared,
    rms, sine,
};
use xguitare_dsp::{AudioBuffers, Module, Shaper, StereoRack, StereoSlot};

const ATTACK: usize = 0;
const SUSTAIN: usize = 1;
const OUTPUT: usize = 2;
const MIX: usize = 3;
const ONSET_MS: f32 = 50.0;

fn run(module: &mut Shaper, input: &[f32]) -> Vec<f32> {
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

/// 50 ms of silence, then a 110 Hz note that starts at `level_db` dBFS and decays by 1/e every 200 ms.
fn pluck(level_db: f32) -> Vec<f32> {
    let amplitude = 10.0_f32.powf(level_db / 20.0);
    let note = sine(amplitude, 110.0, SAMPLE_RATE, frames(800.0));
    let mut input = vec![0.0; frames(ONSET_MS)];
    input.extend(
        note.iter()
            .enumerate()
            .map(|(n, s)| s * (-(n as f32) / SAMPLE_RATE / 0.2).exp()),
    );
    input
}

fn after_onset(from_ms: f32, to_ms: f32) -> Range<usize> {
    frames(ONSET_MS + from_ms)..frames(ONSET_MS + to_ms)
}

fn onset() -> Range<usize> {
    after_onset(0.0, 10.0)
}

fn body() -> Range<usize> {
    after_onset(150.0, 250.0)
}

fn tail() -> Range<usize> {
    after_onset(300.0, 700.0)
}

fn shaped(settings: &[(usize, f32)], level_db: f32) -> (Vec<f32>, Vec<f32>) {
    let input = pluck(level_db);
    let output = run(&mut prepared(Shaper::new(), settings), &input);
    (input, output)
}

/// How much the onset peak rises against the body of the note, in dB.
fn onset_lift_db(settings: &[(usize, f32)]) -> f32 {
    let (input, output) = shaped(settings, -12.0);
    let contrast = |s: &[f32]| db(peak(&s[onset()]) / peak(&s[body()]));
    contrast(&output) - contrast(&input)
}

fn gain_db(input: &[f32], output: &[f32], window: Range<usize>) -> f32 {
    db(rms(&output[window.clone()]) / rms(&input[window]))
}

#[test]
fn zeros_pass_the_input_unchanged_at_any_mix() {
    let input = pluck(-6.0);
    for mix in [0.0, 37.0, 100.0] {
        let output = run(&mut prepared(Shaper::new(), &[(MIX, mix)]), &input);
        assert_eq!(output, input, "mix {mix}");
    }
}

#[test]
fn attack_raises_or_lowers_the_onset_against_the_body() {
    let raised = onset_lift_db(&[(ATTACK, 12.0)]);
    let lowered = onset_lift_db(&[(ATTACK, -12.0)]);
    assert!(raised > 3.0, "attack +12 dB lifts the onset {raised} dB");
    assert!(lowered < -3.0, "attack -12 dB lifts the onset {lowered} dB");
}

#[test]
fn sustain_lowers_or_raises_the_tail() {
    let (input, lowered) = shaped(&[(SUSTAIN, -12.0)], -12.0);
    let (_, raised) = shaped(&[(SUSTAIN, 12.0)], -12.0);
    let lowered_db = gain_db(&input, &lowered, tail());
    let raised_db = gain_db(&input, &raised, tail());
    assert!(lowered_db < -3.0, "sustain -12 dB: tail {lowered_db} dB");
    assert!(raised_db > 3.0, "sustain +12 dB: tail {raised_db} dB");
}

#[test]
fn shaping_does_not_depend_on_the_input_level() {
    let settings = [(ATTACK, 12.0), (SUSTAIN, -12.0)];
    let (loud_in, loud_out) = shaped(&settings, -20.0);
    let (quiet_in, quiet_out) = shaped(&settings, -40.0);
    for (name, window) in [("onset", onset()), ("body", body()), ("tail", tail())] {
        let loud = gain_db(&loud_in, &loud_out, window.clone());
        let quiet = gain_db(&quiet_in, &quiet_out, window);
        assert!(
            (loud - quiet).abs() < 1.0,
            "{name}: {loud} dB at -20 dBFS, {quiet} dB at -40 dBFS"
        );
    }
}

#[test]
fn mix_and_output_scale_the_shaped_signal() {
    let (input, wet) = shaped(&[(SUSTAIN, -12.0)], -12.0);
    let (_, half) = shaped(&[(SUSTAIN, -12.0), (MIX, 50.0), (OUTPUT, 6.0)], -12.0);
    let output = 10.0_f32.powf(6.0 / 20.0);
    for n in tail() {
        let expected = f32::midpoint(input[n], wet[n]) * output;
        assert!(
            (half[n] - expected).abs() < 1e-4,
            "{} vs {expected} at {n}",
            half[n]
        );
    }
}

fn run_stereo_slot(slot: StereoSlot, in_l: &[f32], in_r: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut rack = StereoRack::new(vec![("shaper".to_string(), slot)]).expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    let mut out_l = vec![0.0; in_l.len()];
    let mut out_r = vec![0.0; in_r.len()];
    for (((l, r), ol), or) in in_l
        .chunks(BLOCK)
        .zip(in_r.chunks(BLOCK))
        .zip(out_l.chunks_mut(BLOCK))
        .zip(out_r.chunks_mut(BLOCK))
    {
        rack.process(l, r, ol, or);
    }
    (out_l, out_r)
}

/// The largest difference between the left and right gain where both sides carry signal, and the left onset's gain in dB.
fn largest_side_gain_gap(slot: StereoSlot) -> (f32, f32) {
    let in_l = pluck(-6.0);
    let mut in_r = vec![0.0; frames(100.0)];
    in_r.extend(pluck(-12.0));
    in_r.truncate(in_l.len());
    let (out_l, out_r) = run_stereo_slot(slot, &in_l, &in_r);
    let gap = (frames(ONSET_MS + 100.0)..in_l.len())
        .filter(|&n| in_l[n].abs() > 1e-3 && in_r[n].abs() > 1e-3)
        .map(|n| (out_l[n] / in_l[n] - out_r[n] / in_r[n]).abs())
        .fold(0.0, f32::max);
    (gap, db(peak(&out_l[onset()]) / peak(&in_l[onset()])))
}

/// The right side's note starts 100 ms after the left's: one gain for both sides only when the slot is linked.
#[test]
fn linked_applies_one_gain_to_both_sides() {
    let settings = [(ATTACK, 12.0), (SUSTAIN, -12.0)];
    let (linked, lift) = largest_side_gain_gap(StereoSlot::Linked(Box::new(prepared(
        Shaper::new(),
        &settings,
    ))));
    assert!(linked < 1e-3, "linked gap {linked}");
    assert!(lift > 3.0, "linked onset gain {lift} dB");
    let (doubled, _) = largest_side_gain_gap(StereoSlot::Doubled(
        Box::new(prepared(Shaper::new(), &settings)),
        Box::new(prepared(Shaper::new(), &settings)),
    ));
    assert!(doubled > 0.5, "unlinked gap {doubled}");
}

/// A steady note gives a steady gain: at the extremes the harmonics stay 50 dB under the fundamental, down to low E.
#[test]
fn steady_notes_are_not_distorted() {
    for period in [1_165_usize, 436] {
        let freq = SAMPLE_RATE / period as f32;
        let input = sine(0.3, freq, SAMPLE_RATE, frames(1_000.0) + 40 * period);
        for settings in [
            [(ATTACK, 15.0)],
            [(ATTACK, -15.0)],
            [(SUSTAIN, 24.0)],
            [(SUSTAIN, -24.0)],
        ] {
            let output = run(&mut prepared(Shaper::new(), &settings), &input);
            let settled = &output[frames(1_000.0)..];
            let fundamental = bin_mag(settled, freq, SAMPLE_RATE);
            for harmonic in 2..=5 {
                let level = db(bin_mag(settled, freq * harmonic as f32, SAMPLE_RATE) / fundamental);
                assert!(
                    level < -50.0,
                    "{freq} Hz {settings:?}: harmonic {harmonic} at {level} dB"
                );
            }
        }
    }
}

#[test]
fn silence_in_silence_out() {
    let mut shaper = prepared(
        Shaper::new(),
        &[(ATTACK, 15.0), (SUSTAIN, 24.0), (OUTPUT, 12.0)],
    );
    let output = run(&mut shaper, &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale_at_the_extremes() {
    for (attack, sustain) in [(15.0, 24.0), (-15.0, -24.0), (15.0, -24.0)] {
        let settings = [(ATTACK, attack), (SUSTAIN, sustain), (OUTPUT, 12.0)];
        let output = run(
            &mut prepared(Shaper::new(), &settings),
            &full_scale_sine_then_noise(),
        );
        assert!(output.iter().all(|s| s.is_finite()), "{settings:?}");
    }
}

fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()))
}

/// A jump lands on a peak of a steady sine; unsmoothed, it puts a step into the output; a smoothed one keeps every step near the settled output's largest.
#[test]
fn every_parameter_jump_is_smoothed() {
    let cases: [(&str, usize, f32, f32); 4] = [
        ("attack", ATTACK, -15.0, 15.0),
        ("sustain", SUSTAIN, -24.0, 24.0),
        ("output", OUTPUT, -12.0, 12.0),
        ("mix", MIX, 0.0, 100.0),
    ];
    let base = [(ATTACK, 15.0), (SUSTAIN, -24.0)];
    let period = SAMPLE_RATE / 100.0;
    let settle = ((frames(500.0) as f32 / period).round() * period + period / 4.0) as usize;
    let input = sine(0.1, 100.0, SAMPLE_RATE, settle + frames(100.0));
    let mut failures = Vec::new();
    for (name, idx, from, to) in cases {
        let mut settings = base.to_vec();
        settings.push((idx, from));
        let mut module = prepared(Shaper::new(), &settings);
        let mut jumped = run(&mut module, &input[..settle]);
        module.set_param(idx, to);
        jumped.extend(run(&mut module, &input[settle..]));
        *settings.last_mut().unwrap() = (idx, to);
        let settled = run(&mut prepared(Shaper::new(), &settings), &input);

        let window = settle - 1..input.len();
        let bound = 2.0 * largest_step(&settled[window.clone()]) + 1e-4;
        let step = largest_step(&jumped[window]);
        if step >= bound {
            failures.push(format!("{name}: {step} >= {bound}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The difference a jump makes against the output at `from`, and the difference the settled output at `to` makes, over 10 ms after the jump.
fn jump_steps(input: &[f32], idx: usize, from: f32, to: f32, jump_at: usize) -> (f32, f32) {
    let at = |value: f32| run(&mut prepared(Shaper::new(), &[(idx, value)]), input);
    let (before, after) = (at(from), at(to));
    let mut module = prepared(Shaper::new(), &[(idx, from)]);
    let mut jumped = run(&mut module, &input[..jump_at]);
    module.set_param(idx, to);
    jumped.extend(run(&mut module, &input[jump_at..]));
    let window = jump_at - 1..jump_at + frames(10.0);
    let change: Vec<f32> = window.clone().map(|n| jumped[n] - before[n]).collect();
    let full: Vec<f32> = window.map(|n| after[n] - before[n]).collect();
    (largest_step(&change), largest_step(&full))
}

/// Jumps where the Parameter acts fully, `attack` 2 ms into a note and `sustain` in its decaying tail: the Parameter's Smoother keeps the change's steps well under those of switching the whole note to the new value (about 0.4 of them; without it about 1.0).
#[test]
fn attack_and_sustain_jumps_are_smoothed_where_they_act() {
    let input = pluck(-6.0);
    let cases = [
        ("attack", ATTACK, -15.0, 15.0, frames(ONSET_MS + 2.0)),
        ("sustain", SUSTAIN, -24.0, 24.0, frames(ONSET_MS + 400.0)),
    ];
    for (name, idx, from, to, jump_at) in cases {
        let (change, full) = jump_steps(&input, idx, from, to, jump_at);
        assert!(
            change < 0.7 * full,
            "{name}: largest step of the change {change}, of the settled change {full}"
        );
    }
}

#[test]
fn prepare_clears_the_state() {
    let mut shaper = prepared(Shaper::new(), &[(ATTACK, 15.0)]);
    run(&mut shaper, &noise(1.0, frames(50.0)));
    shaper.prepare(SAMPLE_RATE, BLOCK);
    let input = pluck(-12.0);
    let output = run(&mut shaper, &input);
    let fresh = shaped(&[(ATTACK, 15.0)], -12.0).1;
    assert_eq!(output, fresh);
}

#[test]
fn params_follow_the_spec_order() {
    let names: Vec<&str> = Shaper::new().spec().params.iter().map(|p| p.name).collect();
    assert_eq!(names, ["attack", "sustain", "output", "mix"]);
}
