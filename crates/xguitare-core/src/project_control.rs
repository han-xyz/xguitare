use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use serde_json::{Map, Value, json};

use crate::arrangement::ArrangementCommand;
use crate::arrangement::Transport;
use crate::build::{ENGINE_BUSY, build_arrangement, swap_arrangement};
use crate::control::{
    BusRef, EngineState, MixPeaks, Position, TrackRef, TransportAction, peak_db, slug,
};
use crate::engine::{Command, MeterPoint, transport_from_code};
use crate::mixer_control::{buses_status, master_status, peaks_db, sends_status};
use crate::ports::AudioPorts;
use crate::project::{
    LoopRegion, MAX_BUSES, MAX_TRACKS, PROJECT_FILE, Project, Track, autosave_due, autosave_newer,
    check_project, check_track, checked_name,
};
use crate::take_capture::{FinishedTake, Latency, TrackReader, add_finished_take, load_take};
use crate::{BuildWarning, Config, Preset, PresetModule};

pub(crate) const NO_PROJECT: &str = "no project open";
const PROJECT_IS_OPEN: &str = "a project is open; close it first";
const NO_TRACKS: &str = "the project has no tracks";
const NOTHING_ARMED: &str = "no track is armed";
const NEEDS_POSITION: &str = "locate needs a position";
const BAD_POSITION: &str = "position must be <bar>.<beat> or <seconds>s";
const POSITION_TOO_FAR: &str = "position is more than an hour past the end of the project";
/// The answer to `project_new` and `project_open` while a closed Project still gets its last Takes.
pub const STILL_CLOSING: &str = "a project is still closing; try again";
/// How far past the Project's last Item or loop end `locate` may move the playhead.
const LOCATE_SECONDS_PAST_END: f64 = 3600.0;
const DEFAULT_TRACK_NAME: &str = "Track";
const MASTER_NAME: &str = "Master";
/// `moved_from` when no Track or Bus moved.
const NOTHING_MOVED: (usize, usize) = (MAX_TRACKS, MAX_BUSES);
const ONE_TARGET: &str = "give track, bus or master, not more than one";
const SECONDS_SUFFIX: char = 's';

/// The open Project, if any, and what the Daemon must do outside the Engine because of it.
#[derive(Default)]
pub struct Projects {
    open: Option<OpenProject>,
    events: Vec<ProjectEvent>,
    /// Closed Projects that recorded and whose last Takes the Daemon has not yet added and saved.
    closing: usize,
}

struct OpenProject {
    dir: PathBuf,
    project: Project,
    takes: HashMap<String, Arc<[f32]>>,
    selected: usize,
    /// Whether the last transport request was `record`; `play` and `stop` end it.
    recording: bool,
    /// Whether `record` was requested since it was opened, so Takes may still be on their way when it closes.
    recorded: bool,
    /// One id per Track, in Track order, that stays the Track's while others are added or removed; finished Takes name their Track by it.
    track_ids: Vec<u64>,
    next_track_id: u64,
    /// Whether the Project changed since it was opened, saved or autosaved.
    changed: bool,
    last_autosave: Instant,
    /// Finished Takes not yet in `project` because the Engine was busy; [`add_finished_takes`] retries them.
    unplaced: Vec<FinishedTake>,
}

/// Take readers, one per Track in Track order.
pub type TrackReaders = Vec<TrackReader>;

/// Work for the Daemon, in the order it arose.
#[allow(
    clippy::large_enum_variant,
    reason = "a few events per Control API request; boxing the closed Project saves nothing that matters"
)]
pub enum ProjectEvent {
    /// Start a disk thread writing Takes under `dir` at `sample_rate`.
    Opened { dir: PathBuf, sample_rate: u32 },
    /// Queue these Take readers on the disk thread.
    Readers(TrackReaders),
    /// Connect Track n's input port to `inputs[n]`, disconnecting the rest.
    Inputs(Vec<Option<String>>),
    /// Stop the disk thread and hand the Takes it still finishes to [`finish_closed`].
    Closed(ClosedProject),
}

pub struct ClosedProject {
    pub dir: PathBuf,
    pub project: Project,
    /// Whether it changed since it was last saved or autosaved.
    pub changed: bool,
    track_ids: Vec<u64>,
    /// Whether it counts in [`Projects`]' `closing` until [`finish_closed`] adds its last Takes.
    takes_pending: bool,
    /// Finished Takes the Engine was too busy to take before the close; added with the last Takes.
    unplaced: Vec<FinishedTake>,
}

/// What [`finish_closed`] and [`finish_stopped`] did, for the Daemon to log.
#[derive(Debug, Default)]
pub struct LastTakes {
    /// The Take files added.
    pub added: Vec<String>,
    pub failures: Vec<String>,
    /// The file written, if any.
    pub saved: Option<PathBuf>,
}

/// The events since the last call.
pub fn take_project_events(state: &mut EngineState) -> Vec<ProjectEvent> {
    std::mem::take(&mut state.projects.events)
}

