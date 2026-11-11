mod common;

use common::{BLOCK, SAMPLE_RATE, frames, full_scale_sine_then_noise, noise, peak, prepared, sine};
use xguitare_dsp::{AudioBuffers, Limiter, Module, StereoRack, StereoSlot};

const INPUT: usize = 0;
const CEILING: usize = 1;
const RELEASE: usize = 2;
const LOOKAHEAD: usize = 3;
const LOOKAHEADS_MS: [f32; 3] = [0.0, 1.0, 5.0];

fn run(module: &mut Limiter, input: &[f32]) -> Vec<f32> {
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

fn linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Noise whose loudness jumps every few ms, from far under to far over the ceiling.
fn loud_bursts(seed: usize) -> Vec<f32> {
    let source = noise(1.0, frames(400.0) + seed * 97);
    source[seed * 97..]
        .chunks(frames(7.0))
        .enumerate()
        .flat_map(|(n, chunk)| {
            let level = [0.05, 4.0, 0.3, 1.5, 8.0, 0.01][(n + seed) % 6];
            chunk.iter().map(move |s| s * level)
        })
        .collect()
}

#[test]
fn never_exceeds_the_ceiling() {
    for lookahead in LOOKAHEADS_MS {
        for (seed, ceiling) in [-12.0, -6.0, -1.0, 0.0].into_iter().enumerate() {
            for input_db in [0.0, 18.0] {
                let mut limiter = prepared(
                    Limiter::new(),
                    &[
                        (LOOKAHEAD, lookahead),
                        (CEILING, ceiling),
                        (INPUT, input_db),
                        (RELEASE, 10.0),
                    ],
                );
                let output = run(&mut limiter, &loud_bursts(seed));
                let bound = linear(ceiling) + 1e-6;
                assert!(
                    peak(&output) <= bound,
                    "lookahead {lookahead} ms, ceiling {ceiling} dB, input {input_db} dB: peak {}",
                    peak(&output)
                );
            }
        }
    }
}

#[test]
fn a_signal_under_the_ceiling_passes_unchanged_after_the_lookahead() {
    let input = sine(0.85, 110.0, SAMPLE_RATE, frames(200.0));
    for lookahead in LOOKAHEADS_MS {
        let mut limiter = prepared(Limiter::new(), &[(LOOKAHEAD, lookahead)]);
        let output = run(&mut limiter, &input);
        let delay = frames(lookahead);
        assert!(
            output[..delay].iter().all(|&s| s == 0.0),
            "lookahead {lookahead} ms"
        );
        assert_eq!(
            output[delay..],
            input[..input.len() - delay],
            "lookahead {lookahead} ms"
        );
    }
}

/// The time from a single peak 6 dB over the ceiling leaving the delay to the gain on a steady level under the ceiling being back at 90 %.
fn recovery_ms(release_ms: f32) -> f32 {
    let ceiling = linear(-1.0);
    let steady = 0.1;
    let mut input = vec![steady; frames(3.0 * release_ms + 50.0)];
    let peak_at = frames(20.0);
    input[peak_at] = 2.0 * ceiling;
    let mut limiter = prepared(Limiter::new(), &[(RELEASE, release_ms)]);
    let output = run(&mut limiter, &input);
    let leaves = peak_at + frames(1.0);
    let back = output[leaves + 1..]
        .iter()
        .position(|&s| s / steady >= 0.9)
        .expect("gain recovers");
    (back + 1) as f32 * 1000.0 / SAMPLE_RATE
}

/// A decay at `release` from a peak 6 dB over the ceiling reaches 90 % gain after ln(1.8) ≈ 0.59 release.
#[test]
fn gain_recovers_to_90_percent_within_the_release_time() {
    for release_ms in [10.0, 100.0, 400.0] {
        let recovered = recovery_ms(release_ms);
        assert!(
            recovered >= 0.4 * release_ms && recovered <= release_ms,
            "release {release_ms} ms: 90 % after {recovered} ms"
        );
    }
}

#[test]
fn a_peak_is_limited_before_it_leaves_the_delay() {
    let ceiling = linear(-1.0);
    let mut input = vec![0.1; frames(50.0)];
    let peak_at = frames(20.0);
    input[peak_at] = 4.0;
    let mut limiter = prepared(Limiter::new(), &[]);
    let output = run(&mut limiter, &input);
    let leaves = peak_at + frames(1.0);
    assert!(
        (output[leaves] - ceiling).abs() < 1e-6,
        "{}",
        output[leaves]
    );
    assert!(output[leaves - 1] < 0.1 * ceiling / 4.0 + 1e-6);
}

fn run_stereo_rack(rack: &mut StereoRack, in_l: &[f32], in_r: &[f32]) -> (Vec<f32>, Vec<f32>) {
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

#[test]
fn linked_applies_the_louder_side_gain_to_both() {
    let mut rack = StereoRack::new(vec![(
        "limiter".to_string(),
        StereoSlot::Linked(Box::new(Limiter::new())),
    )])
    .expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    let in_l = sine(2.0, 220.0, SAMPLE_RATE, frames(300.0));
    let in_r = sine(0.1, 220.0, SAMPLE_RATE, frames(300.0));
    let (out_l, out_r) = run_stereo_rack(&mut rack, &in_l, &in_r);
    let delay = frames(1.0);
    for n in frames(100.0)..frames(300.0) {
        if in_r[n - delay].abs() < 0.01 {
            continue;
        }
        let gain_l = out_l[n] / in_l[n - delay];
        let gain_r = out_r[n] / in_r[n - delay];
        assert!(gain_r < 0.5, "right gain {gain_r} at {n}");
        assert!(
            (gain_l - gain_r).abs() < 1e-4,
            "left {gain_l} right {gain_r} at {n}"
        );
    }
}

#[test]
fn silence_in_silence_out() {
    let mut limiter = prepared(Limiter::new(), &[(INPUT, 18.0)]);
    let output = run(&mut limiter, &vec![0.0; SAMPLE_RATE as usize]);
    assert_eq!(peak(&output), 0.0);
}

#[test]
fn finite_under_full_scale() {
    for lookahead in LOOKAHEADS_MS {
        let mut limiter = prepared(Limiter::new(), &[(INPUT, 18.0), (LOOKAHEAD, lookahead)]);
        let output = run(&mut limiter, &full_scale_sine_then_noise());
        assert!(
            output.iter().all(|s| s.is_finite()),
            "lookahead {lookahead}"
        );
    }
}

fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()))
}

