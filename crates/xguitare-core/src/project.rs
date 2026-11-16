use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::control::{slug, temp_path};
use crate::preset::{
    CABINET_TYPE, ModuleEntry, module_entries, parse_modules, parse_stereo_modules,
};
use crate::{Preset, PresetError, PresetModule};

pub const PROJECT_FILE: &str = "project.toml";
pub const AUTOSAVE_FILE: &str = "project.autosave.toml";
pub const TAKES_DIR: &str = "takes";
pub const MAX_TRACKS: usize = 16;
pub const MAX_BUSES: usize = 8;
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(60);

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_TEMPO_BPM: f64 = 120.0;
const DEFAULT_METER: (u32, u32) = (4, 4);
pub(crate) const TEMPO_BPM: RangeInclusive<f64> = 20.0..=300.0;
pub(crate) const METER_BEATS: RangeInclusive<u32> = 1..=16;
pub(crate) const METER_NOTES: [u32; 4] = [2, 4, 8, 16];
const LATENCY_OFFSET_MS: RangeInclusive<f64> = -1000.0..=1000.0;
pub(crate) const COUNT_IN_BARS: RangeInclusive<u32> = 0..=2;
const VOLUME_DB: RangeInclusive<f32> = -60.0..=6.0;
const PAN: RangeInclusive<f32> = -1.0..=1.0;
/// The largest frame or Item id a TOML integer holds, so every saved one loads again.
const LAST_FRAME: u64 = i64::MAX as u64;
const TAKE_EXTENSION: &str = ".wav";

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub sample_rate: u32,
    pub tempo_bpm: f64,
    /// Beats per bar and the note value of one beat.
    pub meter: (u32, u32),
    pub loop_region: LoopRegion,
    pub latency_offset_ms: f64,
    /// Whether the click sounds on every beat.
    pub click: bool,
    /// Bars of click before Recording starts from stopped.
    pub count_in: u32,
    pub tracks: Vec<Track>,
    /// The id the next new Item gets; above every Item's id, so an id is never reused.
    pub next_item_id: u64,
    pub buses: Vec<Bus>,
    pub master: Master,
}

