use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use xguitare_dsp::modules::cabinet::{Cabinet, CabinetIr};
use xguitare_dsp::{AudioBuffers, Channels, Delay, Gain, Module, ModuleSpec, Rack, RackError};

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK: usize = 128;

fn gain_rack() -> Rack {
    let mut rack = Rack::new(vec![(
        "gain".to_string(),
        Box::new(Gain::new()) as Box<dyn Module>,
    )])
    .expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    rack
}

fn run(rack: &mut Rack, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    for ((block, l), r) in input
        .chunks(BLOCK)
        .zip(left.chunks_mut(BLOCK))
        .zip(right.chunks_mut(BLOCK))
    {
        rack.process(block, l, r);
    }
    (left, right)
}

fn sine(amplitude: f32, freq: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| amplitude * (TAU * freq * n as f32 / SAMPLE_RATE).sin())
        .collect()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn silence_in_silence_out() {
    let mut rack = gain_rack();
    let (left, right) = run(&mut rack, &vec![0.0; SAMPLE_RATE as usize]);
    assert!(left.iter().chain(&right).all(|&s| s == 0.0));
}

#[test]
fn gain_plus_6db_doubles() {
    let mut rack = gain_rack();
    rack.module_mut(0).set_param(0, 6.0206);
    let input = sine(0.25, 220.0, SAMPLE_RATE as usize);
    let (left, right) = run(&mut rack, &input);
    let settled = (0.05 * SAMPLE_RATE) as usize;
    let ratio = peak(&left[settled..]) / peak(&input[settled..]);
    assert!((ratio - 2.0).abs() < 1e-3, "ratio {ratio}");
    assert_eq!(left, right);
}

#[test]
fn no_cabinet_duplicates_mono() {
    let mut rack = gain_rack();
    let (left, right) = run(&mut rack, &sine(0.5, 440.0, 4_800));
    assert_eq!(left, right);
}

struct StubModule {
    channels: Channels,
}

static MONO_SPEC: ModuleSpec = ModuleSpec {
    type_name: "mono",
    channels: Channels::Mono,
    params: &[],
};
static STEREO_SPEC: ModuleSpec = ModuleSpec {
    type_name: "stereo",
    channels: Channels::Stereo,
    params: &[],
};
static CABINET_SPEC: ModuleSpec = ModuleSpec {
    type_name: "cabinet",
    channels: Channels::MonoToStereo,
    params: &[],
};

impl Module for StubModule {
    fn spec(&self) -> &'static ModuleSpec {
        match self.channels {
            Channels::Mono => &MONO_SPEC,
            Channels::Stereo => &STEREO_SPEC,
            Channels::MonoToStereo => &CABINET_SPEC,
        }
    }
    fn prepare(&mut self, _sample_rate: f32, _max_frames: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _idx: usize, _value: f32) {}
    fn param(&self, _idx: usize) -> f32 {
        0.0
    }
    fn process(&mut self, io: &mut AudioBuffers) {
        if let Channels::MonoToStereo = self.channels {
            io.left[..io.frames].copy_from_slice(&io.mono[..io.frames]);
            io.right[..io.frames].copy_from_slice(&io.mono[..io.frames]);
        }
    }
}

fn layout_error(channels: &[(&str, Channels)]) -> RackError {
    let modules = channels
        .iter()
        .map(|(id, c)| {
            let module: Box<dyn Module> = Box::new(StubModule { channels: *c });
            (id.to_string(), module)
        })
        .collect();
    match Rack::new(modules) {
        Ok(_) => panic!("layout accepted"),
        Err(e) => e,
    }
}

#[test]
fn stereo_before_cabinet_is_error() {
    let error = layout_error(&[("verb", Channels::Stereo), ("cab", Channels::MonoToStereo)]);
    assert_eq!(error.to_string(), "stereo module 'verb' before the cabinet");
}

#[test]
fn mono_after_cabinet_is_error() {
    let error = layout_error(&[("cab", Channels::MonoToStereo), ("drive", Channels::Mono)]);
    assert_eq!(error.to_string(), "mono module 'drive' after the cabinet");
}

#[test]
fn two_cabinets_is_error() {
    let error = layout_error(&[
        ("cab1", Channels::MonoToStereo),
        ("cab2", Channels::MonoToStereo),
    ]);
    assert_eq!(error.to_string(), "more than one cabinet");
}

#[test]
#[should_panic(expected = "exceed max_frames")]
#[cfg(debug_assertions)]
fn process_more_than_max_frames_panics() {
    let mut rack = gain_rack();
    let input = [0.0; BLOCK + 1];
    let mut left = [0.0; BLOCK + 1];
    let mut right = [0.0; BLOCK + 1];
    rack.process(&input, &mut left, &mut right);
}

#[test]
fn max_frames_accessor() {
    let mut rack = Rack::new(Vec::new()).expect("valid layout");
    rack.prepare(SAMPLE_RATE, 256);
    assert_eq!(rack.max_frames(), 256);
    assert_eq!(gain_rack().max_frames(), BLOCK);
}

const BYPASS_FADE_FRAMES: usize = 480;

#[derive(Default)]
struct Counts {
    processed: AtomicUsize,
    resets: AtomicUsize,
}

struct CountingModule(Arc<Counts>);