/// Name, base settings, Parameter, from, to, input amplitude.
type JumpCase<'a> = (&'a str, &'a [(usize, f32)], usize, f32, f32, f32);

/// A jump lands on a peak of the input sine; unsmoothed, it puts a step into the output; a smoothed one keeps every step near the settled output's largest, and a lookahead jump changes the speed of the delay at most twofold.
#[test]
fn every_parameter_jump_is_smoothed() {
    let cases: [JumpCase; 4] = [
        ("input", &[], INPUT, 0.0, 18.0, 0.1),
        ("ceiling", &[], CEILING, -12.0, 0.0, 0.5),
        ("release", &[(CEILING, -12.0)], RELEASE, 1000.0, 10.0, 0.5),
        ("lookahead", &[], LOOKAHEAD, 5.0, 0.0, 0.5),
    ];
    let period = SAMPLE_RATE / 130.0;
    let settle = ((frames(500.0) as f32 / period).round() * period + period / 4.0) as usize;
    let mut failures = Vec::new();
    for (name, base, idx, from, to, amplitude) in cases {
        let input = sine(amplitude, 130.0, SAMPLE_RATE, settle + frames(100.0));
        let mut settings = base.to_vec();
        settings.push((idx, from));
        let mut module = prepared(Limiter::new(), &settings);
        let mut jumped = run(&mut module, &input[..settle]);
        module.set_param(idx, to);
        jumped.extend(run(&mut module, &input[settle..]));
        *settings.last_mut().unwrap() = (idx, to);
        let settled = run(&mut prepared(Limiter::new(), &settings), &input);

        let window = settle - 1..input.len();
        let bound = 2.5 * largest_step(&settled[window.clone()]) + 1e-4;
        let step = largest_step(&jumped[window]);
        if step >= bound {
            failures.push(format!("{name}: {step} >= {bound}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn prepare_clears_the_state() {
    let mut limiter = prepared(Limiter::new(), &[(LOOKAHEAD, 5.0)]);
    run(&mut limiter, &noise(4.0, frames(50.0)));
    limiter.prepare(SAMPLE_RATE, BLOCK);
    let input = sine(0.5, 110.0, SAMPLE_RATE, frames(50.0));
    let output = run(&mut limiter, &input);
    let delay = frames(5.0);
    assert_eq!(output[delay..], input[..input.len() - delay]);
}

#[test]
fn params_follow_the_spec_order() {
    let names: Vec<&str> = Limiter::new()
        .spec()
        .params
        .iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["input", "ceiling", "release", "lookahead"]);
}
