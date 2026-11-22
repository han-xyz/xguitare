use std::collections::HashMap;
use std::sync::Arc;

use crate::arrangement::ArrangementCommand;
use crate::build::{ENGINE_BUSY, build_arrangement, swap_arrangement};
use crate::control::EngineState;
use crate::engine::Command;
use crate::metronome_control::click_volume_db;
use crate::project::{MAX_BUSES, MAX_TRACKS, Project};
use crate::project_state::{NO_PROJECT, ProjectEvent, TrackReaders, open_project_mut};
use crate::take_capture::TrackReader;
use crate::{BuildWarning, Config};

/// `moved_from` when no Track or Bus moved.
pub(crate) const NOTHING_MOVED: (usize, usize) = (MAX_TRACKS, MAX_BUSES);

/// Builds `project`'s Arrangement, swaps it in and makes `project` the open Project's; on an error nothing changes.
pub(crate) fn swap_project(
    state: &mut EngineState,
    project: Project,
    warnings: &mut Vec<BuildWarning>,
) -> Result<(), String> {
    swap_project_moving(state, project, NOTHING_MOVED, warnings)
}

/// [`swap_project`] after a removal: the clip flags from Track `moved_from.0` and Bus `moved_from.1` on are cleared once the new Arrangement runs alone.
pub(crate) fn swap_project_moving(
    state: &mut EngineState,
    project: Project,
    moved_from: (usize, usize),
    warnings: &mut Vec<BuildWarning>,
) -> Result<(), String> {
    let track_ids = state
        .projects
        .open
        .as_ref()
        .ok_or(NO_PROJECT)?
        .track_ids
        .clone();
    swap_project_with_ids(state, project, track_ids, moved_from, warnings)
}

/// [`swap_project_moving`] for a `project` whose Tracks have the ids `track_ids`.
pub(crate) fn swap_project_with_ids(
    state: &mut EngineState,
    project: Project,
    track_ids: Vec<u64>,
    moved_from: (usize, usize),
    warnings: &mut Vec<BuildWarning>,
) -> Result<(), String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    if rate_mismatch(open.project.sample_rate, state.sample_rate).is_none() {
        let (readers, build_warnings) = start_arrangement(
            &state.config,
            &mut state.queues,
            state.sample_rate,
            state.max_frames,
            &Layout {
                project: &project,
                takes: &open.takes,
                track_ids: &track_ids,
                moved_from,
            },
        )?;
        warnings.extend(build_warnings);
        state.projects.events.push(ProjectEvent::Readers(readers));
    }
    open_project_mut(&mut state.projects).track_ids = track_ids;
    commit(state, project);
    Ok(())
}

/// The commands [`rebuild_arrangement`] may push.
pub(crate) fn rebuild_arrangement_slots(state: &EngineState) -> usize {
    if state.projects.open.is_some() { 4 } else { 0 }
}

/// After a quantum or sample-rate change, rebuilds the open Project's Arrangement at the Engine's rate and `max_frames` and swaps it in; while the Project's rate differs from the Engine's, closes the Arrangement instead, so nothing plays or records at the wrong rate.
pub(crate) fn rebuild_arrangement(state: &mut EngineState) -> Result<Vec<BuildWarning>, String> {
    let Some(open) = state.projects.open.as_ref() else {
        return Ok(Vec::new());
    };
    if rate_mismatch(open.project.sample_rate, state.sample_rate).is_some() {
        state.queues.drain_from_audio();
        state
            .queues
            .commands
            .push(Command::CloseArrangement)
            .map_err(|_| ENGINE_BUSY.to_string())?;
        open_project_mut(&mut state.projects).recording = false;
        return Ok(Vec::new());
    }
    let (readers, warnings) = start_arrangement(
        &state.config,
        &mut state.queues,
        state.sample_rate,
        state.max_frames,
        &Layout {
            project: &open.project,
            takes: &open.takes,
            track_ids: &open.track_ids,
            moved_from: NOTHING_MOVED,
        },
    )?;
    let clicks = click_commands(state, &open.project);
    state.projects.events.push(ProjectEvent::Readers(readers));
    send(state, &clicks)?;
    Ok(warnings)
}

/// Why the open Project does not play: its sample rate differs from the Engine's.
pub fn project_rate_warning(state: &EngineState) -> Option<String> {
    let open = state.projects.open.as_ref()?;
    rate_mismatch(open.project.sample_rate, state.sample_rate)
}

pub(crate) fn rate_mismatch(project_rate: u32, engine_rate: f32) -> Option<String> {
    let engine_rate = engine_rate as u32;
    (project_rate != engine_rate).then(|| {
        format!("project rate {project_rate} Hz differs from the engine rate {engine_rate} Hz")
    })
}

/// How many commands `click` sends.
pub(crate) const CLICK_COMMANDS: usize = 3;

pub(crate) fn click_commands(
    state: &EngineState,
    project: &Project,
) -> [ArrangementCommand; CLICK_COMMANDS] {
    [
        ArrangementCommand::Click(project.click),
        ArrangementCommand::CountIn(project.count_in),
        ArrangementCommand::ClickVolume(click_volume_db(state)),
    ]
}

pub(crate) fn commit(state: &mut EngineState, project: Project) {
    let open = open_project_mut(&mut state.projects);
    open.project = project;
    open.changed = true;
    state.revision += 1;
}

/// What [`start_arrangement`] builds an Arrangement from.
pub(crate) struct Layout<'a> {
    pub(crate) project: &'a Project,
    pub(crate) takes: &'a HashMap<String, Arc<[f32]>>,
    pub(crate) track_ids: &'a [u64],
    /// The first Track and Bus index that moved or went since the running Arrangement, as [`Arrangement::clear_clips_from`] takes them.
    pub(crate) moved_from: (usize, usize),
}

pub(crate) fn start_arrangement(
    config: &Config,
    queues: &mut crate::engine::EngineQueues,
    sample_rate: f32,
    max_frames: usize,
    layout: &Layout,
) -> Result<(TrackReaders, Vec<BuildWarning>), String> {
    let Layout {
        project,
        takes,
        track_ids,
        moved_from,
    } = *layout;
    let (mut arrangement, warnings) =
        build_arrangement(config, project, takes, sample_rate, max_frames)
            .map_err(|err| err.to_string())?;
    arrangement.clear_clips_from(moved_from.0, moved_from.1);
    let readers = project
        .tracks
        .iter()
        .zip(track_ids)
        .zip(arrangement.take_readers())
        .map(|((track, &track_id), reader)| TrackReader {
            track_id,
            name: track.name.clone(),
            reader,
        })
        .collect();
    swap_arrangement(queues, arrangement)?;
    Ok((readers, warnings))
}

pub(crate) fn send(state: &mut EngineState, commands: &[ArrangementCommand]) -> Result<(), String> {
    if state.queues.commands.slots() < commands.len() {
        return Err(ENGINE_BUSY.to_string());
    }
    for command in commands {
        state
            .queues
            .commands
            .push(Command::Arrangement(command.clone()))
            .map_err(|_| ENGINE_BUSY.to_string())?;
    }
    Ok(())
}
