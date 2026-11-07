mod common;

use std::f32::consts::TAU;
use std::time::Instant;

use xguitare_dsp::modules::cabinet::{Cabinet, CabinetIr};
use xguitare_dsp::{Module, Rack};

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK: usize = 128;

fn cabinet_rack(ir: Option<CabinetIr>) -> Rack {
    let mut rack = Rack::new(vec![(
        "cab".to_string(),
        Box::new(Cabinet::new(ir)) as Box<dyn Module>,
    )])
    .expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    rack
}

fn mono_ir(taps: Vec<f32>) -> Option<CabinetIr> {
    Some(CabinetIr {
        left: taps,
        right: None,
    })
}

fn run(rack: &mut Rack, input: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    for ((chunk, l), r) in input
        .chunks(block)
        .zip(left.chunks_mut(block))
        .zip(right.chunks_mut(block))
    {
        rack.process(chunk, l, r);
    }
    (left, right)
}

fn sine(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| 0.5 * (TAU * 440.0 * n as f32 / SAMPLE_RATE).sin())
        .collect()
}

fn noise(len: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1 << 23) as f32 - 1.0
        })
        .collect()
}

fn direct_convolution(input: &[f32], ir: &[f32]) -> Vec<f32> {
    (0..input.len())
        .map(|n| {
            ir.iter()
                .enumerate()
                .take(n + 1)
                .map(|(k, h)| h * input[n - k])
                .sum()
        })
        .collect()
}

fn assert_close(actual: &[f32], expected: &[f32], tolerance: f32) {
    assert_eq!(actual.len(), expected.len());
    for (n, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!((a - e).abs() <= tolerance, "sample {n}: {a} vs {e}");
    }
}

#[test]
fn unit_impulse_is_identity() {
    let mut rack = cabinet_rack(mono_ir(vec![1.0]));
    let input = sine(10 * BLOCK);
    let (left, right) = run(&mut rack, &input, BLOCK);
    assert_close(&left, &input, 1e-5);
    assert_close(&right, &input, 1e-5);
}

#[test]
fn matches_direct_convolution() {
    let ir = noise(64, 7);
    let input = noise(10 * BLOCK, 11);
    let mut rack = cabinet_rack(mono_ir(ir.clone()));
    let (left, right) = run(&mut rack, &input, BLOCK);
    let expected = direct_convolution(&input, &ir);
    assert_close(&left, &expected, 1e-4);
    assert_close(&right, &expected, 1e-4);
}

#[test]
fn long_ir_matches_direct() {
    let ir: Vec<f32> = noise(1000, 3).iter().map(|h| h * 0.05).collect();
    let input = noise(20 * BLOCK, 5);
    let mut rack = cabinet_rack(mono_ir(ir.clone()));
    let (left, _) = run(&mut rack, &input, BLOCK);
    assert_close(&left, &direct_convolution(&input, &ir), 1e-3);
}

#[test]
fn stereo_ir_differs() {
    let mut rack = cabinet_rack(Some(CabinetIr {
        left: vec![1.0],
        right: Some(vec![0.0, 1.0]),
    }));
    let input = noise(4 * BLOCK, 13);
    let (left, right) = run(&mut rack, &input, BLOCK);
    assert!(right[0].abs() <= 1e-5, "{}", right[0]);
    assert_close(&right[1..], &left[..left.len() - 1], 1e-5);
}

#[test]
fn passthrough_duplicates_mono() {
    let mut rack = cabinet_rack(None);
    let input = sine(3 * BLOCK);
    let (left, right) = run(&mut rack, &input, BLOCK);
    assert_close(&left, &input, 1e-6);
    assert_close(&right, &input, 1e-6);
}

#[test]
fn short_block_matches() {
    let input = sine(10 * BLOCK);
    let (full, _) = run(&mut cabinet_rack(mono_ir(vec![1.0])), &input, BLOCK);
    let (short, _) = run(&mut cabinet_rack(mono_ir(vec![1.0])), &input, BLOCK / 2);
    assert_close(&full, &input, 1e-5);
    assert_close(&short, &input, 1e-5);
}

#[test]
fn throughput_report() {
    let ir: Vec<f32> = noise(4800, 17).iter().map(|h| h * 0.01).collect();
    let mut rack = cabinet_rack(mono_ir(ir));
    let input = noise(SAMPLE_RATE as usize, 19);
    let start = Instant::now();
    run(&mut rack, &input, BLOCK);
    println!(
        "cabinet: 4800-tap IR, 1 s of audio in {:?}",
        start.elapsed()
    );
}

