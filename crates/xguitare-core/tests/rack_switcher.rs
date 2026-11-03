use xguitare_core::engine::{
    Command, EngineQueues, RackSwitcher, SWITCHER_MAX_FRAMES, engine_queues,
};
use xguitare_dsp::{Gain, Module, Rack};

const BLOCK: usize = 128;
const FADE: usize = 960;
const CAPACITY: usize = 64;

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
    let (control, audio) = engine_queues(CAPACITY);
    (
        RackSwitcher::new(gain_rack(levels_db), BLOCK, FADE, audio),
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
fn swap_during_fade_retires_both() {
    let (mut switcher, mut control) = switcher(&[0.0]);
    swap(&mut control, gain_rack(&[-6.0]));
    block(&mut switcher);
    assert_eq!(control.from_audio.slots(), 0);
    assert!(
        control
            .commands
            .push(Command::SwapRack(gain_rack(&[-20.0])))
            .is_ok()
    );
    let out = block(&mut switcher);
    assert_eq!(control.from_audio.slots(), 2);
    assert!((out[0] - 0.05).abs() < 1e-6, "{}", out[0]);
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
    let (mut control, audio) = engine_queues(CAPACITY);
    let mut switcher = RackSwitcher::new(gain_rack(&[0.0]), SWITCHER_MAX_FRAMES, FADE, audio);
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
}
