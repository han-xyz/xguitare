use std::fmt;
use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::control::{slug, temp_path};
use crate::preset::{ModuleEntry, is_relative_name, module_entries, parse_modules};
use crate::{Preset, PresetError, PresetModule};

pub const PROJECT_FILE: &str = "project.toml";
pub const AUTOSAVE_FILE: &str = "project.autosave.toml";
pub const TAKES_DIR: &str = "takes";
pub const MAX_TRACKS: usize = 16;
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(60);

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_TEMPO_BPM: f64 = 120.0;
const DEFAULT_METER: (u32, u32) = (4, 4);

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub sample_rate: u32,
    pub tempo_bpm: f64,
    /// Beats per bar and the note value of one beat.
    pub meter: (u32, u32),
    pub loop_region: LoopRegion,
    pub latency_offset_ms: f64,
    pub tracks: Vec<Track>,
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
    reason = "mute, solo and arm are independent Track switches"
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
    pub modules: Vec<PresetModule>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    /// Frames from the start of the Project.
    pub position: u64,
    /// The index in `takes` of the Take that plays.
    pub take: usize,
    pub takes: Vec<Take>,
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
    TakeOutsideProject(String),
    NoTake {
        track: String,
        item: usize,
        take: usize,
    },
    EmptyName,
    Exists(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    name: String,
    sample_rate: u32,
    tempo_bpm: f64,
    meter: (u32, u32),
    latency_offset_ms: f64,
    loop_region: LoopRegion,
    #[serde(default)]
    tracks: Vec<TrackFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    module: Vec<ModuleEntry>,
    #[serde(default)]
    items: Vec<Item>,
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
        let takes = dir.join(TAKES_DIR);
        fs::create_dir(&takes).map_err(io_error(&takes))?;
        let project = Project {
            name: name.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            tempo_bpm: DEFAULT_TEMPO_BPM,
            meter: DEFAULT_METER,
            loop_region: LoopRegion::default(),
            latency_offset_ms: 0.0,
            tracks: Vec::new(),
        };
        project.save(&dir)?;
        Ok((dir, project))
    }

    /// Reads `<dir>/project.toml`.
    ///
    /// # Errors
    ///
    /// A [`ProjectError`] when the file cannot be read or parsed, a Track's Modules fail the checks a Preset's do, there are more than [`MAX_TRACKS`] Tracks, a Take file is not a relative path inside the folder, or an Item's active Take does not exist.
    pub fn load(dir: &Path) -> Result<Project, ProjectError> {
        read(&dir.join(PROJECT_FILE))
    }

    /// Reads `<dir>/project.autosave.toml`.
    ///
    /// # Errors
    ///
    /// As [`Project::load`].
    pub fn load_autosave(dir: &Path) -> Result<Project, ProjectError> {
        read(&dir.join(AUTOSAVE_FILE))
    }

    /// Replaces `<dir>/project.toml` by a file written beside it and renamed into place.
    ///
    /// # Errors
    ///
    /// [`ProjectError::TooManyTracks`], or [`ProjectError::Io`] when the file cannot be written.
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
    /// [`ProjectError::TooManyTracks`] when the Project already has [`MAX_TRACKS`].
    pub fn add_track(&mut self, track: Track) -> Result<(), ProjectError> {
        if self.tracks.len() >= MAX_TRACKS {
            return Err(ProjectError::TooManyTracks);
        }
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

fn read(path: &Path) -> Result<Project, ProjectError> {
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
    let tracks = file
        .tracks
        .into_iter()
        .map(track_from_file)
        .collect::<Result<_, _>>()?;
    Ok(Project {
        name: file.name,
        sample_rate: file.sample_rate,
        tempo_bpm: file.tempo_bpm,
        meter: file.meter,
        loop_region: file.loop_region,
        latency_offset_ms: file.latency_offset_ms,
        tracks,
    })
}

fn track_from_file(file: TrackFile) -> Result<Track, ProjectError> {
    let modules = parse_modules(file.module).map_err(|error| ProjectError::Track {
        track: file.name.clone(),
        error,
    })?;
    for (index, item) in file.items.iter().enumerate() {
        if let Some(take) = item.takes.iter().find(|take| !is_relative_name(&take.file)) {
            return Err(ProjectError::TakeOutsideProject(take.file.clone()));
        }
        if item.take >= item.takes.len() {
            return Err(ProjectError::NoTake {
                track: file.name,
                item: index,
                take: item.take,
            });
        }
    }
    Ok(Track {
        name: file.name,
        input: file.input,
        volume_db: file.volume_db,
        pan: file.pan,
        mute: file.mute,
        solo: file.solo,
        arm: file.arm,
        modules,
        items: file.items,
    })
}

fn write(path: &Path, project: &Project) -> Result<(), ProjectError> {
    if project.tracks.len() > MAX_TRACKS {
        return Err(ProjectError::TooManyTracks);
    }
    let file = ProjectFile {
        name: project.name.clone(),
        sample_rate: project.sample_rate,
        tempo_bpm: project.tempo_bpm,
        meter: project.meter,
        latency_offset_ms: project.latency_offset_ms,
        loop_region: project.loop_region,
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
                module: module_entries(&track.modules),
                items: track.items.clone(),
            })
            .collect(),
    };
    let text = toml::to_string_pretty(&file).map_err(|err| ProjectError::Io {
        path: path.to_path_buf(),
        source: io::Error::other(err),
    })?;
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
            ProjectError::TakeOutsideProject(file) => write!(
                f,
                "take file '{file}' must be a relative path inside the project folder"
            ),
            ProjectError::NoTake { track, item, take } => {
                write!(f, "track '{track}' item {item} has no take {take}")
            }
            ProjectError::EmptyName => f.write_str("project name must not be empty"),
            ProjectError::Exists(name) => write!(f, "a project named '{name}' already exists"),
        }
    }
}

impl std::error::Error for ProjectError {}