impl Module for CountingModule {
    fn spec(&self) -> &'static ModuleSpec {
        &MONO_SPEC
    }
    fn prepare(&mut self, _sample_rate: f32, _max_frames: usize) {}
    fn reset(&mut self) {
        self.0.resets.fetch_add(1, Ordering::Relaxed);
    }
    fn set_param(&mut self, _idx: usize, _value: f32) {}
    fn param(&self, _idx: usize) -> f32 {
        0.0
    }
    fn process(&mut self, io: &mut AudioBuffers) {
        self.0.processed.fetch_add(1, Ordering::Relaxed);
        io.mono[..io.frames].fill(0.0);
    }
}

fn counting_rack() -> (Rack, Arc<Counts>) {
    let counts = Arc::new(Counts::default());
    let module: Box<dyn Module> = Box::new(CountingModule(counts.clone()));
    let mut rack = Rack::new(vec![("count".to_string(), module)]).expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    (rack, counts)
}

fn test_ir() -> CabinetIr {
    CabinetIr {
        left: (0..480).map(|n| 0.5 * 0.99_f32.powi(n)).collect(),
        right: None,
    }
}

#[test]
fn bypassed_module_output_equals_input_after_the_fade() {
    let mut rack = gain_rack();
    rack.module_mut(0).set_param(0, 6.0);
    rack.set_bypass(0, true);
    let input = sine(0.25, 220.0, 4_800);
    let (left, right) = run(&mut rack, &input);
    assert_eq!(left[BYPASS_FADE_FRAMES..], input[BYPASS_FADE_FRAMES..]);
    assert_eq!(left, right);
}

#[test]
fn bypassed_cabinet_sends_its_input_to_both_outputs() {
    let module: Box<dyn Module> = Box::new(Cabinet::new(Some(test_ir())));
    let mut rack = Rack::new(vec![("cabinet".to_string(), module)]).expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    rack.set_bypass(0, true);
    let input = sine(0.25, 220.0, 4_800);
    let (left, right) = run(&mut rack, &input);
    assert_eq!(left[BYPASS_FADE_FRAMES..], input[BYPASS_FADE_FRAMES..]);
    assert_eq!(right[BYPASS_FADE_FRAMES..], input[BYPASS_FADE_FRAMES..]);
}

#[test]
fn bypassed_stereo_module_passes_left_and_right() {
    let cabinet_rack = |modules: Vec<(String, Box<dyn Module>)>| {
        let mut all: Vec<(String, Box<dyn Module>)> = vec![(
            "cabinet".to_string(),
            Box::new(Cabinet::new(Some(test_ir()))),
        )];
        all.extend(modules);
        let mut rack = Rack::new(all).expect("valid layout");
        rack.prepare(SAMPLE_RATE, BLOCK);
        rack
    };
    let mut with_delay = cabinet_rack(vec![("delay".to_string(), Box::new(Delay::new()))]);
    with_delay.set_bypass(1, true);
    let input = sine(0.25, 220.0, 4_800);
    let (left, right) = run(&mut with_delay, &input);
    let (cab_left, cab_right) = run(&mut cabinet_rack(Vec::new()), &input);
    assert_eq!(left[BYPASS_FADE_FRAMES..], cab_left[BYPASS_FADE_FRAMES..]);
    assert_eq!(right[BYPASS_FADE_FRAMES..], cab_right[BYPASS_FADE_FRAMES..]);
}

#[test]
fn bypass_fade_has_no_step_above_its_own_slope() {
    let (amplitude, freq) = (0.5, 100.0);
    let input = sine(amplitude, freq, 9_600);
    let sine_slope = amplitude * TAU * freq / SAMPLE_RATE;
    let fade_slope = amplitude / BYPASS_FADE_FRAMES as f32;
    let (mut rack, _) = counting_rack();
    let mut output = Vec::new();
    for (n, block) in input.chunks(BLOCK).enumerate() {
        match n {
            10 => rack.set_bypass(0, true),
            40 => rack.set_bypass(0, false),
            _ => {}
        }
        let (left, _) = run(&mut rack, block);
        output.extend(left);
    }
    let largest_step = output
        .windows(2)
        .fold(0.0_f32, |m, pair| m.max((pair[1] - pair[0]).abs()));
    assert!(largest_step > 0.0, "the sine reached the output");
    assert!(
        largest_step <= sine_slope + fade_slope + 1e-6,
        "step {largest_step} above {}",
        sine_slope + fade_slope
    );
}

#[test]
fn bypassed_module_is_not_processed_after_the_fade() {
    let (mut rack, counts) = counting_rack();
    rack.set_bypass(0, true);
    run(&mut rack, &[0.0; BYPASS_FADE_FRAMES + BLOCK]);
    let processed = counts.processed.load(Ordering::Relaxed);
    run(&mut rack, &[0.0; 10 * BLOCK]);
    assert_eq!(counts.processed.load(Ordering::Relaxed), processed);
}

#[test]
fn module_back_from_bypass_is_reset_first() {
    let (mut rack, counts) = counting_rack();
    rack.set_bypass(0, true);
    run(&mut rack, &[0.0; BYPASS_FADE_FRAMES + BLOCK]);
    rack.set_bypass(0, false);
    assert_eq!(counts.resets.load(Ordering::Relaxed), 1);
    let processed = counts.processed.load(Ordering::Relaxed);
    run(&mut rack, &[0.0; BLOCK]);
    assert_eq!(counts.processed.load(Ordering::Relaxed), processed + 1);
}

#[test]
fn started_bypassed_module_passes_input_from_the_first_frame() {
    let (mut rack, counts) = counting_rack();
    rack.start_bypassed(0);
    let input = sine(0.25, 220.0, 4 * BLOCK);
    let (left, _) = run(&mut rack, &input);
    assert_eq!(left, input);
    assert_eq!(counts.processed.load(Ordering::Relaxed), 0);
}
