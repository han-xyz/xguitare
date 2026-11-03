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
