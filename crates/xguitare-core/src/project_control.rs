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
use crate::control::{EngineState, Position, TrackRef, TransportAction, peak_db, slug};
use crate::engine::{Command, transport_from_code};
use crate::ports::AudioPorts;
use crate::project::{
    LoopRegion, PROJECT_FILE, Project, Track, autosave_due, autosave_newer, check_project,
    check_track,
};
use crate::take_capture::{FinishedTake, Latency, TakeReader, add_finished_take, load_take};
use crate::{BuildWarning, Config, Preset, PresetModule};

pub(crate) const NO_PROJECT: &str = "no project open";
const PROJECT_IS_OPEN: &str = "a project is open; close it first";
const NO_TRACKS: &str = "the project has no tracks";
const EMPTY_TRACK_NAME: &str = "track name must not be empty";
const NOTHING_ARMED: &str = "no track is armed";
const NEEDS_POSITION: &str = "locate needs a position";
const BAD_POSITION: &str = "position must be <bar>.<beat> or <seconds>s";
const BAD_COUNT_IN: &str = "count-in must be 0, 1 or 2 bars";
const MAX_COUNT_IN_BARS: u32 = 2;
const DEFAULT_TRACK_NAME: &str = "Track";
const SECONDS_SUFFIX: char = 's';

/// The open Project, if any, and what the Daemon must do outside the Engine because of it.
#[derive(Default)]
pub struct Projects {
    open: Option<OpenProject>,
    events: Vec<ProjectEvent>,
}

struct OpenProject {
    dir: PathBuf,
    project: Project,
    takes: HashMap<String, Arc<[f32]>>,
    selected: usize,
    /// Whether the last transport request was `record`; `play` and `stop` end it.
    recording: bool,
    click: bool,
    count_in: u32,
    /// Whether the Project changed since it was opened, saved or autosaved.
    changed: bool,
    last_autosave: Instant,
}

/// Take readers, one per Track in Track order, each with its Track's name.
pub type TrackReaders = Vec<(String, TakeReader)>;

/// Work for the Daemon, in the order it arose.
pub enum ProjectEvent {
    /// Start a disk thread writing Takes under `dir` at `sample_rate`.
    Opened { dir: PathBuf, sample_rate: u32 },
    /// Queue these Take readers, one per Track in Track order with the Track's name, on the disk thread.
    Readers(TrackReaders),
    /// Connect Track n's input port to `inputs[n]`, disconnecting the rest.
    Inputs(Vec<Option<String>>),
    /// Stop the disk thread; the Takes it still finishes belong to this Project, to be added and saved.
    Closed(ClosedProject),
}

pub struct ClosedProject {
    pub dir: PathBuf,
    pub project: Project,
    /// Whether it changed since it was last saved or autosaved.
    pub changed: bool,
}

/// The events since the last call.
pub fn take_project_events(state: &mut EngineState) -> Vec<ProjectEvent> {
    std::mem::take(&mut state.projects.events)
}

/// Adds the Takes the disk thread finished, each with its Track's latency, to the open Project and swaps in an Arrangement that plays them; returns one message per Take that could not be added.
pub fn add_finished_takes(
    state: &mut EngineState,
    finished: &[(FinishedTake, Latency)],
) -> Vec<String> {
    let Some(open) = state.projects.open.as_mut() else {
        return Vec::new();
    };
    let mut project = open.project.clone();
    let mut failures = Vec::new();
    for (take, latency) in finished {
        if let Err(err) =
            add_finished_take(&mut project, &open.dir, &mut open.takes, take, *latency)
        {
            failures.push(format!("cannot load take {}: {err}", take.file));
        }
    }
    let mut warnings = Vec::new();
    if let Err(err) = swap_project(state, project, &mut warnings) {
        failures.push(format!("cannot play the new takes: {err}"));
    }
    failures.extend(warnings.iter().map(ToString::to_string));
    failures
}

/// Writes `project.autosave.toml` when the open Project changed and the autosave interval has passed since it was opened, saved or autosaved; returns the file written, or `None` when none was due.
pub fn autosave_if_due(state: &mut EngineState, now: Instant) -> Option<Result<PathBuf, String>> {
    let open = state.projects.open.as_mut()?;
    if !autosave_due(open.last_autosave, now, open.changed) {
        return None;
    }
    open.last_autosave = now;
    open.changed = false;
    let file = open.dir.join(crate::project::AUTOSAVE_FILE);
    Some(
        open.project
            .save_autosave(&open.dir)
            .map(|()| file)
            .map_err(|err| err.to_string()),
    )
}

/// Takes the open Project out as the Daemon stops; the Engine is not told.
pub fn close_for_stop(state: &mut EngineState) -> Option<ClosedProject> {
    state.projects.open.take().map(|open| ClosedProject {
        dir: open.dir,
        project: open.project,
        changed: open.changed,
    })
}

