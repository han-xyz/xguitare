use std::f32::consts::FRAC_PI_2;

use crate::Rack;

/// Equal-power crossfade from one Rack to another: the outgoing Rack's gain follows `cos(θ)` and the incoming one's `sin(θ)`, θ rising from 0 to π/2 over the fade.
pub struct Crossfader {
    new_l: Vec<f32>,
    new_r: Vec<f32>,
    position: usize,
    length: usize,
    active: bool,
}

impl Crossfader {
    pub fn new() -> Self {
        Crossfader {
            new_l: Vec::new(),
            new_r: Vec::new(),
            position: 0,
            length: 0,
            active: false,
        }
    }

    pub fn prepare(&mut self, max_frames: usize) {
        self.new_l = vec![0.0; max_frames];
        self.new_r = vec![0.0; max_frames];
    }

    pub fn start(&mut self, frames: usize) {
        self.position = 0;
        self.length = frames;
        self.active = true;
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// Runs both Racks on `input` and mixes them into `out_l`/`out_r`; returns true on the block that contains frame `frames`, the first frame at full incoming gain.
    pub fn process(
        &mut self,
        old: &mut Rack,
        new: &mut Rack,
        input: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool {
        let frames = input.len();
        old.process(input, out_l, out_r);
        new.process(input, &mut self.new_l, &mut self.new_r);
        for i in 0..frames {
            let n = self.position + i;
            let theta = if n >= self.length {
                FRAC_PI_2
            } else {
                n as f32 / self.length as f32 * FRAC_PI_2
            };
            let (new_gain, old_gain) = theta.sin_cos();
            out_l[i] = out_l[i] * old_gain + self.new_l[i] * new_gain;
            out_r[i] = out_r[i] * old_gain + self.new_r[i] * new_gain;
        }
        self.position += frames;
        self.active = self.position <= self.length;
        !self.active
    }
}

impl Default for Crossfader {
    fn default() -> Self {
        Crossfader::new()
    }
}
