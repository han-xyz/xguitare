use assert_no_alloc::*;
use xguitare_dsp::modules::cabinet::{Cabinet, CabinetIr};
use xguitare_dsp::{Delay, Drive, Gain, Gate, Module, Preamp, Rack, Reverb, Tonestack};

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

#[test]
fn cabinet_does_not_allocate() {
    let ir = CabinetIr {
        left: (0..4800).map(|n| 0.9_f32.powi(n)).collect(),
        right: Some(vec![0.5; 4800]),
    };
    assert_rack_does_not_allocate(Box::new(Cabinet::new(Some(ir))), &[(0, -6.0)]);
}

fn assert_after_cabinet_does_not_allocate(module: Box<dyn Module>, settings: &[(usize, f32)]) {
    let mut rack = Rack::new(vec![
        (
            "cabinet".to_string(),
            Box::new(Cabinet::new(None)) as Box<dyn Module>,
        ),
        ("module".to_string(), module),
    ])
    .expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for &(idx, value) in settings {
            rack.module_mut(1).set_param(idx, value);
        }
        for _ in 0..100 {
            rack.process(&input, &mut left, &mut right);
        }
    });
}

#[test]
fn delay_does_not_allocate() {
    assert_after_cabinet_does_not_allocate(
        Box::new(Delay::new()),
        &[(0, 120.0), (1, 60.0), (2, 50.0), (3, 1.0)],
    );
}

#[test]
fn reverb_does_not_allocate() {
    assert_after_cabinet_does_not_allocate(
        Box::new(Reverb::new()),
        &[(0, 80.0), (1, 30.0), (2, 40.0), (3, 50.0)],
    );
}

#[test]
fn full_chain_does_not_allocate() {
    let modules: Vec<Box<dyn Module>> = vec![
        Box::new(Gate::new()),
        Box::new(Drive::new()),
        Box::new(Preamp::new()),
        Box::new(Tonestack::new()),
        Box::new(Cabinet::new(None)),
        Box::new(Delay::new()),
        Box::new(Reverb::new()),
    ];
    let mut rack = Rack::new(
        modules
            .into_iter()
            .map(|module| (module.spec().type_name.to_string(), module))
            .collect(),
    )
    .expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for idx in 0..rack.modules().len() {
            let module = rack.module_mut(idx);
            let spec = &module.spec().params[0];
            module.set_param(0, (spec.min + spec.max) / 2.0);
        }
        for _ in 0..100 {
            rack.process(&input, &mut left, &mut right);
        }
    });
}
