use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::history::History;
use crate::project::{Item, LoadWarning, Project};
use crate::render::{RenderJob, RenderTask};
use crate::take_capture::{FinishedTake, TrackReader, WritingTakes};
use xguitare_protocol::{BusRef, IndexOrName, STILL_CLOSING, TrackRef};

pub(crate) const NO_PROJECT: &str = "no project open";
const PROJECT_IS_OPEN: &str = "a project is open; close it first";

/// What `project_control::read_project` reads: the request's Project in `projects_dir`, taken when [`Projects`]' `generation` was `generation`.
#[derive(Clone, Debug)]
pub struct ReadTicket {
    pub(crate) projects_dir: PathBuf,
    pub(crate) name: String,
    pub(crate) restore: bool,
    pub(crate) generation: u64,
}

/// A Project read from disk outside the state lock for `control::handle_request`, or the error the read gave.
pub struct ReadProject {
    pub(crate) ticket: ReadTicket,
    pub(crate) files: Result<ProjectFiles, String>,
}

/// A Project's folder, its loaded state and warnings, whether its autosave is newer, and its active Takes' audio or the error decoding one gave.
pub(crate) struct ProjectFiles {
    pub(crate) dir: PathBuf,
    pub(crate) project: Project,
    pub(crate) warnings: Vec<LoadWarning>,
    pub(crate) autosave_newer: bool,
    pub(crate) takes: Result<LoadedTakes, String>,
}

/// The open Project, if any, and what the Daemon must do outside the Engine because of it.
#[derive(Default)]
pub struct Projects {
    pub(crate) open: Option<OpenProject>,
    pub(crate) events: Vec<ProjectEvent>,
    /// Closed Projects that recorded and whose last Takes the Daemon has not yet added and saved.
    pub(crate) closing: usize,
    /// The running Render, or the last one until the next starts.
    pub(crate) render: Option<Arc<RenderJob>>,
    /// Whether `render_control::finished_render` has reported the end of `render`.
    pub(crate) render_reported: bool,
    /// Counts each Project open and close and each `project_control::finish_closed`, so a [`ReadTicket`] taken before one is not installed after it.
    pub(crate) generation: u64,
}

pub(crate) struct OpenProject {
    pub(crate) dir: PathBuf,
    pub(crate) project: Project,
    pub(crate) takes: HashMap<String, Arc<[f32]>>,
    /// The peak table of each Take in `takes`, with the samples it was built from, kept by `project_control::sync_peaks`.
    pub(crate) peaks: HashMap<String, (Arc<[f32]>, Vec<i8>)>,
    pub(crate) selected: usize,
    /// Whether the last transport request was `record`; `play` and `stop` end it.
    pub(crate) recording: bool,
    /// Whether `record` was requested since it was opened, so Takes may still be on their way when it closes.
    pub(crate) recorded: bool,
    /// One id per Track, in Track order, that stays the Track's while others are added or removed; finished Takes name their Track by it.
    pub(crate) track_ids: Vec<u64>,
    pub(crate) next_track_id: u64,
    /// Never reused, so a client holding a removed Bus's id cannot reach a new Bus by it.
    pub(crate) next_bus_id: u64,
    /// Whether the Project changed since it was opened, saved or autosaved.
    pub(crate) changed: bool,
    pub(crate) last_autosave: Instant,
    /// Finished Takes not yet in `project` because the Engine was busy; `project_control::add_finished_takes` retries them.
    pub(crate) unplaced: Vec<FinishedTake>,
    pub(crate) history: History,
    pub(crate) clipboard: Vec<CopiedItem>,
    pub(crate) writing: WritingTakes,
}

/// The open Project's Take audio, keyed by Take file.
pub(crate) type LoadedTakes = HashMap<String, Arc<[f32]>>;

/// Take readers, one per Track in Track order.
pub type TrackReaders = Vec<TrackReader>;

