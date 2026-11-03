use assert_no_alloc::*;
use xguitare_dsp::{Drive, Gain, Gate, Module, Preamp, Rack, Tonestack};

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const BLOCK: usize = 128;

#[test]
fn gain_rack_process_does_not_allocate() {
    let mut rack = Rack::new(vec![(
        "gain".to_string(),
        Box::new(Gain::new()) as Box<dyn Module>,
    )])
    .expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        rack.module_mut(0).set_param(0, 6.0);
        for _ in 0..100 {
            rack.process(&input, &mut left, &mut right);
        }
    });
}

fn assert_rack_does_not_allocate(module: Box<dyn Module>, settings: &[(usize, f32)]) {
    let mut rack = Rack::new(vec![("module".to_string(), module)]).expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for &(idx, value) in settings {
            rack.module_mut(0).set_param(idx, value);
        }
        for _ in 0..100 {
            rack.process(&input, &mut left, &mut right);
        }
    });
}

#[test]
fn gate_does_not_allocate() {
    assert_rack_does_not_allocate(Box::new(Gate::new()), &[(0, -40.0)]);
}

#[test]
fn drive_does_not_allocate() {
    assert_rack_does_not_allocate(Box::new(Drive::new()), &[(0, 80.0)]);
}

#[test]
fn preamp_does_not_allocate() {
    assert_rack_does_not_allocate(
        Box::new(Preamp::new()),
        &[(0, 80.0), (1, 0.4), (2, -6.0), (3, 2.0)],
    );
}

#[test]
fn tonestack_does_not_allocate() {
    assert_rack_does_not_allocate(
        Box::new(Tonestack::new()),
        &[(0, 70.0), (1, 30.0), (2, 90.0)],
    );
}
