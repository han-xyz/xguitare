use std::sync::Arc;
use std::sync::atomic::Ordering;

use xguitare_core::engine::{
    Command, EngineQueues, EngineShared, FadeLengths, FromAudio, RackSwitcher, SWITCHER_MAX_FRAMES,
    engine_queues,
};
use xguitare_dsp::{Gain, Module, Rack};

const BLOCK: usize = 128;
const FADE: usize = 960;
const CAPACITY: usize = 64;

fn fades() -> FadeLengths {
    FadeLengths::at(48_000.0, 20.0)
}

fn gain_rack(levels_db: &[f32]) -> Box<Rack> {
    sized_gain_rack(levels_db, BLOCK)
}

fn sized_gain_rack(levels_db: &[f32], max_frames: usize) -> Box<Rack> {
    let modules = levels_db
        .iter()
        .enumerate()
        .map(|(idx, &level)| {
            let mut gain = Gain::new();
            gain.set_param(0, level);
            (format!("gain{idx}"), Box::new(gain) as Box<dyn Module>)
        })
        .collect();
    let mut rack = Rack::new(modules).expect("valid layout");
    rack.prepare(48_000.0, max_frames);
    for idx in 0..levels_db.len() {
        rack.module_mut(idx).reset();
    }
    Box::new(rack)
}

fn switcher(levels_db: &[f32]) -> (RackSwitcher, EngineQueues) {
    let (control, audio, _) = engine_queues(CAPACITY);
    (
        RackSwitcher::new(gain_rack(levels_db), BLOCK, fades(), audio, Arc::default()),
        control,
    )
}