/// Adds the Takes the disk thread finished, and those an earlier call found the Engine too busy for, to the open Project, each on the Track its id names at the latency `latency` gives for that Track's index, and swaps in an Arrangement that plays them; returns one message per Take that could not be added. While the Engine is busy the Takes wait for the next call.
pub fn add_finished_takes(
    state: &mut EngineState,
    finished: &[FinishedTake],
    latency: impl FnMut(usize) -> Latency,
) -> Vec<String> {
    let Some(open) = state.projects.open.as_mut() else {
        return Vec::new();
    };
    let mut takes = std::mem::take(&mut open.unplaced);
    takes.extend_from_slice(finished);
    if takes.is_empty() {
        return Vec::new();
    }
    let mut project = open.project.clone();
    let (added, mut failures) = add_takes(
        &mut project,
        &open.dir,
        &mut open.takes,
        &open.track_ids,
        &takes,
        latency,
    );
    let mut warnings = Vec::new();
    match swap_project(state, project, &mut warnings) {
        Ok(()) => {}
        Err(err) if err == ENGINE_BUSY => {
            takes.retain(|take| added.contains(&take.file));
            open_project_mut(state).unplaced = takes;
        }
        Err(err) => failures.push(format!("cannot play the new takes: {err}")),
    }
    failures.extend(warnings.iter().map(ToString::to_string));
    let open = open_project_mut(state);
    if !open.recording {
        failures.extend(autosave_on_stop(open).map(|err| format!("cannot autosave: {err}")));
    }
    failures
}

/// Writes `project.autosave.toml` when the open Project changed and the autosave interval has passed since it was opened, saved or autosaved; returns the file written, or `None` when none was due.
pub fn autosave_if_due(state: &mut EngineState, now: Instant) -> Option<Result<PathBuf, String>> {
    let open = state.projects.open.as_mut()?;
    if !autosave_due(open.last_autosave, now, open.changed) {
        return None;
    }
    Some(write_autosave(open, now))
}

fn write_autosave(open: &mut OpenProject, now: Instant) -> Result<PathBuf, String> {
    open.last_autosave = now;
    open.changed = false;
    let file = open.dir.join(crate::project::AUTOSAVE_FILE);
    open.project
        .save_autosave(&open.dir)
        .map(|()| file)
        .map_err(|err| err.to_string())
}

fn autosave_on_stop(open: &mut OpenProject) -> Option<String> {
    if !open.changed {
        return None;
    }
    write_autosave(open, Instant::now()).err().inspect(|_| {
        open.changed = true;
    })
}

/// Takes the open Project out as the Daemon stops; the Engine is not told.
pub fn close_for_stop(state: &mut EngineState) -> Option<ClosedProject> {
    state.projects.open.take().map(|open| ClosedProject {
        dir: open.dir,
        project: open.project,
        changed: open.changed,
        track_ids: open.track_ids,
        takes_pending: false,
        unplaced: open.unplaced,
    })
}

/// Adds the last Takes of a Project `project_close` closed and saves its `project.toml` when any was added; from then on Projects can be opened again.
pub fn finish_closed(
    state: &mut EngineState,
    mut closed: ClosedProject,
    finished: &[FinishedTake],
    latency: impl FnMut(usize) -> Latency,
) -> LastTakes {
    if closed.takes_pending {
        state.projects.closing -= 1;
    }
    let mut last = last_takes(&mut closed, finished, latency);
    if !last.added.is_empty() {
        save_closed(
            &closed,
            &mut last,
            Project::save,
            crate::project::PROJECT_FILE,
        );
    }
    last
}

/// Adds the last Takes of the Project open when the Daemon stopped and writes its `project.autosave.toml` when it changed or any Take was added.
pub fn finish_stopped(
    mut closed: ClosedProject,
    finished: &[FinishedTake],
    latency: impl FnMut(usize) -> Latency,
) -> LastTakes {
    let mut last = last_takes(&mut closed, finished, latency);
    if closed.changed || !last.added.is_empty() {
        save_closed(
            &closed,
            &mut last,
            Project::save_autosave,
            crate::project::AUTOSAVE_FILE,
        );
    }
    last
}

fn last_takes(
    closed: &mut ClosedProject,
    finished: &[FinishedTake],
    latency: impl FnMut(usize) -> Latency,
) -> LastTakes {
    let mut takes = std::mem::take(&mut closed.unplaced);
    takes.extend_from_slice(finished);
    let (added, failures) = add_takes(
        &mut closed.project,
        &closed.dir,
        &mut HashMap::new(),
        &closed.track_ids,
        &takes,
        latency,
    );
    LastTakes {
        added,
        failures,
        saved: None,
    }
}

fn save_closed(
    closed: &ClosedProject,
    last: &mut LastTakes,
    save: impl Fn(&Project, &Path) -> Result<(), crate::project::ProjectError>,
    file: &str,
) {
    match save(&closed.project, &closed.dir) {
        Ok(()) => last.saved = Some(closed.dir.join(file)),
        Err(err) => last.failures.push(err.to_string()),
    }
}

