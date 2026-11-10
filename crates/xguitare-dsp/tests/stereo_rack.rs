mod common;

use common::{BLOCK, SAMPLE_RATE, frames, peak, prepared, run_stereo, sine};
use xguitare_dsp::modules::cabinet::Cabinet;
use xguitare_dsp::{
    AudioBuffers, Compressor, Delay, Drive, Gain, Module, Reverb, StereoRack, StereoSlot,
};

const GAIN_6DB: f32 = 6.0206;

fn stereo_rack(slots: Vec<(&str, StereoSlot)>) -> StereoRack {
    let mut rack = StereoRack::new(
        slots
            .into_iter()
            .map(|(id, slot)| (id.to_string(), slot))
            .collect(),
    )
    .expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    rack
}

fn run(rack: &mut StereoRack, in_l: &[f32], in_r: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut out_l = vec![0.0; in_l.len()];
    let mut out_r = vec![0.0; in_r.len()];
    for (((l, r), ol), or) in in_l
        .chunks(BLOCK)
        .zip(in_r.chunks(BLOCK))
        .zip(out_l.chunks_mut(BLOCK))
        .zip(out_r.chunks_mut(BLOCK))
    {
        rack.process(l, r, ol, or);
    }
    (out_l, out_r)
}

fn run_mono(module: &mut dyn Module, input: &[f32]) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    for block in output.chunks_mut(BLOCK) {
        module.process(&mut AudioBuffers {
            frames: block.len(),
            mono: block,
            left: &mut left,
            right: &mut right,
        });
    }
    output
}

fn layout_error(slots: Vec<(&str, StereoSlot)>) -> String {
    match StereoRack::new(
        slots
            .into_iter()
            .map(|(id, slot)| (id.to_string(), slot))
            .collect(),
    ) {
        Ok(_) => panic!("layout accepted"),
        Err(e) => e.to_string(),
    }
}

fn delay() -> Delay {
    prepared(Delay::new(), &[(0, 30.0), (1, 50.0), (2, 50.0)])
}

#[test]
fn stereo_module_output_equals_module_alone() {
    let in_l = sine(0.5, 220.0, SAMPLE_RATE, frames(200.0));
    let in_r = sine(0.3, 330.0, SAMPLE_RATE, frames(200.0));
    let mut rack = stereo_rack(vec![("delay", StereoSlot::Stereo(Box::new(delay())))]);
    let (out_l, out_r) = run(&mut rack, &in_l, &in_r);

    let mut alone = delay();
    let (mut want_l, mut want_r) = (in_l.clone(), in_r.clone());
    run_stereo(&mut alone, &mut want_l, &mut want_r);
    assert_eq!(out_l, want_l);
    assert_eq!(out_r, want_r);
}

#[test]
fn doubled_mono_module_runs_each_side_alone() {
    let drive = || prepared(Drive::new(), &[(0, 80.0)]);
    let in_l = sine(0.5, 220.0, SAMPLE_RATE, frames(100.0));
    let in_r = vec![0.0; in_l.len()];
    let mut rack = stereo_rack(vec![(
        "drive",
        StereoSlot::Doubled(Box::new(drive()), Box::new(drive())),
    )]);
    let (out_l, out_r) = run(&mut rack, &in_l, &in_r);

    assert_eq!(out_l, run_mono(&mut drive(), &in_l));
    assert_eq!(out_r, run_mono(&mut drive(), &in_r));
}

#[test]
fn param_reaches_both_instances() {
    let mut rack = stereo_rack(vec![(
        "gain",
        StereoSlot::Doubled(Box::new(Gain::new()), Box::new(Gain::new())),
    )]);
    rack.set_param(0, 0, GAIN_6DB);
    let input = sine(0.25, 220.0, SAMPLE_RATE, frames(200.0));
    let (out_l, out_r) = run(&mut rack, &input, &input);
    let settled = frames(50.0);
    for out in [&out_l, &out_r] {
        let ratio = peak(&out[settled..]) / peak(&input[settled..]);
        assert!((ratio - 2.0).abs() < 1e-3, "ratio {ratio}");
    }
    let (_, first) = rack.modules().next().expect("one slot");
    assert!((first.param(0) - GAIN_6DB).abs() < 1e-6);
}

fn compressor() -> Compressor {
    prepared(Compressor::new(), &[(0, -30.0), (1, 8.0), (2, 1.0)])
}

const LOUD: f32 = 0.5;
const QUIET: f32 = 0.01;

fn gains(slot: StereoSlot, left_level: f32, right_level: f32) -> (f32, f32) {
    let in_l = sine(left_level, 1_000.0, SAMPLE_RATE, frames(300.0));
    let in_r = sine(right_level, 1_000.0, SAMPLE_RATE, frames(300.0));
    let mut rack = stereo_rack(vec![("compressor", slot)]);
    let (out_l, out_r) = run(&mut rack, &in_l, &in_r);
    let window = frames(250.0)..frames(300.0);
    (
        peak(&out_l[window.clone()]) / peak(&in_l[window.clone()]),
        peak(&out_r[window.clone()]) / peak(&in_r[window]),
    )
}

