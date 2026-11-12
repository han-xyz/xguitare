use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::control::EngineState;
use crate::project::{TAKES_DIR, is_hidden_char, is_take_file, shown_file_name};
use crate::project_control::{forget_takes, is_recording, kept_takes};

const NOT_A_DIRECTORY: &str = "takes is not a directory";
const CLEANING_WHILE_RECORDING: &str = "cannot clean takes while recording";
const NOT_A_TAKE_FILE: &str = "not a take file";
const NOT_A_REGULAR_FILE: &str = "not a regular file";

/// The open Project's Take files that nothing keeps, with their sizes and the total.
pub(crate) fn takes_unused(state: &EngineState) -> Result<Value, String> {
    let (_, unused) = unused_takes(state)?;
    let bytes: u64 = unused.iter().map(|(_, bytes)| bytes).sum();
    let files: Vec<Value> = unused
        .iter()
        .map(|(file, bytes)| json!({"file": file, "bytes": bytes}))
        .collect();
    Ok(json!({"files": files, "bytes": bytes}))
}

/// Deletes the Take `files`, each of which must be one [`takes_unused`] lists now, after saving the Project; one refused name deletes nothing. Refused while recording. A file that cannot be deleted once the Project is saved is reported in `failed`, not as an error.
pub(crate) fn takes_clean(state: &mut EngineState, files: &[String]) -> Result<Value, String> {
    if is_recording(state)? {
        return Err(CLEANING_WHILE_RECORDING.to_string());
    }
    let (takes, unused) = unused_takes(state)?;
    for file in files {
        if !is_cleanable(file) {
            return Err(format!("invalid take file '{}'", shown_file_name(file)));
        }
        if unused.binary_search_by(|(name, _)| name.cmp(file)).is_err() {
            return Err(format!("not an unused take: {file}"));
        }
    }
    let mut files = files.to_vec();
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Ok(json!({"deleted": 0, "bytes": 0, "failed": []}));
    }
    forget_takes(state, &files)?;
    Ok(delete_take_files(&takes, &files))
}

/// Removes each `takes/<name>.wav` of `files` from the folder `takes` when it is a regular file there, never following a symlink; replies `{deleted, bytes, failed: [{file, error}]}`, every file tried.
#[must_use]
pub fn delete_take_files(takes: &Path, files: &[String]) -> Value {
    let mut deleted = 0;
    let mut bytes = 0;
    let mut failed = Vec::new();
    for file in files {
        match delete_take(takes, file) {
            Ok(size) => {
                deleted += 1;
                bytes += size;
            }
            Err(error) => failed.push(json!({"file": file, "error": error})),
        }
    }
    json!({"deleted": deleted, "bytes": bytes, "failed": failed})
}

fn delete_take(takes: &Path, file: &str) -> Result<u64, String> {
    let name = file
        .strip_prefix(TAKES_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or(NOT_A_TAKE_FILE)?;
    let path = takes.join(name);
    let metadata = fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
    if !metadata.is_file() {
        return Err(NOT_A_REGULAR_FILE.to_string());
    }
    fs::remove_file(&path).map_err(|err| err.to_string())?;
    Ok(metadata.len())
}

/// The open Project's `takes` folder and each regular file `takes/<name>.wav` in it that [`kept_takes`] does not name, with its size, sorted by name; a symlink is never followed.
fn unused_takes(state: &EngineState) -> Result<(PathBuf, Vec<(String, u64)>), String> {
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
    Ok((takes, unused))
}

/// Whether `file` is a Take file name clean-up may touch: `takes/<name>.wav` with no `..`, no control character and no bidi override or isolate.
fn is_cleanable(file: &str) -> bool {
    is_take_file(file) && !file.contains("..") && !file.contains(is_hidden_char)
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
