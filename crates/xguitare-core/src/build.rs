use std::collections::HashMap;
use std::fmt;
use std::hash::BuildHasher;
use std::sync::Arc;

use xguitare_dsp::{Channels, Rack, StereoRack, StereoRackError, StereoSlot};

use crate::arrangement::{Arrangement, TakeNotLoaded};
use crate::engine::{Command, EngineQueues};
use crate::preset::CABINET_TYPE;
use crate::project::Project;
use crate::{BuildError, BuildWarning, Config, ModuleRegistry, Preset, PresetModule};

pub(crate) const ENGINE_BUSY: &str = "engine busy";
const PROBE_SAMPLE_RATE: f32 = 48_000.0;

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

/// Builds the stereo Rack of a Bus or Master from `modules` at `sample_rate` for blocks of up to `max_frames`: a stereo Module runs as it is, a mono one linked when it supports [`xguitare_dsp::Module::process_linked`], else as two instances with the same Parameters.
///
/// # Errors
///
/// [`StereoRackError::CabinetNotAllowed`] for a cabinet.
///
/// # Panics
///
/// When a Module names an unregistered type; a parsed Project never yields one.
pub fn build_stereo_rack(
    sample_rate: f32,
    max_frames: usize,
    modules: &[PresetModule],
) -> Result<StereoRack, StereoRackError> {
    if modules
        .iter()
        .any(|module| module.type_name == CABINET_TYPE)
    {
        return Err(StereoRackError::CabinetNotAllowed);
    }
    let registry = ModuleRegistry::default();
    let construct = |type_name: &str| {
        registry
            .construct(type_name)
            .expect("a parsed Project names only registered module types")
    };
    let slots = modules
        .iter()
        .map(|module| {
            let first = construct(&module.type_name);
            let slot = match first.spec().channels {
                Channels::Stereo => StereoSlot::Stereo(first),
                _ if links(&module.type_name, &registry) => StereoSlot::Linked(first),
                _ => StereoSlot::Doubled(first, construct(&module.type_name)),
            };
            (module.id.clone(), slot)
        })
        .collect();
    let mut rack = StereoRack::new(slots)?;
    for (idx, module) in modules.iter().enumerate() {
        for (param, (_, value)) in module.params.iter().enumerate() {
            rack.set_param(idx, param, *value);
        }
    }
    rack.prepare(sample_rate, max_frames);
    for (idx, module) in modules.iter().enumerate() {
        rack.reset(idx);
        if module.bypass {
            rack.start_bypassed(idx);
        }
    }
    Ok(rack)
}

/// Whether a Module of `type_name` runs linked over both sides: a one-frame probe of a fresh instance.
fn links(type_name: &str, registry: &ModuleRegistry) -> bool {
    registry.construct(type_name).is_some_and(|mut probe| {
        probe.prepare(PROBE_SAMPLE_RATE, 1);
        probe.process_linked(&mut [0.0], &mut [0.0])
    })
}

/// Why [`build_arrangement`] built nothing.
#[derive(Debug)]
pub enum ArrangementError {
    Track {
        name: String,
        error: BuildError,
    },
    Bus {
        name: String,
        error: StereoRackError,
    },
    Master(StereoRackError),
    Take(TakeNotLoaded),
}

/// Builds `project`'s Arrangement at `sample_rate` for blocks of up to `max_frames`: each Track's Modules as a Rack the way [`build_rack`] builds a Preset, its Items from the preloaded `takes`, and each Bus's and Master's Modules as [`build_stereo_rack`] builds them.
///
/// # Errors
///
/// [`ArrangementError::Track`] when a Track's Module order breaks a Rack layout rule; [`ArrangementError::Bus`] or [`ArrangementError::Master`] for a cabinet there; [`ArrangementError::Take`] when an Item's active Take is not in `takes`.
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
    let bus_racks = project
        .buses
        .iter()
        .map(|bus| {
            build_stereo_rack(sample_rate, max_frames, &bus.modules).map_err(|error| {
                ArrangementError::Bus {
                    name: bus.name.clone(),
                    error,
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let master_rack = build_stereo_rack(sample_rate, max_frames, &project.master.modules)
        .map_err(ArrangementError::Master)?;
    let arrangement = Arrangement::new(
        project,
        racks,
        bus_racks,
        master_rack,
        takes,
        sample_rate,
        max_frames,
    )
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
            ArrangementError::Bus { name, error } => write!(f, "bus '{name}': {error}"),
            ArrangementError::Master(error) => write!(f, "master: {error}"),
            ArrangementError::Take(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ArrangementError {}