/// Adds each finished Take to the Track whose id it names; returns the files added and one message per Take that was not.
fn add_takes(
    project: &mut Project,
    dir: &Path,
    takes: &mut HashMap<String, Arc<[f32]>>,
    track_ids: &[u64],
    finished: &[FinishedTake],
    mut latency: impl FnMut(usize) -> Latency,
) -> (Vec<String>, Vec<String>) {
    let mut added = Vec::new();
    let mut failures = Vec::new();
    for take in finished {
        let Some(track) = track_ids.iter().position(|id| *id == take.track_id) else {
            failures.push(format!("take {} has no track; its file is kept", take.file));
            continue;
        };
        match add_finished_take(project, dir, takes, track, take, latency(track)) {
            Ok(_) => added.push(take.file.clone()),
            Err(err) => failures.push(format!("cannot load take {}: {err}", take.file)),
        }
    }
    (added, failures)
}

/// The Rack a Rack request edits while a Project is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectRack {
    Track(usize),
    Bus(usize),
    Master,
}

impl ProjectRack {
    /// Whether it is a stereo Rack, which takes no cabinet and has no layout rule.
    pub(crate) fn is_stereo(self) -> bool {
        !matches!(self, ProjectRack::Track(_))
    }
}

/// Refuses more than one of `track`, `bus` and `master`.
pub(crate) fn check_one_target(track: bool, bus: bool, master: bool) -> Result<(), String> {
    if [track, bus, master]
        .into_iter()
        .filter(|given| *given)
        .count()
        > 1
    {
        return Err(ONE_TARGET.to_string());
    }
    Ok(())
}

/// The Rack a Rack request edits: `None` (the live Rack) while no Project is open and none is named, else the named Track, Bus or Master, or the selected Track.
pub(crate) fn target_rack(
    state: &EngineState,
    track: Option<&TrackRef>,
    bus: Option<&BusRef>,
    master: bool,
) -> Result<Option<ProjectRack>, String> {
    check_one_target(track.is_some(), bus.is_some(), master)?;
    if let Some(bus) = bus {
        let project = current_project(state)?;
        return resolve_bus(project, bus).map(|bus| Some(ProjectRack::Bus(bus)));
    }
    if master {
        current_project(state)?;
        return Ok(Some(ProjectRack::Master));
    }
    Ok(target_track(state, track)?.map(ProjectRack::Track))
}

fn target_track(state: &EngineState, track: Option<&TrackRef>) -> Result<Option<usize>, String> {
    let Some(open) = state.projects.open.as_ref() else {
        return match track {
            Some(_) => Err(NO_PROJECT.to_string()),
            None => Ok(None),
        };
    };
    match track {
        Some(track) => resolve_track(&open.project, track).map(Some),
        None if open.project.tracks.is_empty() => Err(NO_TRACKS.to_string()),
        None => Ok(Some(open.selected)),
    }
}

/// The open Project, or `no project open`.
pub(crate) fn current_project(state: &EngineState) -> Result<&Project, String> {
    state
        .projects
        .open
        .as_ref()
        .map(|open| &open.project)
        .ok_or_else(|| NO_PROJECT.to_string())
}

/// The Rack's Modules as a Preset named after its Track or Bus (or `Master`), for the Rack requests to read and edit.
pub(crate) fn rack_preset(state: &EngineState, rack: ProjectRack) -> Preset {
    let project = &open_project(state).project;
    let (name, modules) = match rack {
        ProjectRack::Track(track) => (
            project.tracks[track].name.as_str(),
            &project.tracks[track].modules,
        ),
        ProjectRack::Bus(bus) => (
            project.buses[bus].name.as_str(),
            &project.buses[bus].modules,
        ),
        ProjectRack::Master => (MASTER_NAME, &project.master.modules),
    };
    Preset {
        name: name.to_string(),
        modules: modules.clone(),
    }
}

fn rack_modules_mut(project: &mut Project, rack: ProjectRack) -> &mut Vec<PresetModule> {
    match rack {
        ProjectRack::Track(track) => &mut project.tracks[track].modules,
        ProjectRack::Bus(bus) => &mut project.buses[bus].modules,
        ProjectRack::Master => &mut project.master.modules,
    }
}

/// Replaces the Rack's Modules and swaps in the rebuilt Arrangement; on an error nothing changes.
pub(crate) fn set_rack_modules(
    state: &mut EngineState,
    rack: ProjectRack,
    modules: Vec<PresetModule>,
    warnings: &mut Vec<BuildWarning>,
) -> Result<(), String> {
    let mut project = open_project(state).project.clone();
    *rack_modules_mut(&mut project, rack) = modules;
    swap_project(state, project, warnings)
}

/// Sends `command` to the Arrangement, then applies `change` to the Rack's Modules.
pub(crate) fn change_rack_module(
    state: &mut EngineState,
    rack: ProjectRack,
    command: ArrangementCommand,
    change: impl FnOnce(&mut Vec<PresetModule>),
) -> Result<(), String> {
    send(state, &[command])?;
    let open = open_project_mut(state);
    change(rack_modules_mut(&mut open.project, rack));
    open.changed = true;
    state.revision += 1;
    Ok(())
}

pub(crate) fn param_command(
    rack: ProjectRack,
    module: usize,
    param: usize,
    value: f32,
) -> ArrangementCommand {
    match rack {
        ProjectRack::Track(track) => ArrangementCommand::Param {
            track,
            module,
            param,
            value,
        },
        ProjectRack::Bus(bus) => ArrangementCommand::BusParam {
            bus,
            module,
            param,
            value,
        },
        ProjectRack::Master => ArrangementCommand::MasterParam {
            module,
            param,
            value,
        },
    }
}