fn block(switcher: &mut RackSwitcher) -> [f32; BLOCK] {
    let input = [0.5; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    switcher.apply_commands();
    switcher.process(&input, &mut left, &mut right);
    left
}

fn swap(control: &mut EngineQueues, rack: Box<Rack>) {
    control.drain_from_audio();
    assert!(control.commands.push(Command::SwapRack(rack)).is_ok());
}

#[test]
fn swap_mid_fade_crossfades() {
    let (mut switcher, mut control) = switcher(&[0.0]);
    swap(&mut control, gain_rack(&[-6.0]));
    let mut out = block(&mut switcher).to_vec();
    for level in [-12.0, -20.0] {
        assert!(
            control
                .commands
                .push(Command::SwapRack(gain_rack(&[level])))
                .is_ok()
        );
    }
    out.extend(block(&mut switcher));
    let Ok(FromAudio::Retired(replaced)) = control.from_audio.pop() else {
        panic!("expected the replaced pending Rack");
    };
    assert_eq!(replaced.modules()[0].1.param(0), -12.0);
    assert!(control.from_audio.pop().is_err());
    for _ in 0..2 * FADE.div_ceil(BLOCK) + 2 {
        out.extend(block(&mut switcher));
    }
    let largest_step = 0.5 / FADE as f32 + 1e-6;
    for (n, pair) in out.windows(2).enumerate() {
        let step = (pair[1] - pair[0]).abs();
        assert!(step <= largest_step, "frame {n}: step {step}");
    }
    let last = out[out.len() - 1];
    assert!((last - 0.05).abs() < 1e-6, "{last}");
}

#[test]
fn set_param_goes_to_newest() {
    let (mut switcher, mut control) = switcher(&[0.0]);
    swap(&mut control, gain_rack(&[0.0, 0.0]));
    block(&mut switcher);
    assert!(
        control
            .commands
            .push(Command::SetParam {
                module: 1,
                param: 0,
                value: -20.0
            })
            .is_ok()
    );
    for _ in 0..FADE.div_ceil(BLOCK) + 20 {
        block(&mut switcher);
    }
    let out = block(&mut switcher);
    assert!((out[0] - 0.05).abs() < 1e-4, "{}", out[0]);
    assert_eq!(control.from_audio.slots(), 1);
}

#[test]
fn retired_capacity_never_exceeded() {
    let (mut switcher, mut control) = switcher(&[0.0]);
    swap(&mut control, gain_rack(&[0.0]));
    block(&mut switcher);
    for round in 0..3 {
        while control.commands.slots() > 0 {
            swap(&mut control, gain_rack(&[round as f32]));
        }
        block(&mut switcher);
        let retired = control.from_audio.slots();
        assert!(
            retired <= control.from_audio.buffer().capacity(),
            "round {round}: {retired} retired"
        );
    }
}

#[test]
fn swap_to_larger_rack_during_oversize_period() {
    const LARGER: usize = 2 * BLOCK;
    let (mut control, audio, _) = engine_queues(CAPACITY);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(
        gain_rack(&[0.0]),
        SWITCHER_MAX_FRAMES,
        fades(),
        audio,
        Arc::clone(&shared),
    );
    assert_eq!(shared.running_frames.load(Ordering::Relaxed), BLOCK);
    let input = [0.5; LARGER];
    let (mut left, mut right) = ([1.0; LARGER], [1.0; LARGER]);
    switcher.apply_commands();
    assert!(!switcher.process(&input, &mut left, &mut right));
    assert!(left.iter().chain(&right).all(|&sample| sample == 0.0));
    swap(&mut control, sized_gain_rack(&[0.0], LARGER));
    switcher.apply_commands();
    assert!(switcher.process(&input, &mut left, &mut right));
    assert!(left[LARGER - 1] > 0.4, "{}", left[LARGER - 1]);
    assert!(right[LARGER - 1] > 0.4, "{}", right[LARGER - 1]);
    assert_eq!(shared.running_frames.load(Ordering::Relaxed), LARGER);
    assert!(matches!(
        control.from_audio.pop(),
        Ok(FromAudio::Retired(_))
    ));
}

#[test]
fn grown_block_mid_fade_cuts_to_newest() {
    const LARGER: usize = 2 * BLOCK;
    let (mut control, audio, _) = engine_queues(CAPACITY);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(
        sized_gain_rack(&[0.0], LARGER),
        SWITCHER_MAX_FRAMES,
        fades(),
        audio,
        Arc::clone(&shared),
    );
    swap(&mut control, gain_rack(&[-6.0]));
    block(&mut switcher);
    swap(&mut control, sized_gain_rack(&[-20.0], LARGER));
    let input = [0.5; LARGER];
    let (mut left, mut right) = ([0.0; LARGER], [0.0; LARGER]);
    switcher.apply_commands();
    assert!(switcher.process(&input, &mut left, &mut right));
    assert!((left[0] - 0.05).abs() < 1e-6, "{}", left[0]);
    assert_eq!(shared.running_frames.load(Ordering::Relaxed), LARGER);
}

#[test]
fn grown_block_cuts_to_fitting_incoming_and_keeps_pending() {
    const LARGER: usize = 2 * BLOCK;
    let (mut control, audio, _) = engine_queues(CAPACITY);
    let mut switcher = RackSwitcher::new(
        gain_rack(&[0.0]),
        SWITCHER_MAX_FRAMES,
        fades(),
        audio,
        Arc::default(),
    );
    swap(&mut control, sized_gain_rack(&[-6.0], LARGER));
    block(&mut switcher);
    swap(&mut control, gain_rack(&[-20.0]));
    let input = [0.5; LARGER];
    let (mut left, mut right) = ([0.0; LARGER], [0.0; LARGER]);
    switcher.apply_commands();
    assert!(switcher.process(&input, &mut left, &mut right));
    assert!(
        (left[0] - 0.5 * 10f32.powf(-6.0 / 20.0)).abs() < 1e-4,
        "{}",
        left[0]
    );
    let mut out = [0.0; BLOCK];
    for _ in 0..2 * FADE.div_ceil(BLOCK) + 2 {
        out = block(&mut switcher);
    }
    assert!((out[BLOCK - 1] - 0.05).abs() < 1e-6, "{}", out[BLOCK - 1]);
}
