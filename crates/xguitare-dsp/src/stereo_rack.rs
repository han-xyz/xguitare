use std::fmt;

use crate::rack::{BYPASS_FADE_MS, Bypass, mix};
use crate::{AudioBuffers, Channels, Module};

/// A Rack with stereo in and stereo out, for a Bus or the Master; it holds no cabinet.
pub struct StereoRack {
    slots: Vec<(String, StereoSlot)>,
    bypass: Vec<Bypass>,
    left: Vec<f32>,
    right: Vec<f32>,
    spare_a: Vec<f32>,
    spare_b: Vec<f32>,
    dry_left: Vec<f32>,
    dry_right: Vec<f32>,
    gains: Vec<f32>,
    bypass_step: f32,
    max_frames: usize,
}

/// How one Module runs in a [`StereoRack`].
pub enum StereoSlot {
    /// A `Stereo` Module, run as it is.
    Stereo(Box<dyn Module>),
    /// A mono Module run once over both sides through [`Module::process_linked`]; one that returns false leaves the input as it is.
    Linked(Box<dyn Module>),
    /// Two instances of a mono Module, the first on the left side and the second on the right.
    Doubled(Box<dyn Module>, Box<dyn Module>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StereoRackError {
    CabinetNotAllowed,
    MonoInStereoSlot(String),
    StereoInMonoSlot(String),
}

impl StereoRack {
    /// # Errors
    ///
    /// A [`StereoRackError`] for a cabinet in any slot, a mono Module in a `Stereo` slot, or a stereo Module in a `Linked` or `Doubled` slot.
    pub fn new(slots: Vec<(String, StereoSlot)>) -> Result<StereoRack, StereoRackError> {
        for (id, slot) in &slots {
            let stereo_slot = matches!(slot, StereoSlot::Stereo(_));
            for module in slot.instances() {
                match (module.spec().channels, stereo_slot) {
                    (Channels::MonoToStereo, _) => return Err(StereoRackError::CabinetNotAllowed),
                    (Channels::Mono, true) => {
                        return Err(StereoRackError::MonoInStereoSlot(id.clone()));
                    }
                    (Channels::Stereo, false) => {
                        return Err(StereoRackError::StereoInMonoSlot(id.clone()));
                    }
                    _ => {}
                }
            }
        }
        Ok(StereoRack {
            bypass: vec![
                Bypass {
                    bypassed: false,
                    mix: 1.0,
                };
                slots.len()
            ],
            slots,
            left: Vec::new(),
            right: Vec::new(),
            spare_a: Vec::new(),
            spare_b: Vec::new(),
            dry_left: Vec::new(),
            dry_right: Vec::new(),
            gains: Vec::new(),
            bypass_step: 1.0,
            max_frames: 0,
        })
    }

    pub fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.max_frames = max_frames;
        for buffer in [
            &mut self.left,
            &mut self.right,
            &mut self.spare_a,
            &mut self.spare_b,
            &mut self.dry_left,
            &mut self.dry_right,
            &mut self.gains,
        ] {
            *buffer = vec![0.0; max_frames];
        }
        self.bypass_step = 1.0 / (BYPASS_FADE_MS * sample_rate / 1000.0).round().max(1.0);
        for (_, slot) in &mut self.slots {
            for module in slot.instances_mut() {
                module.prepare(sample_rate, max_frames);
            }
        }
    }

    /// Runs one block of `in_l.len()` frames, which must not exceed the `max_frames` given to `prepare`, into `out_l`/`out_r`.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = in_l.len();
        debug_assert!(
            frames <= self.max_frames,
            "{frames} frames exceed max_frames {}",
            self.max_frames
        );
        let left = &mut self.left[..frames];
        let right = &mut self.right[..frames];
        let spare_a = &mut self.spare_a[..frames];
        let spare_b = &mut self.spare_b[..frames];
        left.copy_from_slice(in_l);
        right.copy_from_slice(&in_r[..frames]);
        for ((_, slot), bypass) in self.slots.iter_mut().zip(&mut self.bypass) {
            match (bypass.bypassed, bypass.mix) {
                (true, 0.0) => {}
                (false, 1.0) => slot.process(left, right, spare_a, spare_b),
                _ => {
                    let dry_left = &mut self.dry_left[..frames];
                    let dry_right = &mut self.dry_right[..frames];
                    let gains = &mut self.gains[..frames];
                    dry_left.copy_from_slice(left);
                    dry_right.copy_from_slice(right);
                    slot.process(left, right, spare_a, spare_b);
                    bypass.ramp(self.bypass_step, gains);
                    mix(left, dry_left, gains);
                    mix(right, dry_right, gains);
                }
            }
        }
        out_l[..frames].copy_from_slice(left);
        out_r[..frames].copy_from_slice(right);
    }