pub(crate) fn bypass_command(rack: ProjectRack, module: usize, on: bool) -> ArrangementCommand {
    match rack {
        ProjectRack::Track(track) => ArrangementCommand::Bypass { track, module, on },
        ProjectRack::Bus(bus) => ArrangementCommand::BusBypass { bus, module, on },
        ProjectRack::Master => ArrangementCommand::MasterBypass { module, on },
    }
}

pub(crate) fn project_new(state: &mut EngineState, name: &str) -> Result<Value, String> {
    check_none_open(state)?;
    let (dir, mut project) =
        Project::new(&state.config.projects_dir, name).map_err(|err| err.to_string())?;
    project.sample_rate = state.sample_rate as u32;
    project.save(&dir).map_err(|err| err.to_string())?;
    let payload = json!({"name": project.name, "path": dir.display().to_string()});
    open(state, dir, project, HashMap::new())
        .map_err(|err| format!("{err}; the project was created, open it with project_open"))?;
    Ok(payload)
}

fn check_none_open(state: &EngineState) -> Result<(), String> {
    if state.projects.open.is_some() {
        return Err(PROJECT_IS_OPEN.to_string());
    }
    if state.projects.closing > 0 {
        return Err(STILL_CLOSING.to_string());
    }
    Ok(())
}

pub(crate) fn project_open(
    state: &mut EngineState,
    name: &str,
    restore: bool,
) -> Result<Value, String> {
    check_none_open(state)?;
    let folder = slug(name);
    let dir = state.config.projects_dir.join(&folder);
    if folder.is_empty() || !dir.join(PROJECT_FILE).is_file() {
        return Err(format!("no project '{name}'"));
    }
    let newer = autosave_newer(&dir);
    let loaded = if restore {
        Project::load_autosave(&dir)
    } else {
        Project::load(&dir)
    };
    let project = loaded.map_err(|err| err.to_string())?;
    if let Some(mismatch) = rate_mismatch(project.sample_rate, state.sample_rate) {
        return Err(mismatch);
    }
    let takes = load_takes(&dir, &project)?;
    let payload = json!({
        "name": project.name,
        "path": dir.display().to_string(),
        "autosave_newer": newer,
        "restored": restore,
    });
    open(state, dir, project, takes)?;
    if restore {
        open_project_mut(state).changed = true;
    }
    Ok(payload)
}

pub(crate) fn project_save(state: &mut EngineState) -> Result<Value, String> {
    let open = state.projects.open.as_mut().ok_or(NO_PROJECT)?;
    open.project
        .save(&open.dir)
        .map_err(|err| err.to_string())?;
    open.changed = false;
    open.last_autosave = Instant::now();
    Ok(json!({"path": open.dir.join(PROJECT_FILE).display().to_string()}))
}

/// Saves the Project, stops the Arrangement and hands the Project to the Daemon, which adds the Takes still being written.
pub(crate) fn project_close(state: &mut EngineState) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    open.project
        .save(&open.dir)
        .map_err(|err| err.to_string())?;
    state.queues.drain_from_audio();
    state
        .queues
        .commands
        .push(Command::CloseArrangement)
        .map_err(|_| ENGINE_BUSY.to_string())?;
    let Some(open) = state.projects.open.take() else {
        return Err(NO_PROJECT.to_string());
    };
    let payload = json!({"name": open.project.name, "path": open.dir.display().to_string()});
    state
        .projects
        .events
        .push(ProjectEvent::Closed(ClosedProject {
            dir: open.dir,
            project: open.project,
            changed: false,
            track_ids: open.track_ids,
            takes_pending: open.recorded,
            unplaced: open.unplaced,
        }));
    if open.recorded {
        state.projects.closing += 1;
    }
    state.revision += 1;
    Ok(payload)
}

pub(crate) fn project_list(state: &EngineState) -> Result<Value, String> {
    let dirs = Project::list(&state.config.projects_dir).map_err(|err| err.to_string())?;
    let projects: Vec<Value> = dirs
        .iter()
        .map(|dir| {
            let folder = dir
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            match Project::load(dir) {
                Ok(project) => json!({"name": project.name, "folder": folder, "valid": true}),
                Err(err) => json!({"name": folder, "folder": folder, "valid": false, "error": err.to_string()}),
            }
        })
        .collect();
    Ok(json!({ "projects": projects }))
}

pub(crate) fn track_add(state: &mut EngineState, name: Option<&str>) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let name = match name {
        Some(name) => checked_name(&open.project.tracks, name, None)?,
        None => free_track_name(&open.project),
    };
    let modules = state
        .active
        .as_ref()
        .map_or_else(Vec::new, |(_, preset)| preset.modules.clone());
    let preset = Preset {
        name: String::new(),
        modules,
    };
    let mut project = open.project.clone();
    project
        .add_track(Track::from_preset(&name, &preset))
        .map_err(|err| err.to_string())?;
    let index = project.tracks.len() - 1;
    let mut track_ids = open.track_ids.clone();
    track_ids.push(open.next_track_id);
    swap_project_with_ids(state, project, track_ids, NOTHING_MOVED, &mut Vec::new())?;
    let open = open_project_mut(state);
    open.next_track_id += 1;
    open.selected = index;
    Ok(json!({"index": index, "name": name}))
}

