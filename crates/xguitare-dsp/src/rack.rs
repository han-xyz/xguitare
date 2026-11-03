use std::fmt;

use crate::{AudioBuffers, Channels, Module};

pub struct Rack {
    modules: Vec<(String, Box<dyn Module>)>,
    has_cabinet: bool,
    mono: Vec<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
    max_frames: usize,
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
            modules,
            has_cabinet,
            mono: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
            max_frames: 0,
        })
    }

    pub fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.max_frames = max_frames;
        self.mono = vec![0.0; max_frames];
        self.left = vec![0.0; max_frames];
        self.right = vec![0.0; max_frames];
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
        for (_, module) in &mut self.modules {
            module.process(&mut io);
        }
        let (left, right) = if self.has_cabinet {
            (&self.left[..frames], &self.right[..frames])
        } else {
            (&self.mono[..frames], &self.mono[..frames])
        };
        out_l[..frames].copy_from_slice(left);
        out_r[..frames].copy_from_slice(right);
    }

    pub fn modules(&self) -> &[(String, Box<dyn Module>)] {
        &self.modules
    }

    pub fn module_mut(&mut self, idx: usize) -> &mut dyn Module {
        self.modules[idx].1.as_mut()
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
