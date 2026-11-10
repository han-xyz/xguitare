use std::fmt;

use crate::{AudioBuffers, Channels, Module};

const BYPASS_FADE_MS: f32 = 10.0;

pub struct Rack {
    modules: Vec<(String, Box<dyn Module>)>,
    bypass: Vec<Bypass>,
    has_cabinet: bool,
    mono: Vec<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
    dry_left: Vec<f32>,
    dry_right: Vec<f32>,
    gains: Vec<f32>,
    bypass_step: f32,
    max_frames: usize,
}

/// One slot's Bypass: `mix` is the share of the Module's output, ramping towards 0 while bypassed and towards 1 while not.
#[derive(Clone, Copy)]
struct Bypass {
    bypassed: bool,
    mix: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RackError {
    StereoBeforeCabinet(String),
    MonoAfterCabinet(String),
    TwoCabinets,
}

impl Rack {
    pub fn new(modules: Vec<(String, Box<dyn Module>)>) -> Result<Rack, RackError> {
        let mut has_cabinet = false;
        for (id, module) in &modules {
            match (module.spec().channels, has_cabinet) {
                (Channels::MonoToStereo, true) => return Err(RackError::TwoCabinets),
                (Channels::MonoToStereo, false) => has_cabinet = true,
                (Channels::Stereo, false) => {
                    return Err(RackError::StereoBeforeCabinet(id.clone()));
                }
                (Channels::Mono, true) => return Err(RackError::MonoAfterCabinet(id.clone())),
                _ => {}
            }
        }
        Ok(Rack {
            bypass: vec![
                Bypass {
                    bypassed: false,
                    mix: 1.0,
                };
                modules.len()
            ],
            modules,
            has_cabinet,
            mono: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
            dry_left: Vec::new(),
            dry_right: Vec::new(),
            gains: Vec::new(),
            bypass_step: 1.0,
            max_frames: 0,
        })
    }

    pub fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.max_frames = max_frames;
        self.mono = vec![0.0; max_frames];
        self.left = vec![0.0; max_frames];
        self.right = vec![0.0; max_frames];
        self.dry_left = vec![0.0; max_frames];
        self.dry_right = vec![0.0; max_frames];
        self.gains = vec![0.0; max_frames];
        self.bypass_step = 1.0 / (BYPASS_FADE_MS * sample_rate / 1000.0).round().max(1.0);
        for (_, module) in &mut self.modules {
            module.prepare(sample_rate, max_frames);
        }
    }

    /// Runs one block of `input.len()` frames, which must not exceed the `max_frames` given to `prepare`, into `out_l`/`out_r`.
    pub fn process(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = input.len();
        debug_assert!(
            frames <= self.max_frames,
            "{frames} frames exceed max_frames {}",
            self.max_frames
        );
        self.mono[..frames].copy_from_slice(input);
        let mut io = AudioBuffers {
            frames,
            mono: &mut self.mono,
            left: &mut self.left,
            right: &mut self.right,
        };
        for ((_, module), bypass) in self.modules.iter_mut().zip(&mut self.bypass) {
            let channels = module.spec().channels;
            match (bypass.bypassed, bypass.mix) {
                (true, 0.0) => pass_input(channels, &mut io),
                (false, 1.0) => module.process(&mut io),
                _ => {
                    keep_input(channels, &io, &mut self.dry_left, &mut self.dry_right);
                    module.process(&mut io);
                    bypass.ramp(self.bypass_step, &mut self.gains[..frames]);
                    mix_input(
                        channels,
                        &mut io,
                        &self.dry_left,
                        &self.dry_right,
                        &self.gains,
                    );
                }
            }
        }
        let (left, right) = if self.has_cabinet {
            (&self.left[..frames], &self.right[..frames])
        } else {
            (&self.mono[..frames], &self.mono[..frames])
        };
        out_l[..frames].copy_from_slice(left);
        out_r[..frames].copy_from_slice(right);
    }

    /// Fades the Module at `idx` over 10 ms to its input (`on`) or back to its output; once bypassed it is not processed, and it is reset before it comes back.
    pub fn set_bypass(&mut self, idx: usize, on: bool) {
        let bypass = &mut self.bypass[idx];
        if !on && bypass.mix == 0.0 {
            self.modules[idx].1.reset();
        }
        bypass.bypassed = on;
    }

    /// Bypasses the Module at `idx` with no fade, for a Rack that has not run yet.
    pub fn start_bypassed(&mut self, idx: usize) {
        self.bypass[idx] = Bypass {
            bypassed: true,
            mix: 0.0,
        };
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    pub fn modules(&self) -> &[(String, Box<dyn Module>)] {
        &self.modules
    }

    pub fn module_mut(&mut self, idx: usize) -> &mut dyn Module {
        self.modules[idx].1.as_mut()
    }
}

impl Bypass {
    /// Steps `mix` towards its target once per frame and writes each frame's value to `gains`.
    fn ramp(&mut self, step: f32, gains: &mut [f32]) {
        for gain in gains {
            self.mix = if self.bypassed {
                (self.mix - step).max(0.0)
            } else {
                (self.mix + step).min(1.0)
            };
            *gain = self.mix;
        }
    }
}

/// What a bypassed Module leaves in the buffers: its input, which for a `MonoToStereo` Module means `mono` on both sides.
fn pass_input(channels: Channels, io: &mut AudioBuffers) {
    if channels == Channels::MonoToStereo {
        let frames = io.frames;
        io.left[..frames].copy_from_slice(&io.mono[..frames]);
        io.right[..frames].copy_from_slice(&io.mono[..frames]);
    }
}

fn keep_input(channels: Channels, io: &AudioBuffers, dry_left: &mut [f32], dry_right: &mut [f32]) {
    let frames = io.frames;
    match channels {
        Channels::Mono | Channels::MonoToStereo => {
            dry_left[..frames].copy_from_slice(&io.mono[..frames]);
        }
        Channels::Stereo => {
            dry_left[..frames].copy_from_slice(&io.left[..frames]);
            dry_right[..frames].copy_from_slice(&io.right[..frames]);
        }
    }
}

fn mix_input(
    channels: Channels,
    io: &mut AudioBuffers,
    dry_left: &[f32],
    dry_right: &[f32],
    gains: &[f32],
) {
    let frames = io.frames;
    match channels {
        Channels::Mono => mix(&mut io.mono[..frames], dry_left, gains),
        Channels::MonoToStereo => {
            mix(&mut io.left[..frames], dry_left, gains);
            mix(&mut io.right[..frames], dry_left, gains);
        }
        Channels::Stereo => {
            mix(&mut io.left[..frames], dry_left, gains);
            mix(&mut io.right[..frames], dry_right, gains);
        }
    }
}

fn mix(wet: &mut [f32], dry: &[f32], gains: &[f32]) {
    for ((wet, dry), gain) in wet.iter_mut().zip(dry).zip(gains) {
        *wet = dry + gain * (*wet - dry);
    }
}

impl fmt::Display for RackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RackError::StereoBeforeCabinet(id) => {
                write!(f, "stereo module '{id}' before the cabinet")
            }
            RackError::MonoAfterCabinet(id) => write!(f, "mono module '{id}' after the cabinet"),
            RackError::TwoCabinets => f.write_str("more than one cabinet"),
        }
    }
}

impl std::error::Error for RackError {}
