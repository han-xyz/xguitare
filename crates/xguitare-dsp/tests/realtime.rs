use assert_no_alloc::*;
use xguitare_dsp::crossfade::Crossfader;
use xguitare_dsp::midi::program_change;
use xguitare_dsp::modules::cabinet::{Cabinet, CabinetIr};
use xguitare_dsp::{
    Autowah, Chorus, Compressor, Delay, Drive, Eq, Flanger, Fuzz, Gain, Gate, Module, Preamp, Rack,
    Reverb, StereoRack, StereoSlot, Tonestack,
};

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
fn compressor_does_not_allocate() {
    assert_rack_does_not_allocate(
        Box::new(Compressor::new()),
        &[(0, -30.0), (1, 8.0), (2, 2.0), (3, 200.0), (4, 6.0)],
    );
}

#[test]
fn autowah_does_not_allocate() {
    assert_rack_does_not_allocate(
        Box::new(Autowah::new()),
        &[
            (0, 2.0),
            (1, 1.0),
            (2, 24.0),
            (3, 1.0),
            (4, 300.0),
            (5, 100.0),
            (6, 3000.0),
            (7, 8.0),
            (8, 20.0),
            (9, 80.0),
        ],
    );
}

#[test]
fn eq_does_not_allocate() {
    assert_rack_does_not_allocate(
        Box::new(Eq::new()),
        &[
            (0, 40.0),
            (1, 150.0),
            (2, 6.0),
            (3, 500.0),
            (4, -6.0),
            (5, 3.0),
            (6, 3000.0),
            (7, 4.0),
            (8, 1.5),
            (9, 6000.0),
            (10, -3.0),
            (11, 12000.0),
            (12, 2.0),
        ],
    );
}

#[test]
fn drive_does_not_allocate() {
    assert_rack_does_not_allocate(Box::new(Drive::new()), &[(0, 80.0)]);
}

#[test]
fn fuzz_does_not_allocate() {
    assert_rack_does_not_allocate(Box::new(Fuzz::new()), &[(0, 90.0), (1, 20.0), (2, -6.0)]);
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

#[test]
fn cabinet_cuts_do_not_allocate() {
    let ir = CabinetIr {
        left: (0..4800).map(|n| 0.9_f32.powi(n)).collect(),
        right: Some(vec![0.5; 4800]),
    };
    assert_rack_does_not_allocate(Box::new(Cabinet::new(Some(ir))), &[(1, 100.0), (2, 5000.0)]);
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
fn chorus_does_not_allocate() {
    assert_after_cabinet_does_not_allocate(
        Box::new(Chorus::new()),
        &[(0, 3.0), (1, 80.0), (2, 60.0)],
    );
}

#[test]
fn flanger_does_not_allocate() {
    assert_after_cabinet_does_not_allocate(
        Box::new(Flanger::new()),
        &[(0, 2.0), (1, 90.0), (2, -70.0), (3, 60.0)],
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
    let ir = CabinetIr {
        left: (0..4800).map(|n| 0.9_f32.powi(n)).collect(),
        right: Some(vec![0.5; 4800]),
    };
    let modules: Vec<Box<dyn Module>> = vec![
        Box::new(Gate::new()),
        Box::new(Drive::new()),
        Box::new(Preamp::new()),
        Box::new(Tonestack::new()),
        Box::new(Cabinet::new(Some(ir))),
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
            module.set_param(0, f32::midpoint(spec.min, spec.max));
        }
        for _ in 0..100 {
            rack.process(&input, &mut left, &mut right);
        }
    });
}

#[test]
fn crossfade_step_does_not_allocate() {
    let gain_rack = || {
        let mut rack = Rack::new(vec![(
            "gain".to_string(),
            Box::new(Gain::new()) as Box<dyn Module>,
        )])
        .expect("valid layout");
        rack.prepare(48_000.0, BLOCK);
        rack
    };
    let (mut old, mut new) = (gain_rack(), gain_rack());
    let mut fader = Crossfader::new();
    fader.prepare(BLOCK);
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        fader.start(960);
        while !fader.process(&mut old, &mut new, &input, &mut left, &mut right) {}
    });
}

#[test]
fn midi_parse_does_not_allocate() {
    let events: [&[u8]; 3] = [&[0xC0, 5], &[0x90, 60, 100], &[0xC0]];
    let mut programs = [None; 3];
    assert_no_alloc(|| {
        for (program, bytes) in programs.iter_mut().zip(events) {
            *program = program_change(bytes);
        }
    });
    assert_eq!(programs, [Some(5), None, None]);
}

#[test]
fn bypass_toggling_does_not_allocate() {
    let mut rack = Rack::new(vec![
        (
            "drive".to_string(),
            Box::new(Drive::new()) as Box<dyn Module>,
        ),
        (
            "cabinet".to_string(),
            Box::new(Cabinet::new(None)) as Box<dyn Module>,
        ),
        (
            "delay".to_string(),
            Box::new(Delay::new()) as Box<dyn Module>,
        ),
    ])
    .expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for round in 0..100 {
            let on = round % 10 < 5;
            for idx in 0..3 {
                rack.set_bypass(idx, on);
            }
            rack.process(&input, &mut left, &mut right);
        }
    });
}

#[test]
fn stereo_rack_does_not_allocate() {
    let mut rack = StereoRack::new(vec![
        (
            "gate".to_string(),
            StereoSlot::Doubled(Box::new(Gate::new()), Box::new(Gate::new())),
        ),
        (
            "compressor".to_string(),
            StereoSlot::Linked(Box::new(Compressor::new())),
        ),
        (
            "reverb".to_string(),
            StereoSlot::Stereo(Box::new(Reverb::new())),
        ),
    ])
    .expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    let in_l = [0.25; BLOCK];
    let in_r = [0.1; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    let param_counts: Vec<usize> = rack
        .modules()
        .map(|(_, module)| module.spec().params.len())
        .collect();
    assert_no_alloc(|| {
        for (idx, &count) in param_counts.iter().enumerate() {
            for param in 0..count {
                rack.set_param(idx, param, 0.5);
            }
        }
        for round in 0..100 {
            let on = round % 10 < 5;
            for idx in 0..3 {
                rack.set_bypass(idx, on);
            }
            rack.process(&in_l, &in_r, &mut left, &mut right);
        }
    });
}
