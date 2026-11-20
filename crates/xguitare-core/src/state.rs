use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_file::temp_path;
use crate::metronome::{BAD_BPM, BAD_VOLUME, VOLUME_DB};
use crate::project::{ProjectError, TEMPO_BPM, is_meter};
use crate::{BankKind, Ports};

const STATE_FILE_MODE: u32 = 0o600;

/// What the Daemon's `state.toml` holds: the choices made through the Control API (`set_ports`, `set_settings`), which override `config.toml`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bank_kind: Option<BankKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ir_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Banks::is_empty")]
    pub banks: Banks,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Ports>,
    /// The folder that holds one sub-folder per Project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "SavedMetronome::is_empty")]
    pub metronome: SavedMetronome,
}

/// The `[metronome]` table: the tempo, meter and volume the Metronome plays with no Project open; it always starts off.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedMetronome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meter: Option<(u32, u32)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_db: Option<f64>,
}

impl SavedMetronome {
    fn is_empty(&self) -> bool {
        *self == SavedMetronome::default()
    }

    /// Drops each value out of range; the reasons, or `None` when every value is valid.
    fn drop_invalid(&mut self) -> Option<String> {
        let mut reasons = Vec::new();
        if self.bpm.take_if(|bpm| !TEMPO_BPM.contains(bpm)).is_some() {
            reasons.push(BAD_BPM.to_string());
        }
        if self.meter.take_if(|meter| !is_meter(*meter)).is_some() {
            reasons.push(ProjectError::Meter.to_string());
        }
        if self
            .volume_db
            .take_if(|db| !VOLUME_DB.contains(db))
            .is_some()
        {
            reasons.push(BAD_VOLUME.to_string());
        }
        (!reasons.is_empty()).then(|| reasons.join(", "))
    }
}

/// The `[banks]` table: the Bank directory each kind switched to or from, recorded by the first `bank_kind` switch.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Banks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guitar: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bass: Option<PathBuf>,
}

impl Banks {
    pub fn get(&self, kind: BankKind) -> Option<&PathBuf> {
        match kind {
            BankKind::Guitar => self.guitar.as_ref(),
            BankKind::Bass => self.bass.as_ref(),
        }
    }

    pub(crate) fn insert(&mut self, kind: BankKind, dir: PathBuf) {
        match kind {
            BankKind::Guitar => self.guitar = Some(dir),
            BankKind::Bass => self.bass = Some(dir),
        }
    }

    fn is_empty(&self) -> bool {
        self.guitar.is_none() && self.bass.is_none()
    }
}

/// Reads `path`; a missing file is the empty state, and a file that cannot be read or parsed is the empty state plus a warning naming the path and the error.
pub(crate) fn read_state(path: &Path) -> (SavedState, Option<String>) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return (SavedState::default(), None),
        Err(err) => {
            return (
                SavedState::default(),
                Some(format!("{}: {err}", path.display())),
            );
        }
    };
    match toml::from_str::<SavedState>(&text) {
        Ok(mut state) => {
            let warning = state
                .metronome
                .drop_invalid()
                .map(|reasons| format!("{}: metronome: {reasons}", path.display()));
            (state, warning)
        }
        Err(err) => {
            let (line, col) = crate::toml_position(&text, &err);
            let warning = format!("{}: line {line}:{col}: {}", path.display(), err.message());
            (SavedState::default(), Some(warning))
        }
    }
}

/// Replaces `path` by a new file of mode 0600 holding `state`, written beside it and renamed into place.
pub(crate) fn write_state(path: &Path, state: &SavedState) -> io::Result<()> {
    let text = toml::to_string(state).map_err(io::Error::other)?;
    write_private(path, text.as_bytes())
}

/// The bytes of `path`, or `None` when there is no such file, for `restore_state`.
pub(crate) fn read_state_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Puts back what `read_state_bytes` returned: the old bytes, or no file.
pub(crate) fn restore_state(path: &Path, bytes: Option<&[u8]>) -> io::Result<()> {
    match bytes {
        Some(bytes) => write_private(path, bytes),
        None => fs::remove_file(path),
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = temp_path(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(STATE_FILE_MODE)
        .open(&tmp)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}
