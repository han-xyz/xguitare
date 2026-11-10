use xguitare_dsp::Rack;

use crate::engine::{Command, EngineQueues};
use crate::{BuildError, BuildWarning, Config, Preset};

pub(crate) const ENGINE_BUSY: &str = "engine busy";

/// Builds `preset`'s Rack at `sample_rate` for blocks of up to `max_frames`, resolving IR names under the config's `ir_dir` then its Bank dir.
///
/// # Errors
///
/// [`BuildError::Layout`] when the Module order breaks a Rack layout rule.
pub fn build_rack(
    config: &Config,
    sample_rate: f32,
    max_frames: usize,
    preset: &Preset,
) -> Result<(Rack, Vec<BuildWarning>), BuildError> {
    preset.build(
        sample_rate,
        max_frames,
        &config.ir_dir,
        &config.bank_dir,
        config.ir_max_ms,
    )
}

/// Drains the Racks the audio thread retired, then pushes `rack` as a `SwapRack`; fails with `engine busy` when the command queue is full.
///
/// # Errors
///
/// `engine busy` when the command queue has no free slot for the `SwapRack`; `rack` is then dropped here.
pub fn swap_rack(queues: &mut EngineQueues, rack: Rack) -> Result<(), String> {
    queues.drain_from_audio();
    queues
        .commands
        .push(Command::SwapRack(Box::new(rack)))
        .map_err(|_| ENGINE_BUSY.to_string())
}
