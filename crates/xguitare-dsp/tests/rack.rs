use std::f32::consts::TAU;

use xguitare_dsp::{AudioBuffers, Channels, Gain, Module, ModuleSpec, Rack, RackError};

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