fn linked() -> StereoSlot {
    StereoSlot::Linked(Box::new(compressor()))
}

#[test]
fn linked_compressor_applies_one_gain_to_both_sides() {
    let (linked_l, linked_r) = gains(linked(), LOUD, QUIET);
    assert!(linked_l < 0.5, "left gain {linked_l}");
    assert!(
        (linked_l - linked_r).abs() < 1e-3,
        "left {linked_l} right {linked_r}"
    );

    let (doubled_l, doubled_r) = gains(
        StereoSlot::Doubled(Box::new(compressor()), Box::new(compressor())),
        LOUD,
        QUIET,
    );
    assert!((doubled_l - linked_l).abs() < 1e-3, "left {doubled_l}");
    assert!(doubled_r > 0.9, "unlinked right gain {doubled_r}");
}

#[test]
fn linked_compressor_detects_the_louder_side() {
    let (loud_left, _) = gains(linked(), LOUD, QUIET);
    let (swapped_l, swapped_r) = gains(linked(), QUIET, LOUD);
    assert!(swapped_r < 0.5, "right gain {swapped_r}");
    assert!(
        (swapped_l - swapped_r).abs() < 1e-3,
        "left {swapped_l} right {swapped_r}"
    );
    assert!(
        (swapped_l - loud_left).abs() < 1e-3,
        "swapped {swapped_l} loud left {loud_left}"
    );
}

#[test]
fn reset_starts_slot_at_its_param_values() {
    let mut rack = boosted_rack();
    rack.reset(0);
    let input = sine(0.25, 220.0, SAMPLE_RATE, frames(10.0));
    let (out_l, out_r) = run(&mut rack, &input, &input);
    for out in [&out_l, &out_r] {
        let ratio = peak(&out[..BLOCK]) / peak(&input[..BLOCK]);
        assert!((ratio - 2.0).abs() < 1e-3, "ratio {ratio}");
    }
}

fn boosted_rack() -> StereoRack {
    let mut rack = stereo_rack(vec![(
        "gain",
        StereoSlot::Doubled(Box::new(Gain::new()), Box::new(Gain::new())),
    )]);
    rack.set_param(0, 0, GAIN_6DB);
    rack
}

#[test]
fn bypassed_slot_passes_input() {
    let mut rack = boosted_rack();
    let in_l = sine(0.25, 220.0, SAMPLE_RATE, frames(100.0));
    let in_r = sine(0.2, 330.0, SAMPLE_RATE, frames(100.0));
    rack.set_bypass(0, true);
    let (out_l, out_r) = run(&mut rack, &in_l, &in_r);
    let faded = frames(20.0);
    assert_eq!(out_l[faded..], in_l[faded..]);
    assert_eq!(out_r[faded..], in_r[faded..]);
}

#[test]
fn start_bypassed_slot_passes_input_from_first_frame() {
    let mut rack = boosted_rack();
    rack.start_bypassed(0);
    let in_l = sine(0.25, 220.0, SAMPLE_RATE, frames(20.0));
    let in_r = sine(0.2, 330.0, SAMPLE_RATE, frames(20.0));
    let (out_l, out_r) = run(&mut rack, &in_l, &in_r);
    assert_eq!(out_l, in_l);
    assert_eq!(out_r, in_r);
}

#[test]
fn cabinet_is_refused() {
    let error = layout_error(vec![(
        "cab",
        StereoSlot::Doubled(Box::new(Cabinet::new(None)), Box::new(Cabinet::new(None))),
    )]);
    assert_eq!(error, "a cabinet is not allowed on a Bus or Master");
    let error = layout_error(vec![(
        "cab",
        StereoSlot::Stereo(Box::new(Cabinet::new(None))),
    )]);
    assert_eq!(error, "a cabinet is not allowed on a Bus or Master");
}

#[test]
fn mono_module_in_stereo_slot_is_refused() {
    let error = layout_error(vec![("gain", StereoSlot::Stereo(Box::new(Gain::new())))]);
    assert_eq!(error, "mono module 'gain' in a stereo slot");
}

#[test]
fn stereo_module_in_doubled_or_linked_slot_is_refused() {
    let error = layout_error(vec![(
        "delay",
        StereoSlot::Doubled(Box::new(Delay::new()), Box::new(Delay::new())),
    )]);
    assert_eq!(error, "stereo module 'delay' in a doubled or linked slot");
    let error = layout_error(vec![("verb", StereoSlot::Linked(Box::new(Reverb::new())))]);
    assert_eq!(error, "stereo module 'verb' in a doubled or linked slot");
}

#[test]
fn out_of_range_index_is_ignored() {
    let mut rack = boosted_rack();
    rack.set_bypass(1, true);
    rack.start_bypassed(1);
    rack.set_param(1, 0, 0.0);
    rack.set_param(0, 1, 0.0);
    let input = sine(0.25, 220.0, SAMPLE_RATE, frames(100.0));
    let (out_l, _) = run(&mut rack, &input, &input);
    let (want_l, _) = run(&mut boosted_rack(), &input, &input);
    assert_eq!(out_l, want_l);
}
