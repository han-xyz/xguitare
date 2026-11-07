use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use xguitare_core::control::passthrough_rack;
use xguitare_core::engine::{
    EngineShared, RackSwitcher, block_peak, engine_queues, raise_peak, take_peak,
};

const SAMPLE_RATE: f32 = 48_000.0;
const MUTE_RAMP_FRAMES: usize = 512;

#[test]
fn block_peak_is_max_abs() {
    assert_eq!(block_peak(&[0.1, -0.7, 0.3]), 0.7);
    assert_eq!(block_peak(&[0.0; 8]), 0.0);
    assert_eq!(block_peak(&[]), 0.0);
}

#[test]
fn peak_atomic_only_raises_and_swaps_to_zero() {
    let peak = AtomicU32::new(0);
    raise_peak(&peak, 0.5);
    raise_peak(&peak, 0.25);
    assert_eq!(take_peak(&peak), 0.5);
    assert_eq!(take_peak(&peak), 0.0);
}

fn passthrough_switcher(frames: usize, shared: Arc<EngineShared>) -> RackSwitcher {
    let (_, audio, _) = engine_queues(4);
    let rack = Box::new(passthrough_rack(SAMPLE_RATE, frames));
    RackSwitcher::new(rack, frames, 960, audio, shared)
}

fn sine(start: usize, frames: usize) -> Vec<f32> {
    (start..start + frames)
        .map(|n| 0.5 * (TAU * 110.0 * n as f32 / SAMPLE_RATE + 0.3).sin())
        .collect()
}

#[test]
fn tap_ring_carries_input() {
    const FRAMES: usize = 128;
    let (_control, audio, mut tap) = engine_queues(4);
    let rack = Box::new(passthrough_rack(SAMPLE_RATE, FRAMES));
    let mut switcher = RackSwitcher::new(rack, FRAMES, 960, audio, Arc::default());
    let (mut left, mut right) = ([0.0; FRAMES], [0.0; FRAMES]);
    let input = sine(0, 2 * FRAMES);
    for block in input.chunks(FRAMES) {
        switcher.process(block, &mut left, &mut right);
    }
    let mut tapped = Vec::new();
    while let Ok(sample) = tap.pop() {
        tapped.push(sample);
    }
    assert_eq!(tapped, input);
}

#[test]
fn mute_ramps_to_silence_and_back() {
    const FRAMES: usize = MUTE_RAMP_FRAMES;
    let shared = Arc::new(EngineShared::default());
    let mut switcher = passthrough_switcher(FRAMES, Arc::clone(&shared));
    let (mut left, mut right) = (vec![0.0; FRAMES], vec![0.0; FRAMES]);
    let mut start = 0;
    let mut block = |switcher: &mut RackSwitcher, left: &mut [f32], right: &mut [f32]| {
        let input = sine(start, FRAMES);
        start += FRAMES;
        assert!(switcher.process(&input, left, right));
        input
    };
    let input = block(&mut switcher, &mut left, &mut right);
    assert_eq!(left, input);

    shared.muted.store(true, Ordering::Relaxed);
    let input = block(&mut switcher, &mut left, &mut right);
    let mut previous_gain = 1.0;
    for (n, (&dry, (&out_l, &out_r))) in input.iter().zip(left.iter().zip(&right)).enumerate() {
        assert_eq!(out_l, out_r, "frame {n}");
        if dry.abs() > 1e-3 {
            let gain = out_l / dry;
            assert!(gain <= previous_gain && gain >= 0.0, "frame {n}: {gain}");
            previous_gain = gain;
        }
    }
    assert!(previous_gain < 0.01, "{previous_gain}");
    block(&mut switcher, &mut left, &mut right);
    assert!(left.iter().chain(&right).all(|&sample| sample == 0.0));

    shared.muted.store(false, Ordering::Relaxed);
    block(&mut switcher, &mut left, &mut right);
    let input = block(&mut switcher, &mut left, &mut right);
    assert_eq!(left, input);
    assert_eq!(right, input);
}