/// Work for the Daemon, in the order it arose.
#[allow(
    clippy::large_enum_variant,
    reason = "a few events per Control API request; boxing the closed Project saves nothing that matters"
)]
pub enum ProjectEvent {
    /// Start a disk thread writing Takes under `dir` at `sample_rate` that keeps `writing` current, and log what loading the Project changed.
    Opened {
        dir: PathBuf,
        sample_rate: u32,
        writing: WritingTakes,
        warnings: Vec<LoadWarning>,
    },
    /// Queue these Take readers on the disk thread.
    Readers(TrackReaders),
    /// Connect Track n's input port to `inputs[n]`, disconnecting the rest.
    Inputs(Vec<Option<String>>),
    /// Stop the disk thread and hand the Takes it still finishes to `project_control::finish_closed`.
    Closed(ClosedProject),
    /// Run this Render on a thread of its own.
    Render(RenderTask),
}

pub struct ClosedProject {
    pub dir: PathBuf,
    pub project: Project,
    /// Whether it changed since it was last saved or autosaved.
    pub changed: bool,
    pub(crate) track_ids: Vec<u64>,
    /// Whether it counts in [`Projects`]' `closing` until `project_control::finish_closed` adds its last Takes.
    pub(crate) takes_pending: bool,
    /// Finished Takes the Engine was too busy to take before the close; added with the last Takes.
    pub(crate) unplaced: Vec<FinishedTake>,
}

/// An Item in the clipboard: `item.position` counted from the start of the group copied, `track` its Track's distance from the Track of the first Item named.
#[derive(Clone)]
pub(crate) struct CopiedItem {
    pub track: isize,
    pub item: Item,
}

/// The open Project, or `no project open`.
pub(crate) fn current_project(projects: &Projects) -> Result<&Project, String> {
    projects
        .open
        .as_ref()
        .map(|open| &open.project)
        .ok_or_else(|| NO_PROJECT.to_string())
}

pub(crate) fn check_none_open(projects: &Projects) -> Result<(), String> {
    if projects.open.is_some() {
        return Err(PROJECT_IS_OPEN.to_string());
    }
    if projects.closing > 0 {
        return Err(STILL_CLOSING.to_string());
    }
    Ok(())
}

/// The open Project's loaded Take `file`: its frames and its peak table.
pub(crate) fn loaded_peaks<'a>(
    projects: &'a Projects,
    file: &str,
) -> Result<(usize, &'a [i8]), String> {
    let open = projects.open.as_ref().ok_or(NO_PROJECT)?;
    open.peaks
        .get(file)
        .map(|(samples, table)| (samples.len(), table.as_slice()))
        .ok_or_else(|| format!("no such take: {file}"))
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
    wanted: &IndexOrName,
) -> Result<usize, String> {
    let found = match wanted {
        IndexOrName::Index(index) => names.nth(*index).map(|_| *index),
        IndexOrName::Name(name) => names.position(|other| other == name),
    };
    found.ok_or_else(|| match wanted {
        IndexOrName::Index(index) => format!("no {kind} '{index}'"),
        IndexOrName::Name(name) => format!("no {kind} '{name}'"),
    })
}

/// The open Project's folder, its current state and its loaded Take audio.
pub(crate) fn open_project_state(projects: &Projects) -> Option<(&Path, &Project, &LoadedTakes)> {
    let open = projects.open.as_ref()?;
    Some((&open.dir, &open.project, &open.takes))
}

pub(crate) fn open_project(projects: &Projects) -> &OpenProject {
    projects
        .open
        .as_ref()
        .expect("callers check that a Project is open")
}

/// An id for a new Bus of the open Project; each call gives another.
pub(crate) fn new_bus_id(projects: &mut Projects) -> u64 {
    let open = open_project_mut(projects);
    open.next_bus_id += 1;
    open.next_bus_id - 1
}

pub(crate) fn open_project_mut(projects: &mut Projects) -> &mut OpenProject {
    projects
        .open
        .as_mut()
        .expect("callers check that a Project is open")
}
