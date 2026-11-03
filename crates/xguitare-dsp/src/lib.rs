pub mod modules;
mod rack;

pub use modules::gain::Gain;
pub use rack::{Rack, RackError};

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
}