/// The Track a Rack request edits: `None` while no Project is open and no Track is named, else the named Track or the selected one.
pub(crate) fn target_track(
    state: &EngineState,
    track: Option<&TrackRef>,
) -> Result<Option<usize>, String> {
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

/// The Track's Modules as a Preset named after it, for the Rack requests to read and edit.
pub(crate) fn track_preset(state: &EngineState, track: usize) -> Preset {
    let track = &open_project(state).project.tracks[track];
    Preset {
        name: track.name.clone(),
        modules: track.modules.clone(),
    }
}

/// Replaces the Track's Modules and swaps in the rebuilt Arrangement; on an error nothing changes.
pub(crate) fn set_track_modules(
    state: &mut EngineState,
    track: usize,
    modules: Vec<PresetModule>,
    warnings: &mut Vec<BuildWarning>,
) -> Result<(), String> {
    let mut project = open_project(state).project.clone();
    project.tracks[track].modules = modules;
    swap_project(state, project, warnings)
}

/// Sends `command` to the Arrangement, then applies `change` to the Track's Modules.
pub(crate) fn change_track_module(
    state: &mut EngineState,
    track: usize,
    command: ArrangementCommand,
    change: impl FnOnce(&mut Vec<PresetModule>),
) -> Result<(), String> {
    send(state, &[command])?;
    let open = open_project_mut(state);
    change(&mut open.project.tracks[track].modules);
    open.changed = true;
    state.revision += 1;
    Ok(())
}

pub(crate) fn project_new(state: &mut EngineState, name: &str) -> Result<Value, String> {
    if state.projects.open.is_some() {
        return Err(PROJECT_IS_OPEN.to_string());
    }
    let (dir, mut project) =
        Project::new(&state.config.projects_dir, name).map_err(|err| err.to_string())?;
    project.sample_rate = state.sample_rate as u32;
    project.save(&dir).map_err(|err| err.to_string())?;
    let payload = json!({"name": project.name, "path": dir.display().to_string()});
    open(state, dir, project, HashMap::new())?;
    Ok(payload)
}

pub(crate) fn project_open(
    state: &mut EngineState,
    name: &str,
    restore: bool,
) -> Result<Value, String> {
    if state.projects.open.is_some() {
        return Err(PROJECT_IS_OPEN.to_string());
    }
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
    let engine_rate = state.sample_rate as u32;
    if project.sample_rate != engine_rate {
        return Err(format!(
            "project rate {} Hz differs from the engine rate {engine_rate} Hz",
            project.sample_rate
        ));
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
        }));
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
        Some(name) => checked_name(&open.project, name, None)?,
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
    swap_project(state, project, &mut Vec::new())?;
    open_project_mut(state).selected = index;
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
    swap_project(state, project, &mut Vec::new())?;
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
        new.name = checked_name(&open.project, name, Some(index))?;
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
        TransportAction::Record => open.recording = true,
        TransportAction::Play | TransportAction::Stop => open.recording = false,
        TransportAction::Locate => {}
    }
    let mut payload = json!({ "action": action });
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
    if count_in.is_some_and(|bars| bars > MAX_COUNT_IN_BARS) {
        return Err(BAD_COUNT_IN.to_string());
    }
    let (on, count_in) = (on.unwrap_or(open.click), count_in.unwrap_or(open.count_in));
    send(
        state,
        &[
            ArrangementCommand::Click(on),
            ArrangementCommand::CountIn(count_in),
        ],
    )?;
    let open = open_project_mut(state);
    (open.click, open.count_in) = (on, count_in);
    state.revision += 1;
    Ok(json!({"on": on, "count_in": count_in}))
}