pub(crate) fn track_remove(state: &mut EngineState, track: &TrackRef) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let index = resolve_track(&open.project, track)?;
    if open.recording {
        return Err("cannot remove a track while recording".to_string());
    }
    let mut project = open.project.clone();
    let removed = project.tracks.remove(index);
    let mut track_ids = open.track_ids.clone();
    track_ids.remove(index);
    let moved_from = (index, MAX_BUSES);
    swap_project_with_ids(state, project, track_ids, moved_from, &mut Vec::new())?;
    let open = open_project_mut(state);
    if index < open.selected {
        open.selected -= 1;
    }
    open.selected = open
        .selected
        .min(open.project.tracks.len().saturating_sub(1));
    let inputs = track_inputs(&open.project);
    state.projects.events.push(ProjectEvent::Inputs(inputs));
    Ok(json!({"index": index, "name": removed.name}))
}

/// The values of a `track_set` request; `None` leaves that value as it is.
pub(crate) struct TrackChange {
    pub input: Option<String>,
    pub volume: Option<f32>,
    pub pan: Option<f32>,
    pub mute: Option<bool>,
    pub solo: Option<bool>,
    pub arm: Option<bool>,
    pub name: Option<String>,
    pub to_master: Option<bool>,
}

/// Checks every value against the Project's rules, and the input against the capture ports when `ports` is given, before changing anything; a rename rebuilds the Arrangement, the rest are commands.
pub(crate) fn track_set(
    state: &mut EngineState,
    ports: Option<&dyn AudioPorts>,
    track: &TrackRef,
    change: &TrackChange,
) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let index = resolve_track(&open.project, track)?;
    let old = &open.project.tracks[index];
    let mut new = old.clone();
    if let Some(name) = &change.name {
        new.name = checked_name(&open.project.tracks, name, Some(index))?;
    }
    if let Some(input) = &change.input {
        if open.recording && (old.arm || change.arm == Some(true)) {
            return Err("cannot change an armed track's input while recording".to_string());
        }
        new.input = Some(input.clone()).filter(|input| !input.is_empty());
        if let (Some(input), Some(ports)) = (&new.input, ports)
            && !ports.audio_ports().0.iter().any(|port| port.name == *input)
        {
            return Err(format!("no capture port '{input}'"));
        }
    }
    new.volume_db = change.volume.unwrap_or(new.volume_db);
    new.pan = change.pan.unwrap_or(new.pan);
    new.mute = change.mute.unwrap_or(new.mute);
    new.solo = change.solo.unwrap_or(new.solo);
    new.arm = change.arm.unwrap_or(new.arm);
    new.to_master = change.to_master.unwrap_or(new.to_master);
    check_track(&new).map_err(|err| err.to_string())?;
    let renamed = new.name != old.name;
    let mut project = open.project.clone();
    project.tracks[index] = new.clone();
    if renamed {
        swap_project(state, project, &mut Vec::new())?;
    } else {
        let commands: Vec<ArrangementCommand> = [
            change
                .volume
                .map(|db| ArrangementCommand::Volume { track: index, db }),
            change
                .pan
                .map(|pan| ArrangementCommand::Pan { track: index, pan }),
            change
                .mute
                .map(|on| ArrangementCommand::Mute { track: index, on }),
            change
                .solo
                .map(|on| ArrangementCommand::Solo { track: index, on }),
            change
                .arm
                .map(|on| ArrangementCommand::Arm { track: index, on }),
            change
                .to_master
                .map(|on| ArrangementCommand::ToMaster { track: index, on }),
        ]
        .into_iter()
        .flatten()
        .collect();
        send(state, &commands)?;
        let open = open_project_mut(state);
        open.project = project;
        open.changed = true;
        state.revision += 1;
    }
    if change.input.is_some() {
        let inputs = track_inputs(&open_project(state).project);
        state.projects.events.push(ProjectEvent::Inputs(inputs));
    }
    Ok(json!({
        "index": index,
        "name": new.name,
        "input": new.input,
        "volume_db": new.volume_db,
        "pan": new.pan,
        "mute": new.mute,
        "solo": new.solo,
        "arm": new.arm,
        "to_master": new.to_master,
    }))
}

pub(crate) fn track_select(state: &mut EngineState, track: &TrackRef) -> Result<Value, String> {
    let open = state.projects.open.as_mut().ok_or(NO_PROJECT)?;
    let index = resolve_track(&open.project, track)?;
    open.selected = index;
    state.revision += 1;
    Ok(json!({"index": index, "name": open.project.tracks[index].name}))
}