#[test]
fn short_block_matches_long_ir() {
    let ir: Vec<f32> = noise(300, 23).iter().map(|h| h * 0.1).collect();
    let input = noise(20 * BLOCK, 29);
    let expected = direct_convolution(&input, &ir);
    let (full, _) = run(&mut cabinet_rack(mono_ir(ir.clone())), &input, BLOCK);
    assert_close(&full, &expected, 1e-4);

    let mut rack = cabinet_rack(mono_ir(ir));
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    let sizes = [64, 32, 128, 96, 128, 16, 1, 127];
    let mut start = 0;
    for size in sizes.iter().cycle() {
        if start == input.len() {
            break;
        }
        let end = (start + size).min(input.len());
        rack.process(
            &input[start..end],
            &mut left[start..end],
            &mut right[start..end],
        );
        start = end;
    }
    assert_close(&left, &expected, 1e-4);
    assert_close(&right, &expected, 1e-4);
}

const LOW_CUT: usize = 1;
const HIGH_CUT: usize = 2;

fn tone_gain_db(cut: (usize, f32), freq: f32) -> f32 {
    let mut rack = cabinet_rack(mono_ir(vec![1.0]));
    rack.module_mut(0).set_param(cut.0, cut.1);
    let input = common::sine(0.5, freq, SAMPLE_RATE, SAMPLE_RATE as usize);
    let (left, _) = run(&mut rack, &input, BLOCK);
    let settled = input.len() / 2;
    common::db(
        common::bin_mag(&left[settled..], freq, SAMPLE_RATE)
            / common::bin_mag(&input[settled..], freq, SAMPLE_RATE),
    )
}

fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s * s).sum()
}

#[test]
fn default_cuts_are_bit_exact() {
    let input = noise(10 * BLOCK, 31);
    let (left, right) = run(&mut cabinet_rack(None), &input, BLOCK);
    assert_eq!(left, input);
    assert_eq!(right, input);

    let ir = noise(300, 37);
    let (fresh_left, fresh_right) = run(&mut cabinet_rack(mono_ir(ir.clone())), &input, BLOCK);
    let mut restored = cabinet_rack(mono_ir(ir));
    restored.module_mut(0).set_param(LOW_CUT, 150.0);
    restored.module_mut(0).set_param(HIGH_CUT, 4000.0);
    restored.process(&input[..BLOCK], &mut [0.0; BLOCK], &mut [0.0; BLOCK]);
    restored.module_mut(0).reset();
    restored.module_mut(0).set_param(LOW_CUT, 20.0);
    restored.module_mut(0).set_param(HIGH_CUT, 20_000.0);
    let (left, right) = run(&mut restored, &input, BLOCK);
    assert_eq!(left, fresh_left);
    assert_eq!(right, fresh_right);
}

#[test]
fn low_cut_attenuates_sub_bass() {
    let sub_bass = tone_gain_db((LOW_CUT, 100.0), 50.0);
    assert!(sub_bass <= -10.0, "50 Hz {sub_bass} dB");
    let mid = tone_gain_db((LOW_CUT, 100.0), 1000.0);
    assert!(mid.abs() <= 0.5, "1 kHz {mid} dB");
}

#[test]
fn high_cut_attenuates_treble() {
    let treble = tone_gain_db((HIGH_CUT, 5000.0), 10_000.0);
    assert!(treble <= -10.0, "10 kHz {treble} dB");
    let mid = tone_gain_db((HIGH_CUT, 5000.0), 1000.0);
    assert!(mid.abs() <= 0.5, "1 kHz {mid} dB");
}

#[test]
fn normalise_unit_energy() {
    let left = noise(500, 41);
    let right = noise(500, 43);
    let mut ir = CabinetIr {
        left: left.clone(),
        right: Some(right.clone()),
    };
    ir.normalise();
    let scaled = ir.right.clone().expect("right kept");
    let mean = (energy(&ir.left) + energy(&scaled)) / 2.0;
    assert!((mean - 1.0).abs() <= 1e-4, "{mean}");
    let factor = ir.left[0] / left[0];
    for (n, (s, r)) in scaled.iter().zip(&right).enumerate() {
        assert!(
            (s - r * factor).abs() <= 1e-6,
            "sample {n}: {s} vs {}",
            r * factor
        );
    }
}

#[test]
fn normalise_zero_ir_is_noop() {
    let mut ir = CabinetIr {
        left: vec![0.0; 64],
        right: Some(vec![0.0; 64]),
    };
    let before = ir.clone();
    ir.normalise();
    assert_eq!(ir, before);
}

#[test]
fn cabinet_level_ramps() {
    const SMOOTHING_FRAMES: f32 = 0.005 * SAMPLE_RATE;
    let mut rack = cabinet_rack(None);
    rack.module_mut(0).set_param(0, 20.0);
    let (left, right) = run(&mut rack, &vec![1.0; 4800], BLOCK);
    let limit = (10.0 - 1.0) / SMOOTHING_FRAMES;
    for channel in [&left, &right] {
        let steepest = channel
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max);
        assert!(steepest <= limit, "step {steepest} above {limit}");
        assert!(channel[0] < 1.1, "first sample {}", channel[0]);
        assert!((channel[4799] - 10.0).abs() <= 1e-3, "{}", channel[4799]);
    }
}
