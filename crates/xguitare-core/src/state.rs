use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::control::temp_path;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Ports>,
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
    match toml::from_str(&text) {
        Ok(state) => (state, None),
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
    let tmp = temp_path(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(STATE_FILE_MODE)
        .open(&tmp)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}
