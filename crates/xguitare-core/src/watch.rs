use std::path::{Path, PathBuf};

use tracing::warn;

use crate::control::{ENGINE_BUSY, EngineState};
use crate::engine::Command;
use crate::{Bank, BankEntry};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Reindex,
    Reload { program: usize },
    LogAndKeep { file: PathBuf, error: String },
    Nothing,
}

/// Applies a batch of changed paths in the Bank dir: re-scans the Bank when any `*.toml` changed, and swaps in the active Preset's file when its content differs from the live Preset; a broken or removed active file keeps the running Rack.
pub fn apply(state: &mut EngineState, changed: &[PathBuf]) -> Vec<Action> {
    let changed: Vec<&PathBuf> = changed.iter().filter(|path| is_toml(path)).collect();
    if changed.is_empty() {
        return Vec::new();
    }
    let active_file = state
        .active
        .as_ref()
        .and_then(|(program, _)| state.bank.entries.get(*program))
        .map(|entry| entry.file().to_path_buf());
    state.bank = match Bank::load_dir(&state.bank.dir) {
        Ok(bank) => bank,
        Err(err) => {
            return vec![Action::LogAndKeep {
                file: state.bank.dir.clone(),
                error: err.to_string(),
            }];
        }
    };
    let mut actions = vec![Action::Reindex];
    let Some(active_file) = active_file else {
        return actions;
    };
    let Some(program) = position(&state.bank, &active_file) else {
        warn!(
            "active preset file {} removed; still running",
            active_file.display()
        );
        return actions;
    };
    if let Some(active) = state.active.as_mut() {
        active.0 = program;
    }
    if changed.iter().any(|path| same_file(path, &active_file)) {
        actions.push(reload(state, program));
    }
    actions
}

fn reload(state: &mut EngineState, program: usize) -> Action {
    let preset = match &state.bank.entries[program] {
        BankEntry::Invalid { file, error } => {
            return Action::LogAndKeep {
                file: file.clone(),
                error: error.clone(),
            };
        }
        BankEntry::Valid { preset, .. } => preset.clone(),
    };
    if state
        .active
        .as_ref()
        .is_some_and(|(_, live)| *live == preset)
    {
        return Action::Nothing;
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
            return Action::LogAndKeep {
                file,
                error: err.to_string(),
            };
        }
    };
    for warning in &warnings {
        warn!("{warning}");
    }
    state.queues.drain_from_audio();
    if state
        .queues
        .commands
        .push(Command::SwapRack(Box::new(rack)))
        .is_err()
    {
        return Action::LogAndKeep {
            file,
            error: ENGINE_BUSY.to_string(),
        };
    }
    state.active = Some((program, preset));
    Action::Reload { program }
}

fn is_toml(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "toml")
}

fn position(bank: &Bank, file: &Path) -> Option<usize> {
    bank.entries
        .iter()
        .position(|entry| same_file(entry.file(), file))
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.file_name() == b.file_name()
}