pub(crate) fn transport(
    state: &mut EngineState,
    action: TransportAction,
    position: Option<&Position>,
) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let (command, frames) = match action {
        TransportAction::Play | TransportAction::Record
            if let Some(mismatch) = project_rate_warning(state) =>
        {
            return Err(mismatch);
        }
        TransportAction::Play => (ArrangementCommand::Play, None),
        TransportAction::Stop => (ArrangementCommand::Stop, None),
        TransportAction::Record => {
            if !open.project.tracks.iter().any(|track| track.arm) {
                return Err(NOTHING_ARMED.to_string());
            }
            (ArrangementCommand::Record, None)
        }
        TransportAction::Locate => {
            let frame = locate_frame(&open.project, position.ok_or(NEEDS_POSITION)?)?;
            (ArrangementCommand::Locate(frame), Some(frame))
        }
    };
    send(state, &[command])?;
    let open = open_project_mut(state);
    match action {
        TransportAction::Record => {
            open.recording = true;
            open.recorded = true;
        }
        TransportAction::Play | TransportAction::Stop => open.recording = false,
        TransportAction::Locate => {}
    }
    let mut payload = json!({ "action": action });
    if action == TransportAction::Stop {
        // a failed write leaves the Project changed, so the timer retries and logs it
        autosave_on_stop(open);
    }
    if let Some(frames) = frames {
        payload["frames"] = json!(frames);
    }
    Ok(payload)
}

pub(crate) fn set_loop(state: &mut EngineState, region: LoopRegion) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let mut project = open.project.clone();
    project.loop_region = region;
    check_project(&project).map_err(|err| err.to_string())?;
    send(state, &[ArrangementCommand::Loop(region)])?;
    commit(state, project);
    Ok(json!({"start": region.start, "end": region.end, "on": region.on}))
}

pub(crate) fn tempo(
    state: &mut EngineState,
    bpm: Option<f64>,
    meter: Option<(u32, u32)>,
) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let mut project = open.project.clone();
    project.tempo_bpm = bpm.unwrap_or(project.tempo_bpm);
    project.meter = meter.unwrap_or(project.meter);
    check_project(&project).map_err(|err| err.to_string())?;
    let (bpm, meter) = (project.tempo_bpm, project.meter);
    send(state, &[ArrangementCommand::Tempo { bpm, meter }])?;
    commit(state, project);
    Ok(json!({"bpm": bpm, "meter": [meter.0, meter.1]}))
}

pub(crate) fn click(
    state: &mut EngineState,
    on: Option<bool>,
    count_in: Option<u32>,
) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let mut project = open.project.clone();
    project.click = on.unwrap_or(project.click);
    project.count_in = count_in.unwrap_or(project.count_in);
    check_project(&project).map_err(|err| err.to_string())?;
    let (on, count_in) = (project.click, project.count_in);
    send(state, &click_commands(&project))?;
    commit(state, project);
    Ok(json!({"on": on, "count_in": count_in}))
}

/// Each Track's id, name and Items, with each Item's `length` the frames of its active Take it plays.
pub(crate) fn items(state: &EngineState) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let project = &open.project;
    let tracks: Vec<Value> = project
        .tracks
        .iter()
        .zip(&open.track_ids)
        .map(|(track, id)| {
            let items: Vec<Value> = track
                .items
                .iter()
                .map(|item| {
                    let files: Vec<&str> =
                        item.takes.iter().map(|take| take.file.as_str()).collect();
                    json!({
                        "position": item.position,
                        "bars_beats": bars_beats(project, item.position),
                        "offset": item.offset,
                        "length": item.takes[item.take].frames.saturating_sub(item.offset),
                        "take": item.take,
                        "takes": files,
                    })
                })
                .collect();
            json!({"id": id, "name": track.name, "items": items})
        })
        .collect();
    Ok(json!({"sample_rate": project.sample_rate, "tracks": tracks}))
}

