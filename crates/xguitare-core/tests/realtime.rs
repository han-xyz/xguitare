use std::sync::Arc;

use assert_no_alloc::*;
use xguitare_core::engine::{Command, FromAudio, RackSwitcher, SWITCHER_MAX_FRAMES, engine_queues};
use xguitare_dsp::{Gain, Module, Rack};

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const BLOCK: usize = 128;

fn gain_rack() -> Box<Rack> {
    sized_gain_rack(BLOCK)
}

fn sized_gain_rack(max_frames: usize) -> Box<Rack> {
    let mut rack = Rack::new(vec![(
        "gain".to_string(),
        Box::new(Gain::new()) as Box<dyn Module>,
    )])
    .expect("valid layout");
    rack.prepare(48_000.0, max_frames);
    Box::new(rack)
}

#[test]
fn rack_switcher_does_not_allocate() {
    let (mut control, audio) = engine_queues(64);
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, 960, audio, Arc::default());
    for _ in 0..2 {
        assert!(
            control
                .commands
                .push(Command::SwapRack(gain_rack()))
                .is_ok()
        );
    }
    let set = Command::SetParam {
        module: 0,
        param: 0,
        value: -6.0,
    };
    assert!(control.commands.push(set).is_ok());
    assert!(
        control
            .commands
            .push(Command::SwapRack(gain_rack()))
            .is_ok()
    );
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for _ in 0..20 {
            switcher.push_program_change(3);
            switcher.apply_commands();
            switcher.process(&input, &mut left, &mut right);
        }
    });
    drop(switcher);
    control.drain_from_audio();
}

#[test]
fn rack_switcher_at_quantum_limit_does_not_allocate() {
    let (mut control, audio) = engine_queues(64);
    let mut switcher = RackSwitcher::new(
        sized_gain_rack(SWITCHER_MAX_FRAMES),
        SWITCHER_MAX_FRAMES,
        960,
        audio,
        Arc::default(),
    );
    assert!(
        control
            .commands
            .push(Command::SwapRack(sized_gain_rack(SWITCHER_MAX_FRAMES)))
            .is_ok()
    );
    let input = vec![0.25; SWITCHER_MAX_FRAMES];
    let mut left = vec![0.0; SWITCHER_MAX_FRAMES];
    let mut right = vec![0.0; SWITCHER_MAX_FRAMES];
    assert_no_alloc(|| {
        for _ in 0..20 {
            switcher.apply_commands();
            assert!(switcher.process(&input, &mut left, &mut right));
        }
    });
    drop(switcher);
    control.drain_from_audio();
}

#[test]
fn oversize_then_swap_and_cut_does_not_allocate() {
    const LARGER: usize = 2 * BLOCK;
    let (mut control, audio) = engine_queues(64);
    let mut switcher = RackSwitcher::new(
        sized_gain_rack(BLOCK),
        SWITCHER_MAX_FRAMES,
        960,
        audio,
        Arc::default(),
    );
    let larger = sized_gain_rack(LARGER);
    let input = [0.25; LARGER];
    let mut left = [0.0; LARGER];
    let mut right = [0.0; LARGER];
    let processed = assert_no_alloc(|| {
        switcher.apply_commands();
        let silent = !switcher.process(&input, &mut left, &mut right);
        assert!(control.commands.push(Command::SwapRack(larger)).is_ok());
        switcher.apply_commands();
        silent && switcher.process(&input, &mut left, &mut right)
    });
    assert!(processed);
    assert!(matches!(
        control.from_audio.pop(),
        Ok(FromAudio::Retired(_))
    ));
    drop(switcher);
    control.drain_from_audio();
}
