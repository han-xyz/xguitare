use crate::Rack;

/// Linear (equal-gain) crossfade from one Rack to another: the incoming Rack's gain `t` rises from 0 to 1 over the fade and the outgoing one's is `1 - t`. Both Racks process the same guitar input, so their outputs are correlated and equal gain keeps the level flat where equal power would swell by 3 dB.
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
            let new_gain = if n >= self.length {
                1.0
            } else {
                n as f32 / self.length as f32
            };
            let old_gain = 1.0 - new_gain;
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
