mod common;

use common::{BLOCK, SAMPLE_RATE, sine};
use xguitare_dsp::crossfade::Crossfader;
use xguitare_dsp::{Gain, Module, Rack};

fn gain_rack(level_db: f32) -> Rack {
    let mut gain = Gain::new();
    gain.set_param(0, level_db);
    let mut rack = Rack::new(vec![(
        "gain".to_string(),
        Box::new(gain) as Box<dyn Module>,
    )])
    .expect("valid layout");
    rack.prepare(SAMPLE_RATE, BLOCK);
    rack.module_mut(0).reset();
    rack
}

#[test]
fn identical_racks_sum_to_input() {
    let frames = 960;
    let mut old = gain_rack(0.0);
    let mut new = gain_rack(0.0);
    let mut fader = Crossfader::new();
    fader.prepare(BLOCK);
    fader.start(frames);
    let input = sine(0.8, 220.0, SAMPLE_RATE, 2 * frames);
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    for (block, chunk) in input.chunks(BLOCK).enumerate() {
        fader.process(&mut old, &mut new, chunk, &mut left, &mut right);
        for (i, (&x, &y)) in chunk.iter().zip(&left).enumerate() {
            let n = block * BLOCK + i;
            assert!((y - x).abs() < 1e-4, "frame {n}: {y} vs input {x}");
        }
    }
}

#[test]
fn finishes_after_frames() {
    for frames in [960, 1024] {
        let mut old = gain_rack(0.0);
        let mut new = gain_rack(0.0);
        let mut fader = Crossfader::new();
        fader.prepare(BLOCK);
        fader.start(frames);
        let input = [0.5; BLOCK];
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        let last_block = frames / BLOCK;
        for block in 0..last_block {
            assert!(fader.active());
            assert!(
                !fader.process(&mut old, &mut new, &input, &mut left, &mut right),
                "{frames} frames: finished early in block {block}"
            );
        }
        assert!(fader.process(&mut old, &mut new, &input, &mut left, &mut right));
        assert!(!fader.active());
    }
}
