use assert_no_alloc::*;
use xguitare_core::engine::{Command, RackSwitcher, engine_queues};
use xguitare_dsp::{Gain, Module, Rack};

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const BLOCK: usize = 128;

fn gain_rack() -> Box<Rack> {
    let mut rack = Rack::new(vec![(
        "gain".to_string(),
        Box::new(Gain::new()) as Box<dyn Module>,
    )])
    .expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    Box::new(rack)
}

#[test]
fn rack_switcher_does_not_allocate() {
    let (mut control, audio) = engine_queues(64);
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, 960, audio);
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
