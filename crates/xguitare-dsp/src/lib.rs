pub mod crossfade;
pub mod midi;
pub(crate) mod modulated_delay;
pub mod modules;
pub mod oversample;
mod rack;
pub mod resample;
pub mod smooth;
mod stereo_rack;
pub mod svf;
pub mod synth_ir;
pub mod tuner;

pub use modules::autowah::Autowah;
pub use modules::blend::Blend;
pub use modules::chorus::Chorus;
pub use modules::compressor::Compressor;
pub use modules::delay::Delay;
pub use modules::drive::Drive;
pub use modules::eq::{Eq, eq_response_db};
pub use modules::flanger::Flanger;
pub use modules::fuzz::Fuzz;
pub use modules::gain::Gain;
pub use modules::gate::Gate;
pub use modules::limiter::Limiter;
pub use modules::preamp::Preamp;
pub use modules::reverb::Reverb;
pub use modules::shaper::Shaper;
pub use modules::tonestack::Tonestack;
pub use rack::{Rack, RackError};
pub use stereo_rack::{StereoRack, StereoRackError, StereoSlot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channels {
    Mono,
    Stereo,
    MonoToStereo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Linear,
    Db,
    Ms,
    Hz,
    Percent,
}

#[derive(Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: Unit,
    pub integer: bool,
    /// Names for the values of a choice Parameter: value `n` means `labels[n]`; empty for a plain number.
    pub labels: &'static [&'static str],
}

#[derive(Debug)]
pub struct ModuleSpec {
    pub type_name: &'static str,
    pub channels: Channels,
    pub params: &'static [ParamSpec],
}

pub struct AudioBuffers<'a> {
    pub frames: usize,
    pub mono: &'a mut [f32],
    pub left: &'a mut [f32],
    pub right: &'a mut [f32],
}

/// One Rack stage: a `Mono` Module reads and writes `mono`, a `MonoToStereo` Module reads `mono` and writes `left`/`right`, a `Stereo` Module reads and writes `left`/`right`; only constructors and `prepare` may allocate.
pub trait Module: Send {
    fn spec(&self) -> &'static ModuleSpec;
    fn prepare(&mut self, sample_rate: f32, max_frames: usize);
    fn reset(&mut self);
    fn set_param(&mut self, idx: usize, value: f32);
    fn param(&self, idx: usize) -> f32;
    fn process(&mut self, io: &mut AudioBuffers);
    /// Processes `left` and `right` as one linked pair (one gain for both sides); returns false when the Module does not support it and has left both untouched.
    fn process_linked(&mut self, _left: &mut [f32], _right: &mut [f32]) -> bool {
        false
    }
}