    /// Fades the slot at `idx` over 10 ms to its input (`on`) or back to its output, as [`crate::Rack::set_bypass`] does; an `idx` out of range is ignored.
    pub fn set_bypass(&mut self, idx: usize, on: bool) {
        let (Some(bypass), Some((_, slot))) = (self.bypass.get_mut(idx), self.slots.get_mut(idx))
        else {
            return;
        };
        if !on && bypass.mix == 0.0 {
            for module in slot.instances_mut() {
                module.reset();
            }
        }
        bypass.bypassed = on;
    }

    /// Bypasses the slot at `idx` with no fade, for a Rack that has not run yet; an `idx` out of range is ignored.
    pub fn start_bypassed(&mut self, idx: usize) {
        if let Some(bypass) = self.bypass.get_mut(idx) {
            *bypass = Bypass {
                bypassed: true,
                mix: 0.0,
            };
        }
    }

    /// Sets Parameter `param` on every instance in the slot at `idx`; an `idx` or `param` out of range is ignored.
    pub fn set_param(&mut self, idx: usize, param: usize, value: f32) {
        let Some((_, slot)) = self.slots.get_mut(idx) else {
            return;
        };
        for module in slot.instances_mut() {
            if param < module.spec().params.len() {
                module.set_param(param, value);
            }
        }
    }

    /// The id and first Module instance of each slot, in Rack order.
    pub fn modules(&self) -> impl ExactSizeIterator<Item = (&str, &dyn Module)> {
        self.slots
            .iter()
            .map(|(id, slot)| (id.as_str(), slot.first()))
    }
}

impl StereoSlot {
    fn first(&self) -> &dyn Module {
        match self {
            StereoSlot::Stereo(module)
            | StereoSlot::Linked(module)
            | StereoSlot::Doubled(module, _) => module.as_ref(),
        }
    }

    fn instances(&self) -> impl Iterator<Item = &dyn Module> {
        let second = match self {
            StereoSlot::Doubled(_, second) => Some(second.as_ref()),
            StereoSlot::Stereo(_) | StereoSlot::Linked(_) => None,
        };
        std::iter::once(self.first()).chain(second)
    }

    fn instances_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Module>> {
        let (first, second) = match self {
            StereoSlot::Stereo(module) | StereoSlot::Linked(module) => (module, None),
            StereoSlot::Doubled(first, second) => (first, Some(second)),
        };
        std::iter::once(first).chain(second)
    }

    /// Runs the slot in place on `left`/`right`; `spare_a`/`spare_b` fill the `AudioBuffers` channels the Module does not use.
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        spare_a: &mut [f32],
        spare_b: &mut [f32],
    ) {
        let frames = left.len();
        match self {
            StereoSlot::Stereo(module) => module.process(&mut AudioBuffers {
                frames,
                mono: spare_a,
                left,
                right,
            }),
            StereoSlot::Linked(module) => {
                module.process_linked(left, right);
            }
            StereoSlot::Doubled(first, second) => {
                first.process(&mut AudioBuffers {
                    frames,
                    mono: left,
                    left: &mut *spare_a,
                    right: &mut *spare_b,
                });
                second.process(&mut AudioBuffers {
                    frames,
                    mono: right,
                    left: spare_a,
                    right: spare_b,
                });
            }
        }
    }
}

impl fmt::Display for StereoRackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StereoRackError::CabinetNotAllowed => {
                f.write_str("a cabinet is not allowed on a Bus or Master")
            }
            StereoRackError::MonoInStereoSlot(id) => {
                write!(f, "mono module '{id}' in a stereo slot")
            }
            StereoRackError::StereoInMonoSlot(id) => {
                write!(f, "stereo module '{id}' in a doubled or linked slot")
            }
        }
    }
}

impl std::error::Error for StereoRackError {}
