// Each test crate includes this module and uses only some of the helpers.
#![allow(dead_code)]

use std::f64::consts::TAU;

use xguitare_dsp::{Module, Rack};

pub const SAMPLE_RATE: f32 = 48_000.0;
pub const BLOCK: usize = 128;

pub fn sine(amplitude: f32, freq: f32, rate: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| amplitude * (TAU * f64::from(freq) * n as f64 / f64::from(rate)).sin() as f32)
        .collect()
}

pub fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

pub fn db(ratio: f32) -> f32 {
    20.0 * ratio.log10()
}

pub fn bin_mag(signal: &[f32], freq: f32, rate: f32) -> f32 {
    let (re, im) = signal
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (n, &s)| {
            let phase = TAU * f64::from(freq) * n as f64 / f64::from(rate);
            (
                re + f64::from(s) * phase.cos(),
                im - f64::from(s) * phase.sin(),
            )
        });
    (2.0 * (re * re + im * im).sqrt() / signal.len() as f64) as f32
}

pub fn rack_with(module: Box<dyn Module>) -> Rack {
    let mut rack =
        Rack::new(vec![(module.spec().type_name.to_string(), module)]).expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    rack
}

pub fn run(rack: &mut Rack, input: &[f32]) -> Vec<f32> {
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    for ((block, l), r) in input
        .chunks(BLOCK)
        .zip(left.chunks_mut(BLOCK))
        .zip(right.chunks_mut(BLOCK))
    {
        rack.process(block, l, r);
    }
    left
}

pub fn noise(amplitude: f32, frames: usize) -> Vec<f32> {
    let mut state: u32 = 0x9E37_79B9;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            amplitude * (state as f32 / u32::MAX as f32 * 2.0 - 1.0)
        })
        .collect()
}

pub fn full_scale_sine_then_noise() -> Vec<f32> {
    let second = SAMPLE_RATE as usize;
    let mut input = sine(1.0, 220.0, SAMPLE_RATE, second);
    input.extend(noise(1.0, second));
    input
}