/// Start and end in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopRegion {
    pub start: u64,
    pub end: u64,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "mute, solo, arm and to_master are independent Track switches"
)]
pub struct Track {
    pub name: String,
    /// The capture port this Track records and monitors; `None` connects nothing.
    pub input: Option<String>,
    pub volume_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub arm: bool,
    /// Whether the Track's own output goes to Master; its Sends play either way.
    pub to_master: bool,
    /// At most one per Bus; a Bus with none here gets nothing from this Track.
    pub sends: Vec<Send>,
    pub modules: Vec<PresetModule>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bus {
    /// Stays the Bus's while others are added or removed; Sends name their Bus by it.
    pub id: u64,
    pub name: String,
    pub volume_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub modules: Vec<PresetModule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    /// The id of the Bus it sends to.
    pub bus: u64,
    /// `-inf` is off.
    pub level_db: f32,
    /// Taken after the Track's Rack and pan, before its volume.
    pub pre: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Master {
    /// `-inf` is silent.
    pub volume_db: f32,
    pub modules: Vec<PresetModule>,
}

impl Default for Master {
    fn default() -> Self {
        Master {
            volume_db: 0.0,
            modules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Unique in the Project; stays the Item's while others are added, moved or removed.
    pub id: u64,
    /// Frames from the start of the Project.
    pub position: u64,
    /// Frames of the active Take skipped before `position`: the Item plays its Take from this frame on.
    pub offset: u64,
    /// Frames played from `offset`; `None` plays to the end of the active Take.
    pub length: Option<u64>,
    /// The index in `takes` of the Take that plays.
    pub take: usize,
    pub takes: Vec<Take>,
}

impl Item {
    /// The frames the Item plays: `length`, or its active Take's frames less `offset`.
    pub fn played_frames(&self) -> u64 {
        self.length
            .unwrap_or_else(|| self.takes[self.take].frames.saturating_sub(self.offset))
    }

    /// The frame after the Item's last.
    pub fn end(&self) -> u64 {
        self.position.saturating_add(self.played_frames())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Take {
    /// Relative to the Project folder, e.g. `takes/guitar-1.wav`.
    pub file: String,
    pub frames: u64,
}

#[derive(Debug)]
pub enum ProjectError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Syntax {
        path: PathBuf,
        line: usize,
        col: usize,
        msg: String,
    },
    Track {
        track: String,
        error: PresetError,
    },
    TooManyTracks,
    Tempo,
    Meter,
    SampleRate,
    LatencyOffset,
    CountIn,
    Loop,
    LoopPastLastFrame,
    Volume(String),
    Pan(String),
    ItemPastLastFrame {
        track: String,
        item: usize,
    },
    OffsetPastLastFrame {
        track: String,
        item: usize,
    },
    TakeOutsideProject(String),
    NoTake {
        track: String,
        item: usize,
        take: usize,
    },
    ZeroLength {
        track: String,
        item: usize,
    },
    LengthPastTake {
        track: String,
        item: usize,
    },
    OffsetPastTake {
        track: String,
        id: u64,
    },
    DuplicateItemId(u64),
    NextItemId,
    NextItemIdPastLast,
    EmptyName,
    Exists(String),
    /// A Track name that `track_add` would refuse, with its message.
    TrackName(String),
    TooManyBuses,
    Bus {
        bus: String,
        error: PresetError,
    },
    Master(PresetError),
    CabinetOnBus,
    /// A Bus name that breaks the Track name rules, with its message.
    BusName(String),
    DuplicateBusId(u64),
    BusVolume(String),
    BusPan(String),
    MasterVolume,
    UnknownBus {
        track: String,
        bus: u64,
    },
    DuplicateSend {
        track: String,
        bus: u64,
    },
    SendLevel(String),
}

/// What [`Project::load`] changed to open a file that an earlier build wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadWarning {
    /// An Item that played nothing, its active Take no longer than its `offset`, was left out; `file` is that Take's, which stays on disk.
    DroppedItem {
        track: String,
        id: u64,
        file: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    name: String,
    sample_rate: u32,
    tempo_bpm: f64,
    meter: (u32, u32),
    latency_offset_ms: f64,
    #[serde(default)]
    click: bool,
    #[serde(default)]
    count_in: u32,
    loop_region: LoopRegion,
    #[serde(default)]
    next_item_id: Option<u64>,
    #[serde(default)]
    master: MasterFile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    buses: Vec<BusFile>,
    #[serde(default)]
    tracks: Vec<TrackFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MasterFile {
    volume_db: f32,
    #[serde(default)]
    module: Vec<ModuleEntry>,
}

impl Default for MasterFile {
    fn default() -> Self {
        MasterFile {
            volume_db: 0.0,
            module: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BusFile {
    id: u64,
    name: String,
    volume_db: f32,
    pan: f32,
    #[serde(default)]
    mute: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    module: Vec<ModuleEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "mute, solo, arm and to_master are independent Track switches"
)]
struct TrackFile {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<String>,
    volume_db: f32,
    pan: f32,
    #[serde(default)]
    mute: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    arm: bool,
    #[serde(default = "on")]
    to_master: bool,
    #[serde(default)]
    module: Vec<ModuleEntry>,
    #[serde(default)]
    items: Vec<ItemFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sends: Vec<Send>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemFile {
    /// Absent in files saved before Items had ids; [`read`] gives them ids in file order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    position: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    length: Option<u64>,
    take: usize,
    takes: Vec<Take>,
}

impl Project {
    /// Creates `<projects_dir>/<slug of name>/` with an empty `takes/` and a `project.toml` for a Project with no Tracks; the name reaches the path only through the slug.
    ///
    /// # Errors
    ///
    /// [`ProjectError::EmptyName`] when the name has no ASCII letter or digit, [`ProjectError::Exists`] when the folder is already there, [`ProjectError::Io`] when a folder or file cannot be written.
    pub fn new(projects_dir: &Path, name: &str) -> Result<(PathBuf, Project), ProjectError> {
        let name: String = name.chars().filter(|c| !c.is_ascii_control()).collect();
        let name = name.trim();
        let slug = slug(name);
        if slug.is_empty() {
            return Err(ProjectError::EmptyName);
        }
        fs::create_dir_all(projects_dir).map_err(io_error(projects_dir))?;
        let dir = projects_dir.join(slug);
        match fs::create_dir(&dir) {
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                return Err(ProjectError::Exists(name.to_string()));
            }
            created => created.map_err(io_error(&dir))?,
        }
        let project = Project {
            name: name.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            tempo_bpm: DEFAULT_TEMPO_BPM,
            meter: DEFAULT_METER,
            loop_region: LoopRegion::default(),
            latency_offset_ms: 0.0,
            click: false,
            count_in: 0,
            tracks: Vec::new(),
            next_item_id: 1,
            buses: Vec::new(),
            master: Master::default(),
        };
        let takes = dir.join(TAKES_DIR);
        let filled = fs::create_dir(&takes)
            .map_err(io_error(&takes))
            .and_then(|()| project.save(&dir));
        if let Err(error) = filled {
            let _ = fs::remove_dir_all(&dir);
            return Err(error);
        }
        Ok((dir, project))
    }

    /// Reads `<dir>/project.toml`, leaving out each Item whose active Take holds no frames after its `offset`, with a [`LoadWarning`] for it; saving refuses such an Item.
    ///
    /// # Errors
    ///
    /// A [`ProjectError`] when the file cannot be read or parsed, a Track's Modules fail the checks a Preset's do, or a value is out of range: more than [`MAX_TRACKS`] Tracks, tempo outside 20..=300 bpm, a meter other than 1..=16 beats of a 2, 4, 8 or 16 note, a sample rate of 0, a latency offset outside ±1000 ms, a loop that is on with its start not before its end, a Track's volume outside −60..=6 dB or pan outside −1..=1, an Item or loop past the last frame, a Take file other than `takes/<name>.wav`, an Item's active Take that does not exist, more than [`MAX_BUSES`] Buses, two Buses with one id or one name, a Bus's volume or pan out of the Track range, a Master volume other than `-inf` or −60..=6 dB, a cabinet on a Bus or Master, or a Send to an unknown Bus, a second Send to one Bus, or a Send level other than `-inf` or −60..=6 dB.
    pub fn load(dir: &Path) -> Result<(Project, Vec<LoadWarning>), ProjectError> {
        read(&dir.join(PROJECT_FILE))
    }

    /// Reads `<dir>/project.autosave.toml` as [`Project::load`] reads `project.toml`.
    ///
    /// # Errors
    ///
    /// As [`Project::load`].
    pub fn load_autosave(dir: &Path) -> Result<(Project, Vec<LoadWarning>), ProjectError> {
        read(&dir.join(AUTOSAVE_FILE))
    }

    /// Replaces `<dir>/project.toml` by a file written beside it and renamed into place.
    ///
    /// # Errors
    ///
    /// The [`ProjectError`] [`Project::load`] would give for a value out of range, or [`ProjectError::Io`] when the file cannot be written.
    pub fn save(&self, dir: &Path) -> Result<(), ProjectError> {
        write(&dir.join(PROJECT_FILE), self)
    }

    /// Replaces `<dir>/project.autosave.toml` as [`Project::save`] does; `project.toml` stays as it was.
    ///
    /// # Errors
    ///
    /// As [`Project::save`].
    pub fn save_autosave(&self, dir: &Path) -> Result<(), ProjectError> {
        write(&dir.join(AUTOSAVE_FILE), self)
    }

    /// The folders under `projects_dir` that hold a `project.toml`, sorted; none when `projects_dir` does not exist.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Io`] when `projects_dir` exists but cannot be read.
    pub fn list(projects_dir: &Path) -> Result<Vec<PathBuf>, ProjectError> {
        let entries = match fs::read_dir(projects_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(io_error(projects_dir)(err)),
        };
        let mut dirs = Vec::new();
        for entry in entries {
            let dir = entry.map_err(io_error(projects_dir))?.path();
            if dir.join(PROJECT_FILE).is_file() {
                dirs.push(dir);
            }
        }
        dirs.sort();
        Ok(dirs)
    }

    /// # Errors
    ///
    /// [`ProjectError::TooManyTracks`] when the Project already has [`MAX_TRACKS`], or the error [`Project::load`] would give for a value of the Track out of range.
    pub fn add_track(&mut self, track: Track) -> Result<(), ProjectError> {
        if self.tracks.len() >= MAX_TRACKS {
            return Err(ProjectError::TooManyTracks);
        }
        check_track(&track)?;
        self.tracks.push(track);
        Ok(())
    }
}

impl Track {
    /// A Track with the Preset's Modules, no input, at 0 dB, centred, with no Items.
    pub fn from_preset(name: &str, preset: &Preset) -> Track {
        Track {
            name: name.to_string(),
            input: None,
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            arm: false,
            to_master: true,
            sends: Vec::new(),
            modules: preset.modules.clone(),
            items: Vec::new(),
        }
    }
}

/// Whether an autosave is due: the Project changed since `last_save` and [`AUTOSAVE_INTERVAL`] has passed.
pub fn autosave_due(last_save: Instant, now: Instant, dirty: bool) -> bool {
    dirty && now.saturating_duration_since(last_save) >= AUTOSAVE_INTERVAL
}

/// Whether `<dir>/project.autosave.toml` exists and was modified after `project.toml`, or `project.toml` is gone.
pub fn autosave_newer(dir: &Path) -> bool {
    let modified = |file| fs::metadata(dir.join(file)).and_then(|meta| meta.modified());
    match (modified(AUTOSAVE_FILE), modified(PROJECT_FILE)) {
        (Ok(autosave), Ok(project)) => autosave > project,
        (Ok(_), Err(_)) => true,
        (Err(_), _) => false,
    }
}

/// The name without control characters or surrounding blanks, refused when empty, only digits, or when another of `tracks` (but the one at `except`) has it in any letter case.
pub(crate) fn checked_name(
    tracks: &[Track],
    name: &str,
    except: Option<usize>,
) -> Result<String, String> {
    let others = tracks
        .iter()
        .enumerate()
        .filter(|(idx, _)| Some(*idx) != except);
    checked_strip_name("track", others.map(|(_, track)| track.name.as_str()), name)
}

/// [`checked_name`] for a Bus among `buses`.
pub(crate) fn checked_bus_name(
    buses: &[Bus],
    name: &str,
    except: Option<usize>,
) -> Result<String, String> {
    let others = buses
        .iter()
        .enumerate()
        .filter(|(idx, _)| Some(*idx) != except);
    checked_strip_name("bus", others.map(|(_, bus)| bus.name.as_str()), name)
}

fn checked_strip_name<'a>(
    kind: &str,
    mut others: impl Iterator<Item = &'a str>,
    name: &str,
) -> Result<String, String> {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("{kind} name must not be empty"));
    }
    if name.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{kind} name must not be only digits"));
    }
    if others.any(|other| other.to_lowercase() == name.to_lowercase()) {
        return Err(format!("a {kind} named '{name}' already exists"));
    }
    Ok(name.to_string())
}

fn read(path: &Path) -> Result<(Project, Vec<LoadWarning>), ProjectError> {
    let text = fs::read_to_string(path).map_err(io_error(path))?;
    let file: ProjectFile = toml::from_str(&text).map_err(|error| {
        let (line, col) = crate::toml_position(&text, &error);
        ProjectError::Syntax {
            path: path.to_path_buf(),
            line,
            col,
            msg: error.message().to_string(),
        }
    })?;
    if file.tracks.len() > MAX_TRACKS {
        return Err(ProjectError::TooManyTracks);
    }
    let item_files = file.tracks.iter().flat_map(|track| &track.items);
    let mut next_item_id = file.next_item_id.unwrap_or_else(|| {
        item_files
            .filter_map(|item| item.id)
            .max()
            .map_or(1, |id| id.saturating_add(1))
    });
    let mut tracks: Vec<Track> = file
        .tracks
        .into_iter()
        .map(|track| track_from_file(track, &mut next_item_id))
        .collect::<Result<_, _>>()?;
    for idx in 0..tracks.len() {
        tracks[idx].name = checked_name(&tracks[..idx], &tracks[idx].name, None)
            .map_err(ProjectError::TrackName)?;
    }
    let warnings = tracks
        .iter_mut()
        .flat_map(drop_items_past_their_take)
        .collect();
    if file.buses.len() > MAX_BUSES {
        return Err(ProjectError::TooManyBuses);
    }
    let mut buses: Vec<Bus> = file
        .buses
        .into_iter()
        .map(bus_from_file)
        .collect::<Result<_, _>>()?;
    for idx in 0..buses.len() {
        buses[idx].name = checked_bus_name(&buses[..idx], &buses[idx].name, None)
            .map_err(ProjectError::BusName)?;
    }
    let master = master_from_file(file.master)?;
    let project = Project {
        name: file.name,
        sample_rate: file.sample_rate,
        tempo_bpm: file.tempo_bpm,
        meter: file.meter,
        loop_region: file.loop_region,
        latency_offset_ms: file.latency_offset_ms,
        click: file.click,
        count_in: file.count_in,
        tracks,
        next_item_id,
        buses,
        master,
    };
    check_project(&project)?;
    Ok((project, warnings))
}

/// Leaves out the Track's Items that [`starts_past_its_take`], which an earlier build could record and save.
fn drop_items_past_their_take(track: &mut Track) -> Vec<LoadWarning> {
    let (dropped, kept) = track
        .items
        .drain(..)
        .partition(|item| item.take < item.takes.len() && starts_past_its_take(item));
    track.items = kept;
    dropped
        .into_iter()
        .map(|item: Item| LoadWarning::DroppedItem {
            track: track.name.clone(),
            id: item.id,
            file: item.takes[item.take].file.clone(),
        })
        .collect()
}

/// Whether the Item's active Take holds no frames after its `offset`; the active Take exists.
fn starts_past_its_take(item: &Item) -> bool {
    item.offset >= item.takes[item.take].frames
}

/// Whether `(beats, note)` is 1 to 16 beats of a 2, 4, 8 or 16 note.
pub(crate) fn is_meter((beats, note): (u32, u32)) -> bool {
    METER_BEATS.contains(&beats) && METER_NOTES.contains(&note)
}

pub(crate) fn check_project(project: &Project) -> Result<(), ProjectError> {
    if project.tracks.len() > MAX_TRACKS {
        return Err(ProjectError::TooManyTracks);
    }
    if !TEMPO_BPM.contains(&project.tempo_bpm) {
        return Err(ProjectError::Tempo);
    }
    if !is_meter(project.meter) {
        return Err(ProjectError::Meter);
    }
    if project.sample_rate == 0 {
        return Err(ProjectError::SampleRate);
    }
    if !LATENCY_OFFSET_MS.contains(&project.latency_offset_ms) {
        return Err(ProjectError::LatencyOffset);
    }
    if !COUNT_IN_BARS.contains(&project.count_in) {
        return Err(ProjectError::CountIn);
    }
    let LoopRegion { start, end, on } = project.loop_region;
    if start.max(end) > LAST_FRAME {
        return Err(ProjectError::LoopPastLastFrame);
    }
    if on && start >= end {
        return Err(ProjectError::Loop);
    }
    project.tracks.iter().try_for_each(check_track)?;
    check_item_ids(project)?;
    check_mixer(project)
}

fn check_item_ids(project: &Project) -> Result<(), ProjectError> {
    if project.next_item_id >= LAST_FRAME {
        return Err(ProjectError::NextItemIdPastLast);
    }
    let mut ids = HashSet::new();
    for item in project.tracks.iter().flat_map(|track| &track.items) {
        if !ids.insert(item.id) {
            return Err(ProjectError::DuplicateItemId(item.id));
        }
        if item.id >= project.next_item_id {
            return Err(ProjectError::NextItemId);
        }
    }
    Ok(())
}

fn check_mixer(project: &Project) -> Result<(), ProjectError> {
    if project.buses.len() > MAX_BUSES {
        return Err(ProjectError::TooManyBuses);
    }
    let mut ids = HashSet::new();
    for bus in &project.buses {
        if !ids.insert(bus.id) {
            return Err(ProjectError::DuplicateBusId(bus.id));
        }
        if !VOLUME_DB.contains(&bus.volume_db) {
            return Err(ProjectError::BusVolume(bus.name.clone()));
        }
        if !PAN.contains(&bus.pan) {
            return Err(ProjectError::BusPan(bus.name.clone()));
        }
    }
    let racks = project.buses.iter().map(|bus| &bus.modules);
    let mut modules = racks.chain([&project.master.modules]).flatten();
    if modules.any(|module| module.type_name == CABINET_TYPE) {
        return Err(ProjectError::CabinetOnBus);
    }
    if !is_level_db(project.master.volume_db) {
        return Err(ProjectError::MasterVolume);
    }
    for track in &project.tracks {
        let mut sent_to = HashSet::new();
        for send in &track.sends {
            if !ids.contains(&send.bus) {
                return Err(ProjectError::UnknownBus {
                    track: track.name.clone(),
                    bus: send.bus,
                });
            }
            if !sent_to.insert(send.bus) {
                return Err(ProjectError::DuplicateSend {
                    track: track.name.clone(),
                    bus: send.bus,
                });
            }
            if !is_level_db(send.level_db) {
                return Err(ProjectError::SendLevel(track.name.clone()));
            }
        }
    }
    Ok(())
}

/// Whether `db` is a Send level or Master volume: off (`-inf`) or within the Track volume range.
fn is_level_db(db: f32) -> bool {
    db == f32::NEG_INFINITY || VOLUME_DB.contains(&db)
}

fn is_off(send: &Send) -> bool {
    send.level_db == f32::NEG_INFINITY
}

fn on() -> bool {
    true
}

pub(crate) fn check_track(track: &Track) -> Result<(), ProjectError> {
    if !VOLUME_DB.contains(&track.volume_db) {
        return Err(ProjectError::Volume(track.name.clone()));
    }
    if !PAN.contains(&track.pan) {
        return Err(ProjectError::Pan(track.name.clone()));
    }
    for (index, item) in track.items.iter().enumerate() {
        if let Some(take) = item.takes.iter().find(|take| !is_take_file(&take.file)) {
            return Err(ProjectError::TakeOutsideProject(take.file.clone()));
        }
        if item.take >= item.takes.len() {
            return Err(ProjectError::NoTake {
                track: track.name.clone(),
                item: index,
                take: item.take,
            });
        }
        if item.offset > LAST_FRAME {
            return Err(ProjectError::OffsetPastLastFrame {
                track: track.name.clone(),
                item: index,
            });
        }
        if starts_past_its_take(item) {
            return Err(ProjectError::OffsetPastTake {
                track: track.name.clone(),
                id: item.id,
            });
        }
        if item.length == Some(0) {
            return Err(ProjectError::ZeroLength {
                track: track.name.clone(),
                item: index,
            });
        }
        if let Some(length) = item.length
            && item.offset.saturating_add(length) > item.takes[item.take].frames
        {
            return Err(ProjectError::LengthPastTake {
                track: track.name.clone(),
                item: index,
            });
        }
        let longest = item.takes.iter().map(|take| take.frames).max().unwrap_or(0);
        if item.position.saturating_add(longest) > LAST_FRAME {
            return Err(ProjectError::ItemPastLastFrame {
                track: track.name.clone(),
                item: index,
            });
        }
    }
    Ok(())
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_zero(frames: &u64) -> bool {
    *frames == 0
}

/// `name` with each control character and each Unicode bidi override or isolate escaped as `escape_debug` writes it, so a file name shown to people reads as the name it is.
pub fn shown_file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if is_hidden_char(c) {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// A control character, or a bidi override or isolate (U+202A–U+202E, U+2066–U+2069), which can make shown text read as other text.
pub(crate) fn is_hidden_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Whether `file` is `takes/<name>.wav`: one file directly in the Project's `takes/` folder.
pub(crate) fn is_take_file(file: &str) -> bool {
    file.strip_prefix(TAKES_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|name| name.strip_suffix(TAKE_EXTENSION))
        .is_some_and(|stem| !stem.is_empty() && !stem.contains('/'))
}

fn track_from_file(file: TrackFile, next_item_id: &mut u64) -> Result<Track, ProjectError> {
    let modules = parse_modules(file.module).map_err(|error| ProjectError::Track {
        track: file.name.clone(),
        error,
    })?;
    Ok(Track {
        name: file.name,
        input: file.input,
        volume_db: file.volume_db,
        pan: file.pan,
        mute: file.mute,
        solo: file.solo,
        arm: file.arm,
        to_master: file.to_master,
        sends: file.sends,
        modules,
        items: file
            .items
            .into_iter()
            .map(|item| item_from_file(item, next_item_id))
            .collect(),
    })
}

fn item_from_file(file: ItemFile, next_item_id: &mut u64) -> Item {
    let id = file.id.unwrap_or_else(|| {
        let id = *next_item_id;
        *next_item_id = next_item_id.saturating_add(1);
        id
    });
    Item {
        id,
        position: file.position,
        offset: file.offset,
        length: file.length,
        take: file.take,
        takes: file.takes,
    }
}

fn item_file(item: &Item) -> ItemFile {
    ItemFile {
        id: Some(item.id),
        position: item.position,
        offset: item.offset,
        length: item.length,
        take: item.take,
        takes: item.takes.clone(),
    }
}

fn bus_from_file(file: BusFile) -> Result<Bus, ProjectError> {
    if file.module.iter().any(ModuleEntry::is_cabinet) {
        return Err(ProjectError::CabinetOnBus);
    }
    let modules = parse_stereo_modules(file.module).map_err(|error| ProjectError::Bus {
        bus: file.name.clone(),
        error,
    })?;
    Ok(Bus {
        id: file.id,
        name: file.name,
        volume_db: file.volume_db,
        pan: file.pan,
        mute: file.mute,
        solo: file.solo,
        modules,
    })
}

fn master_from_file(file: MasterFile) -> Result<Master, ProjectError> {
    if file.module.iter().any(ModuleEntry::is_cabinet) {
        return Err(ProjectError::CabinetOnBus);
    }
    Ok(Master {
        volume_db: file.volume_db,
        modules: parse_stereo_modules(file.module).map_err(ProjectError::Master)?,
    })
}

fn write(path: &Path, project: &Project) -> Result<(), ProjectError> {
    check_project(project)?;
    let file = ProjectFile {
        name: project.name.clone(),
        sample_rate: project.sample_rate,
        tempo_bpm: project.tempo_bpm,
        meter: project.meter,
        latency_offset_ms: project.latency_offset_ms,
        click: project.click,
        count_in: project.count_in,
        loop_region: project.loop_region,
        next_item_id: Some(project.next_item_id),
        master: MasterFile {
            volume_db: project.master.volume_db,
            module: module_entries(&project.master.modules),
        },
        buses: project
            .buses
            .iter()
            .map(|bus| BusFile {
                id: bus.id,
                name: bus.name.clone(),
                volume_db: bus.volume_db,
                pan: bus.pan,
                mute: bus.mute,
                solo: bus.solo,
                module: module_entries(&bus.modules),
            })
            .collect(),
        tracks: project
            .tracks
            .iter()
            .map(|track| TrackFile {
                name: track.name.clone(),
                input: track.input.clone(),
                volume_db: track.volume_db,
                pan: track.pan,
                mute: track.mute,
                solo: track.solo,
                arm: track.arm,
                to_master: track.to_master,
                module: module_entries(&track.modules),
                items: track.items.iter().map(item_file).collect(),
                sends: track
                    .sends
                    .iter()
                    .filter(|send| !is_off(send))
                    .copied()
                    .collect(),
            })
            .collect(),
    };
    let text = toml::to_string_pretty(&file)
        .expect("a checked Project serialises: every frame fits a TOML integer");
    replace_file(path, text.as_bytes()).map_err(io_error(path))
}

fn replace_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = temp_path(path);
    let mut file = File::create_new(&tmp)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

fn io_error(path: &Path) -> impl Fn(io::Error) -> ProjectError + '_ {
    move |source| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ProjectError::Syntax {
                path,
                line,
                col,
                msg,
            } => write!(f, "{}: line {line}:{col}: {msg}", path.display()),
            ProjectError::Track { track, error } => write!(f, "track '{track}': {error}"),
            ProjectError::TooManyTracks => {
                write!(f, "a project has at most {MAX_TRACKS} tracks")
            }
            ProjectError::Tempo => f.write_str("tempo must be between 20 and 300 bpm"),
            ProjectError::Meter => {
                f.write_str("meter must be 1 to 16 beats of a 2, 4, 8 or 16 note")
            }
            ProjectError::SampleRate => f.write_str("sample rate must be above 0"),
            ProjectError::LatencyOffset => {
                f.write_str("latency offset must be between -1000 and 1000 ms")
            }
            ProjectError::CountIn => f.write_str("count-in must be 0, 1 or 2 bars"),
            ProjectError::Loop => f.write_str("loop start must be before loop end"),
            ProjectError::LoopPastLastFrame => f.write_str("loop region ends past the last frame"),
            ProjectError::Volume(track) => {
                write!(f, "track '{track}' volume must be between -60 and 6 dB")
            }
            ProjectError::Pan(track) => write!(f, "track '{track}' pan must be between -1 and 1"),
            ProjectError::ItemPastLastFrame { track, item } => {
                write!(f, "track '{track}' item {item} ends past the last frame")
            }
            ProjectError::OffsetPastLastFrame { track, item } => {
                write!(
                    f,
                    "track '{track}' item {item} offset is past the last frame"
                )
            }
            ProjectError::TakeOutsideProject(file) => {
                write!(f, "take file '{file}' must be takes/<name>.wav")
            }
            ProjectError::NoTake { track, item, take } => {
                write!(f, "track '{track}' item {item} has no take {take}")
            }
            ProjectError::ZeroLength { track, item } => {
                write!(f, "track '{track}' item {item} length must be at least 1")
            }
            ProjectError::LengthPastTake { track, item } => {
                write!(
                    f,
                    "track '{track}' item {item} plays past the end of its take"
                )
            }
            ProjectError::OffsetPastTake { track, id } => {
                write!(
                    f,
                    "track '{track}' item {id} starts past the end of its take"
                )
            }
            ProjectError::DuplicateItemId(id) => write!(f, "two items have id {id}"),
            ProjectError::NextItemId => f.write_str("next_item_id must be above every item id"),
            ProjectError::NextItemIdPastLast => {
                write!(f, "next_item_id must be below {LAST_FRAME}")
            }
            ProjectError::EmptyName => f.write_str("project name must not be empty"),
            ProjectError::Exists(name) => write!(f, "a project named '{name}' already exists"),
            ProjectError::TrackName(error) | ProjectError::BusName(error) => f.write_str(error),
            ProjectError::TooManyBuses => write!(f, "at most {MAX_BUSES} buses"),
            ProjectError::Bus { bus, error } => write!(f, "bus '{bus}': {error}"),
            ProjectError::Master(error) => write!(f, "master: {error}"),
            ProjectError::CabinetOnBus => {
                f.write_str("a cabinet is not allowed on a Bus or Master")
            }
            ProjectError::DuplicateBusId(id) => write!(f, "two buses have id {id}"),
            ProjectError::BusVolume(bus) => {
                write!(f, "bus '{bus}' volume must be between -60 and 6 dB")
            }
            ProjectError::BusPan(bus) => write!(f, "bus '{bus}' pan must be between -1 and 1"),
            ProjectError::MasterVolume => {
                f.write_str("master volume must be -inf or between -60 and 6 dB")
            }
            ProjectError::UnknownBus { track, bus } => {
                write!(f, "track '{track}' sends to unknown bus {bus}")
            }
            ProjectError::DuplicateSend { track, bus } => {
                write!(f, "track '{track}' has two sends to bus {bus}")
            }
            ProjectError::SendLevel(track) => write!(
                f,
                "track '{track}' send level must be -inf or between -60 and 6 dB"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

impl fmt::Display for LoadWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadWarning::DroppedItem { track, id, file } => write!(
                f,
                "track '{track}': dropped item {id} ({}): starts past the end of its take",
                shown_file_name(file)
            ),
        }
    }
}
