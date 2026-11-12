use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::control::EngineState;
use crate::project::{TAKES_DIR, is_take_file};
use crate::project_control::{forget_takes, is_recording, kept_takes};

const NOT_A_DIRECTORY: &str = "takes is not a directory";
const CLEANING_WHILE_RECORDING: &str = "cannot clean takes while recording";

/// The open Project's Take files that nothing keeps, with their sizes and the total.
pub(crate) fn takes_unused(state: &EngineState) -> Result<Value, String> {
    let unused = unused_takes(state)?;
    let bytes: u64 = unused.iter().map(|(_, bytes)| bytes).sum();
    let files: Vec<Value> = unused
        .iter()
        .map(|(file, bytes)| json!({"file": file, "bytes": bytes}))
        .collect();
    Ok(json!({"files": files, "bytes": bytes}))
}

/// Deletes the Take `files`, each of which must be one [`takes_unused`] lists now, after saving the Project; one refused name deletes nothing. Refused while recording.
pub(crate) fn takes_clean(state: &mut EngineState, files: &[String]) -> Result<Value, String> {
    if is_recording(state)? {
        return Err(CLEANING_WHILE_RECORDING.to_string());
    }
    let unused = unused_takes(state)?;
    for file in files {
        if !is_cleanable(file) {
            return Err(format!("invalid take file '{}'", file.escape_debug()));
        }
        if unused.binary_search_by(|(name, _)| name.cmp(file)).is_err() {
            return Err(format!("not an unused take: {file}"));
        }
    }
    let mut files = files.to_vec();
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Ok(json!({"deleted": 0, "bytes": 0}));
    }
    let (dir, _) = kept_takes(state)?;
    let dir = dir.to_path_buf();
    forget_takes(state, &files)?;
    let takes = takes_dir(&dir)?;
    let mut bytes = 0;
    for file in &files {
        bytes += delete_take(&takes, file)?;
    }
    Ok(json!({"deleted": files.len(), "bytes": bytes}))
}

/// Removes the regular file `file` (`takes/<name>.wav`) from the folder `takes` and returns its size; a symlink or anything else there is refused, never followed.
fn delete_take(takes: &Path, file: &str) -> Result<u64, String> {
    let name = file
        .strip_prefix(TAKES_DIR)
        .unwrap_or(file)
        .trim_start_matches('/');
    let path = takes.join(name);
    let metadata =
        fs::symlink_metadata(&path).map_err(|err| format!("cannot read {file}: {err}"))?;
    if !metadata.is_file() {
        return Err(format!("not an unused take: {file}"));
    }
    fs::remove_file(&path).map_err(|err| format!("cannot delete {file}: {err}"))?;
    Ok(metadata.len())
}

/// Each regular file `takes/<name>.wav` of the open Project that [`kept_takes`] does not name, with its size, sorted by name; a symlink is never followed.
fn unused_takes(state: &EngineState) -> Result<Vec<(String, u64)>, String> {
    let (dir, kept) = kept_takes(state)?;
    let takes = takes_dir(dir)?;
    let entries =
        fs::read_dir(&takes).map_err(|err| format!("cannot read {}: {err}", takes.display()))?;
    let mut unused = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("cannot read {}: {err}", takes.display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let file = format!("{TAKES_DIR}/{name}");
        if !is_cleanable(&file) || kept.contains(&file) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|err| format!("cannot read {}: {err}", entry.path().display()))?;
        if metadata.is_file() {
            unused.push((file, metadata.len()));
        }
    }
    unused.sort();
    Ok(unused)
}

/// Whether `file` is a Take file name clean-up may touch: `takes/<name>.wav` with no `..` and no control character.
fn is_cleanable(file: &str) -> bool {
    is_take_file(file) && !file.contains("..") && !file.contains(char::is_control)
}

/// `<dir>/takes`, refused when it is a symlink or anything but a directory.
fn takes_dir(dir: &Path) -> Result<PathBuf, String> {
    let takes = dir.join(TAKES_DIR);
    match fs::symlink_metadata(&takes) {
        Ok(metadata) if metadata.is_dir() => Ok(takes),
        Ok(_) => Err(NOT_A_DIRECTORY.to_string()),
        Err(err) => Err(format!("cannot read {}: {err}", takes.display())),
    }
}
