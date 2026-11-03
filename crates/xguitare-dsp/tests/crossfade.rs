mod common;

use common::{BLOCK, SAMPLE_RATE, db};
use xguitare_dsp::crossfade::Crossfader;
use xguitare_dsp::{Gain, Module, Rack};

const SILENT_DB: f32 = -60.0;

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

fn fade_gains(old_db: f32, new_db: f32, frames: usize) -> Vec<f32> {
    let mut old = gain_rack(old_db);
    let mut new = gain_rack(new_db);
    let mut fader = Crossfader::new();
    fader.prepare(BLOCK);
    fader.start(frames);
    let input = [0.5; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let mut gains = Vec::new();
    for _ in 0..frames.div_ceil(BLOCK) {
        fader.process(&mut old, &mut new, &input, &mut left, &mut right);
        gains.extend(left.iter().map(|s| s / 0.5));
    }
    gains.truncate(frames);
    gains
}

#[test]
fn equal_power() {
    let frames = 960;
    let old_gains = fade_gains(0.0, SILENT_DB, frames);
    let new_gains = fade_gains(SILENT_DB, 0.0, frames);
    for (n, (g_old, g_new)) in old_gains.iter().zip(&new_gains).enumerate() {
        let power_db = 10.0 * (g_old * g_old + g_new * g_new).log10();
        assert!(power_db.abs() < 0.1, "frame {n}: power {power_db} dB");
    }
    assert!(db(old_gains[0]) > -0.1 && new_gains[0] < 0.01);
    assert!(db(new_gains[frames - 1]) > -0.1 && old_gains[frames - 1] < 0.01);
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