/// The Project fields of `status`, with `track_peaks` the input peaks of each Track since this connection's last `status`.
pub(crate) fn status_fields(state: &EngineState, track_peaks: &[f32]) -> Map<String, Value> {
    let shared = &state.shared;
    let mut fields = Map::new();
    fields.insert(
        "disk_overflows".to_string(),
        json!(shared.disk_overflows.load(Ordering::Relaxed)),
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
        return fields;
    };
    let project = &open.project;
    let frames = shared.playhead.load(Ordering::Relaxed);
    let transport = match transport_from_code(shared.transport.load(Ordering::Relaxed)) {
        Transport::Stopped => "stopped",
        Transport::Playing => "playing",
        Transport::Recording => "recording",
    };
    let tracks: Vec<Value> = project
        .tracks
        .iter()
        .zip(track_peaks)
        .map(|(track, &peak)| {
            json!({
                "name": track.name,
                "input": track.input,
                "volume_db": track.volume_db,
                "pan": track.pan,
                "mute": track.mute,
                "solo": track.solo,
                "armed": track.arm,
                "input_peak_db": peak_db(peak),
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
            json!({"on": open.click, "count_in": open.count_in}),
        ),
        ("selected", json!(open.selected)),
        ("tracks", json!(tracks)),
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
    let (readers, _) = start_arrangement(
        &state.config,
        &mut state.queues,
        state.sample_rate,
        state.max_frames,
        &project,
        &takes,
    )?;
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
        click: false,
        count_in: 0,
        changed: false,
        last_autosave: Instant::now(),
    });
    state.revision += 1;
    Ok(())
}

/// Builds `project`'s Arrangement, swaps it in and makes `project` the open Project's; on an error nothing changes.
fn swap_project(
    state: &mut EngineState,
    project: Project,
    warnings: &mut Vec<BuildWarning>,
) -> Result<(), String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let (readers, build_warnings) = start_arrangement(
        &state.config,
        &mut state.queues,
        state.sample_rate,
        state.max_frames,
        &project,
        &open.takes,
    )?;
    warnings.extend(build_warnings);
    state.projects.events.push(ProjectEvent::Readers(readers));
    commit(state, project);
    Ok(())
}

fn commit(state: &mut EngineState, project: Project) {
    let open = open_project_mut(state);
    open.project = project;
    open.changed = true;
    state.revision += 1;
}

fn start_arrangement(
    config: &Config,
    queues: &mut crate::engine::EngineQueues,
    sample_rate: f32,
    max_frames: usize,
    project: &Project,
    takes: &HashMap<String, Arc<[f32]>>,
) -> Result<(TrackReaders, Vec<BuildWarning>), String> {
    let (mut arrangement, warnings) =
        build_arrangement(config, project, takes, sample_rate, max_frames)
            .map_err(|err| err.to_string())?;
    let names = project.tracks.iter().map(|track| track.name.clone());
    let readers = names.zip(arrangement.take_readers()).collect();
    swap_arrangement(queues, arrangement)?;
    Ok((readers, warnings))
}

fn send(state: &mut EngineState, commands: &[ArrangementCommand]) -> Result<(), String> {
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

fn resolve_track(project: &Project, track: &TrackRef) -> Result<usize, String> {
    let found = match track {
        TrackRef::Index(index) => (*index < project.tracks.len()).then_some(*index),
        TrackRef::Name(name) => project.tracks.iter().position(|t| t.name == *name),
    };
    found.ok_or_else(|| match track {
        TrackRef::Index(index) => format!("no track '{index}'"),
        TrackRef::Name(name) => format!("no track '{name}'"),
    })
}

/// The name without control characters or surrounding blanks, refused when empty or when another Track has it in any letter case.
fn checked_name(project: &Project, name: &str, except: Option<usize>) -> Result<String, String> {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let name = name.trim();
    if name.is_empty() {
        return Err(EMPTY_TRACK_NAME.to_string());
    }
    let taken = project.tracks.iter().enumerate().any(|(idx, track)| {
        Some(idx) != except && track.name.to_lowercase() == name.to_lowercase()
    });
    if taken {
        return Err(format!("a track named '{name}' already exists"));
    }
    Ok(name.to_string())
}

#[allow(
    clippy::maybe_infinite_iter,
    reason = "a Project has at most 16 Tracks, so some number is free"
)]
fn free_track_name(project: &Project) -> String {
    (project.tracks.len() + 1..)
        .map(|n| format!("{DEFAULT_TRACK_NAME} {n}"))
        .find(|name| checked_name(project, name, None).is_ok())
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

/// The frame of `<bar>.<beat>` or `<bar>` (both from 1, the beat within the meter) or `<seconds>s`; a frame number is taken as it is.
fn locate_frame(project: &Project, position: &Position) -> Result<u64, String> {
    let text = match position {
        Position::Frames(frames) => return Ok(*frames),
        Position::Text(text) => text.trim(),
    };
    if let Some(seconds) = text.strip_suffix(SECONDS_SUFFIX) {
        let seconds: f64 = seconds.parse().map_err(|_| BAD_POSITION)?;
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(BAD_POSITION.to_string());
        }
        return Ok((seconds * f64::from(project.sample_rate)).round() as u64);
    }
    let (bar, beat) = text.split_once('.').unwrap_or((text, "1"));
    let (Ok(bar), Ok(beat)) = (bar.parse::<u64>(), beat.parse::<u64>()) else {
        return Err(BAD_POSITION.to_string());
    };
    let beats_per_bar = u64::from(project.meter.0);
    if bar == 0 || beat == 0 || beat > beats_per_bar {
        return Err(BAD_POSITION.to_string());
    }
    let beat_index = (bar - 1) * beats_per_bar + (beat - 1);
    Ok((beat_index as f64 * frames_per_beat(project)).round() as u64)
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
