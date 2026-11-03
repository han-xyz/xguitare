use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::control::EngineState;
use crate::engine::Command;
use crate::{Bank, BankEntry};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Reindex,
    Reload { program: usize },
    LogAndKeep { file: PathBuf, error: String },
    EngineBusy { file: PathBuf },
    Warn(String),
    Nothing,
}

pub type FileStamp = (SystemTime, u64);

/// Returns the event paths whose stamp differs between the two snapshots; a path missing from a snapshot does not exist at that time.
pub fn changed_paths(
    previous: &HashMap<PathBuf, FileStamp>,
    current: &HashMap<PathBuf, FileStamp>,
    events: &[PathBuf],
) -> Vec<PathBuf> {
    events
        .iter()
        .filter(|path| previous.get(*path) != current.get(*path))
        .cloned()
        .collect()
}

/// Applies a batch of changed paths in the Bank dir: re-scans the Bank when any `*.toml` changed, and swaps in the active Preset's file when its content differs from the live Preset; a broken or removed active file keeps the running Rack.
pub fn apply(state: &mut EngineState, changed: &[PathBuf]) -> Vec<Action> {
    let changed: Vec<&PathBuf> = changed.iter().filter(|path| is_toml(path)).collect();
    if changed.is_empty() {
        return Vec::new();
    }
    state.bank = match Bank::load_dir(&state.bank.dir) {
        Ok(bank) => bank,
        Err(err) => {
            return vec![Action::LogAndKeep {
                file: state.bank.dir.clone(),
                error: err.to_string(),
            }];
        }
    };
    state.revision += 1;
    let mut actions = vec![Action::Reindex];
    let Some(active_file) = state.active.as_ref().map(|(file, _)| file.clone()) else {
        return actions;
    };
    if !changed.iter().any(|path| same_file(path, &active_file)) {
        return actions;
    }
    match state.bank.position(&active_file) {
        Some(program) => actions.extend(reload(state, program)),
        None => actions.push(Action::Warn(format!(
            "active preset file {} removed; still running",
            active_file.display()
        ))),
    }
    actions
}

fn reload(state: &mut EngineState, program: usize) -> Vec<Action> {
    let preset = match &state.bank.entries[program] {
        BankEntry::Invalid { file, error } => {
            return vec![Action::LogAndKeep {
                file: file.clone(),
                error: error.clone(),
            }];
        }
        BankEntry::Valid { preset, .. } => preset.clone(),
    };
    if state
        .active
        .as_ref()
        .is_some_and(|(_, live)| *live == preset)
    {
        return vec![Action::Nothing];
    }
    let file = state.bank.entries[program].file().to_path_buf();
    let built = preset.build(
        state.sample_rate,
        state.max_frames,
        &state.config.bank_dir,
        state.config.ir_max_ms,
    );
    let (rack, warnings) = match built {
        Ok(built) => built,
        Err(err) => {
            return vec![Action::LogAndKeep {
                file,
                error: err.to_string(),
            }];
        }
    };
    let mut actions: Vec<Action> = warnings
        .iter()
        .map(|warning| Action::Warn(warning.to_string()))
        .collect();
    state.queues.drain_from_audio();
    if state
        .queues
        .commands
        .push(Command::SwapRack(Box::new(rack)))
        .is_err()
    {
        actions.push(Action::EngineBusy { file });
        return actions;
    }
    state.active = Some((file, preset));
    state.revision += 1;
    actions.push(Action::Reload { program });
    actions
}

fn is_toml(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "toml")
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.file_name() == b.file_name()
}
