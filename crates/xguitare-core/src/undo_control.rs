use serde_json::{Value, json};

use crate::control::EngineState;
use crate::history::{Change, Entry, History};
use crate::project_control::{
    NO_PROJECT, OpenProject, apply_items, open_project, open_project_mut, remove_track_at,
    restore_track,
};

const UNDO_WHILE_RECORDING: &str = "cannot undo while recording";
const REDO_WHILE_RECORDING: &str = "cannot redo while recording";
const NOTHING_TO_UNDO: &str = "nothing to undo";
const NOTHING_TO_REDO: &str = "nothing to redo";

/// Undoes the newest entry of the open Project's history through the path its edit took.
pub(crate) fn project_undo(state: &mut EngineState) -> Result<Value, String> {
    let entry = history_entry(
        state,
        UNDO_WHILE_RECORDING,
        History::next_undo,
        NOTHING_TO_UNDO,
    )?;
    let (done, label) = apply_change(state, entry.change.inverse(), entry.label)?;
    open_project_mut(state).history.undone(Entry {
        label: label.clone(),
        change: done.inverse(),
    });
    Ok(json!({ "label": label }))
}

/// Does again the newest undone entry of the open Project's history.
pub(crate) fn project_redo(state: &mut EngineState) -> Result<Value, String> {
    let entry = history_entry(
        state,
        REDO_WHILE_RECORDING,
        History::next_redo,
        NOTHING_TO_REDO,
    )?;
    let (done, label) = apply_change(state, entry.change, entry.label)?;
    open_project_mut(state).history.redone(Entry {
        label: label.clone(),
        change: done,
    });
    Ok(json!({ "label": label }))
}

/// The entry `next` picks from the open Project's history; refused while recording.
fn history_entry(
    state: &EngineState,
    while_recording: &str,
    next: impl Fn(&History) -> Option<&Entry>,
    nothing: &str,
) -> Result<Entry, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    if open.recording {
        return Err(while_recording.to_string());
    }
    next(&open.history)
        .cloned()
        .ok_or_else(|| nothing.to_string())
}

/// Does `change` to the open Project; returns it as done and `label`, naming the Track by the name it came back under.
fn apply_change(
    state: &mut EngineState,
    change: Change,
    label: String,
) -> Result<(Change, String), String> {
    match change {
        Change::Items(tracks) => {
            let open = open_project(state);
            let mut project = open.project.clone();
            for items in &tracks {
                let index = track_index(open, items.track_id)?;
                project.tracks[index].items.clone_from(&items.after);
            }
            apply_items(state, project)?;
            Ok((Change::Items(tracks), label))
        }
        Change::TrackAdded(mut placed) => {
            let name = placed.track.name.clone();
            restore_track(state, &mut placed)?;
            let label = match label.strip_suffix(&name) {
                Some(verb) if name != placed.track.name => format!("{verb}{}", placed.track.name),
                _ => label,
            };
            Ok((Change::TrackAdded(placed), label))
        }
        Change::TrackRemoved(placed) => {
            let index = track_index(open_project(state), placed.id)?;
            let removed = remove_track_at(state, index)?;
            Ok((Change::TrackRemoved(removed), label))
        }
    }
}

fn track_index(open: &OpenProject, track_id: u64) -> Result<usize, String> {
    open.track_ids
        .iter()
        .position(|&id| id == track_id)
        .ok_or_else(|| format!("the track of this edit is gone (id {track_id})"))
}