/// The Project fields of `status`, with `track_peaks` the input peaks of each Track and `mix` the post-fader peaks since this connection's last `status`.
pub(crate) fn status_fields(
    state: &EngineState,
    track_peaks: &[f32],
    mix: &MixPeaks,
) -> Map<String, Value> {
    let shared = &state.shared;
    let mut fields = Map::new();
    fields.insert(
        "disk_overflows".to_string(),
        json!(shared.disk_overflows.load(Ordering::Relaxed)),
    );
    fields.insert(
        "count_in_remaining".to_string(),
        json!(shared.count_in_remaining.load(Ordering::Relaxed)),
    );
    let loaded_audio_bytes: usize = state.projects.open.as_ref().map_or(0, |open| {
        open.takes
            .values()
            .map(|samples| size_of_val(&**samples))
            .sum()
    });
    fields.insert("loaded_audio_bytes".to_string(), json!(loaded_audio_bytes));
    fields.insert(
        "project_warning".to_string(),
        json!(project_rate_warning(state)),
    );
    let Some(open) = state.projects.open.as_ref() else {
        for key in [
            "project",
            "transport",
            "position",
            "tempo",
            "loop",
            "click",
            "selected",
        ] {
            fields.insert(key.to_string(), Value::Null);
        }
        fields.insert("tracks".to_string(), json!([]));
        fields.insert("buses".to_string(), json!([]));
        fields.insert("master".to_string(), Value::Null);
        return fields;
    };
    let project = &open.project;
    let frames = shared.playhead.load(Ordering::Relaxed);
    let transport = match transport_from_code(shared.transport.load(Ordering::Relaxed)) {
        Transport::Stopped => "stopped",
        Transport::Playing => "playing",
        Transport::Recording => "recording",
    };
    let meters = &shared.mix_meters;
    let tracks: Vec<Value> = project
        .tracks
        .iter()
        .zip(track_peaks)
        .enumerate()
        .map(|(index, (track, &peak))| {
            json!({
                "name": track.name,
                "input": track.input,
                "volume_db": track.volume_db,
                "pan": track.pan,
                "mute": track.mute,
                "solo": track.solo,
                "armed": track.arm,
                "input_peak_db": peak_db(peak),
                "to_master": track.to_master,
                "sends": sends_status(project, track),
                "peak_db": peaks_db(mix.tracks[index]),
                "clip": meters.clip(MeterPoint::Track(index)),
            })
        })
        .collect();
    let LoopRegion { start, end, on } = project.loop_region;
    for (key, value) in [
        ("project", json!(project.name)),
        ("transport", json!(transport)),
        (
            "position",
            json!({"frames": frames, "bars_beats": bars_beats(project, frames)}),
        ),
        (
            "tempo",
            json!({"bpm": project.tempo_bpm, "meter": [project.meter.0, project.meter.1]}),
        ),
        ("loop", json!({"start": start, "end": end, "on": on})),
        (
            "click",
            json!({"on": project.click, "count_in": project.count_in}),
        ),
        ("selected", json!(open.selected)),
        ("tracks", json!(tracks)),
        ("buses", buses_status(project, mix, meters)),
        ("master", master_status(project, mix, meters)),
    ] {
        fields.insert(key.to_string(), value);
    }
    fields
}

fn open(
    state: &mut EngineState,
    dir: PathBuf,
    project: Project,
    takes: HashMap<String, Arc<[f32]>>,
) -> Result<(), String> {
    let track_ids: Vec<u64> = (0..project.tracks.len() as u64).collect();
    let (readers, _) = start_arrangement(
        &state.config,
        &mut state.queues,
        state.sample_rate,
        state.max_frames,
        &Layout {
            project: &project,
            takes: &takes,
            track_ids: &track_ids,
            moved_from: NOTHING_MOVED,
        },
    )?;
    // The swap is queued, so the commands after it reach the new Arrangement; a full queue leaves the click off until the next `click`.
    let _ = send(state, &click_commands(&project));
    let events = &mut state.projects.events;
    events.push(ProjectEvent::Opened {
        dir: dir.clone(),
        sample_rate: project.sample_rate,
    });
    events.push(ProjectEvent::Readers(readers));
    events.push(ProjectEvent::Inputs(track_inputs(&project)));
    state.projects.open = Some(OpenProject {
        dir,
        project,
        takes,
        selected: 0,
        recording: false,
        recorded: false,
        next_track_id: track_ids.len() as u64,
        track_ids,
        changed: false,
        last_autosave: Instant::now(),
        unplaced: Vec::new(),
    });
    state.revision += 1;
    Ok(())
}

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
fn swap_project_with_ids(
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
    open_project_mut(state).track_ids = track_ids;
    commit(state, project);
    Ok(())
}

/// The commands [`rebuild_arrangement`] may push.
pub(crate) fn rebuild_arrangement_slots(state: &EngineState) -> usize {
    if state.projects.open.is_some() { 3 } else { 0 }
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
        open_project_mut(state).recording = false;
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
    let clicks = click_commands(&open.project);
    state.projects.events.push(ProjectEvent::Readers(readers));
    send(state, &clicks)?;
    Ok(warnings)
}

/// Why the open Project does not play: its sample rate differs from the Engine's.
pub fn project_rate_warning(state: &EngineState) -> Option<String> {
    let open = state.projects.open.as_ref()?;
    rate_mismatch(open.project.sample_rate, state.sample_rate)
}

fn rate_mismatch(project_rate: u32, engine_rate: f32) -> Option<String> {
    let engine_rate = engine_rate as u32;
    (project_rate != engine_rate).then(|| {
        format!("project rate {project_rate} Hz differs from the engine rate {engine_rate} Hz")
    })
}

fn click_commands(project: &Project) -> [ArrangementCommand; 2] {
    [
        ArrangementCommand::Click(project.click),
        ArrangementCommand::CountIn(project.count_in),
    ]
}

pub(crate) fn commit(state: &mut EngineState, project: Project) {
    let open = open_project_mut(state);
    open.project = project;
    open.changed = true;
    state.revision += 1;
}

/// What [`start_arrangement`] builds an Arrangement from.
struct Layout<'a> {
    project: &'a Project,
    takes: &'a HashMap<String, Arc<[f32]>>,
    track_ids: &'a [u64],
    /// The first Track and Bus index that moved or went since the running Arrangement, as [`Arrangement::clear_clips_from`] takes them.
    moved_from: (usize, usize),
}

fn start_arrangement(
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
    for &command in commands {
        state
            .queues
            .commands
            .push(Command::Arrangement(command))
            .map_err(|_| ENGINE_BUSY.to_string())?;
    }
    Ok(())
}

