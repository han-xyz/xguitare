use std::collections::HashMap;
use std::fmt;
use std::hash::BuildHasher;
use std::sync::Arc;

use xguitare_dsp::Rack;

use crate::arrangement::{Arrangement, TakeNotLoaded};
use crate::engine::{Command, EngineQueues};
use crate::project::Project;
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

/// Why [`build_arrangement`] built nothing.
#[derive(Debug)]
pub enum ArrangementError {
    Track { name: String, error: BuildError },
    Take(TakeNotLoaded),
}

/// Builds `project`'s Arrangement at `sample_rate` for blocks of up to `max_frames`: each Track's Modules as a Rack the way [`build_rack`] builds a Preset, and its Items from the preloaded `takes`.
///
/// # Errors
///
/// [`ArrangementError::Track`] when a Track's Module order breaks a Rack layout rule; [`ArrangementError::Take`] when an Item's active Take is not in `takes`.
pub fn build_arrangement<S: BuildHasher>(
    config: &Config,
    project: &Project,
    takes: &HashMap<String, Arc<[f32]>, S>,
    sample_rate: f32,
    max_frames: usize,
) -> Result<(Arrangement, Vec<BuildWarning>), ArrangementError> {
    let mut racks = Vec::with_capacity(project.tracks.len());
    let mut warnings = Vec::new();
    for track in &project.tracks {
        let preset = Preset {
            name: track.name.clone(),
            modules: track.modules.clone(),
        };
        let (rack, track_warnings) =
            build_rack(config, sample_rate, max_frames, &preset).map_err(|error| {
                ArrangementError::Track {
                    name: track.name.clone(),
                    error,
                }
            })?;
        racks.push(rack);
        warnings.extend(track_warnings);
    }
    let arrangement = Arrangement::new(project, racks, takes, sample_rate, max_frames)
        .map_err(ArrangementError::Take)?;
    Ok((arrangement, warnings))
}

/// Drains what the audio thread retired, then pushes `arrangement` as a `SwapArrangement`.
///
/// # Errors
///
/// `engine busy` when the command queue has no free slot; `arrangement` is then dropped here.
pub fn swap_arrangement(queues: &mut EngineQueues, arrangement: Arrangement) -> Result<(), String> {
    queues.drain_from_audio();
    queues
        .commands
        .push(Command::SwapArrangement(Box::new(arrangement)))
        .map_err(|_| ENGINE_BUSY.to_string())
}

impl fmt::Display for ArrangementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArrangementError::Track { name, error } => write!(f, "track '{name}': {error}"),
            ArrangementError::Take(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ArrangementError {}
