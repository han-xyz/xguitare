use assert_no_alloc::*;
use xguitare_dsp::modules::cabinet::{Cabinet, CabinetIr};
use xguitare_dsp::{Gain, Module, Rack};

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

#[test]
fn cabinet_does_not_allocate() {
    let ir = CabinetIr {
        left: (0..4800).map(|n| 0.9_f32.powi(n)).collect(),
        right: Some(vec![0.5; 4800]),
    };
    let mut rack = Rack::new(vec![(
        "cab".to_string(),
        Box::new(Cabinet::new(Some(ir))) as Box<dyn Module>,
    )])
    .expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        rack.module_mut(0).set_param(0, -6.0);
        for _ in 0..100 {
            rack.process(&input, &mut left, &mut right);
        }
    });
}