fn load_takes(dir: &Path, project: &Project) -> Result<HashMap<String, Arc<[f32]>>, String> {
    let mut takes = HashMap::new();
    for item in project.tracks.iter().flat_map(|track| &track.items) {
        let file = &item.takes[item.take].file;
        if takes.contains_key(file) {
            continue;
        }
        let samples =
            load_take(dir, file).map_err(|err| format!("cannot load take {file}: {err}"))?;
        takes.insert(file.clone(), samples);
    }
    Ok(takes)
}

pub(crate) fn resolve_track(project: &Project, track: &TrackRef) -> Result<usize, String> {
    resolve_ref(
        "track",
        project.tracks.iter().map(|t| t.name.as_str()),
        track,
    )
}

pub(crate) fn resolve_bus(project: &Project, bus: &BusRef) -> Result<usize, String> {
    resolve_ref("bus", project.buses.iter().map(|b| b.name.as_str()), bus)
}

fn resolve_ref<'a>(
    kind: &str,
    mut names: impl Iterator<Item = &'a str>,
    wanted: &TrackRef,
) -> Result<usize, String> {
    let found = match wanted {
        TrackRef::Index(index) => names.nth(*index).map(|_| *index),
        TrackRef::Name(name) => names.position(|other| other == name),
    };
    found.ok_or_else(|| match wanted {
        TrackRef::Index(index) => format!("no {kind} '{index}'"),
        TrackRef::Name(name) => format!("no {kind} '{name}'"),
    })
}

#[allow(
    clippy::maybe_infinite_iter,
    reason = "a Project has at most 16 Tracks, so some number is free"
)]
fn free_track_name(project: &Project) -> String {
    (project.tracks.len() + 1..)
        .map(|n| format!("{DEFAULT_TRACK_NAME} {n}"))
        .find(|name| checked_name(&project.tracks, name, None).is_ok())
        .expect("a Project has at most 16 Tracks, so some number is free")
}

fn track_inputs(project: &Project) -> Vec<Option<String>> {
    project
        .tracks
        .iter()
        .map(|track| track.input.clone())
        .collect()
}

fn frames_per_beat(project: &Project) -> f64 {
    f64::from(project.sample_rate) * 60.0 / project.tempo_bpm
}

/// The frame of `<bar>.<beat>` or `<bar>` (both from 1, the beat within the meter), `<seconds>s` or a frame number, at most an hour past the Project's end.
fn locate_frame(project: &Project, position: &Position) -> Result<u64, String> {
    let frame = match position {
        Position::Frames(frames) => *frames as f64,
        Position::Text(text) => text_frame(project, text.trim())?,
    };
    let last =
        project_end(project) as f64 + LOCATE_SECONDS_PAST_END * f64::from(project.sample_rate);
    if frame > last {
        return Err(POSITION_TOO_FAR.to_string());
    }
    Ok(frame as u64)
}

/// The frame a text position names, rounded; not bounded.
fn text_frame(project: &Project, text: &str) -> Result<f64, String> {
    if let Some(seconds) = text.strip_suffix(SECONDS_SUFFIX) {
        let seconds: f64 = seconds.parse().map_err(|_| BAD_POSITION)?;
        if !seconds.is_finite() || seconds.is_sign_negative() {
            return Err(BAD_POSITION.to_string());
        }
        return Ok((seconds * f64::from(project.sample_rate)).round());
    }
    let (bar, beat) = text.split_once('.').unwrap_or((text, "1"));
    let (Ok(bar), Ok(beat)) = (bar.parse::<u64>(), beat.parse::<u64>()) else {
        return Err(BAD_POSITION.to_string());
    };
    let beats_per_bar = u64::from(project.meter.0);
    if bar == 0 || beat == 0 || beat > beats_per_bar {
        return Err(BAD_POSITION.to_string());
    }
    let beat_index = (bar - 1) as f64 * beats_per_bar as f64 + (beat - 1) as f64;
    Ok((beat_index * frames_per_beat(project)).round())
}

/// The last frame any Take or the loop region reaches.
fn project_end(project: &Project) -> u64 {
    let items = project.tracks.iter().flat_map(|track| &track.items);
    let item_ends = items.flat_map(|item| {
        item.takes
            .iter()
            .map(|take| item.position.saturating_add(take.frames))
    });
    item_ends
        .chain([project.loop_region.end])
        .max()
        .unwrap_or(0)
}

fn bars_beats(project: &Project, frames: u64) -> String {
    let beat = (frames as f64 / frames_per_beat(project)).floor() as u64;
    let beats_per_bar = u64::from(project.meter.0);
    format!("{}.{}", beat / beats_per_bar + 1, beat % beats_per_bar + 1)
}

fn open_project(state: &EngineState) -> &OpenProject {
    state
        .projects
        .open
        .as_ref()
        .expect("callers check that a Project is open")
}

fn open_project_mut(state: &mut EngineState) -> &mut OpenProject {
    state
        .projects
        .open
        .as_mut()
        .expect("callers check that a Project is open")
}

/// Creates `dir` if missing; the error names it.
pub(crate) fn create_projects_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|err| format!("cannot create projects_dir {}: {err}", dir.display()))
}
