use std::collections::VecDeque;

use crate::project::Item;

/// How many entries undo keeps; a new one past it drops the oldest.
pub(crate) const HISTORY_DEPTH: usize = 200;

/// The open Project's undo and redo stacks; the newest entry is last.
#[derive(Default)]
pub(crate) struct History {
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
}

#[derive(Clone)]
pub(crate) struct Entry {
    pub label: String,
    pub change: Change,
}

/// An edit as it was done; [`Change::inverse`] undoes it.
#[derive(Clone)]
pub(crate) enum Change {
    Items(Vec<TrackItems>),
}

/// One Track's Items before and after an edit, the Track named by its id.
#[derive(Clone)]
pub(crate) struct TrackItems {
    pub track_id: u64,
    pub before: Vec<Item>,
    pub after: Vec<Item>,
}

impl Change {
    pub(crate) fn inverse(self) -> Change {
        match self {
            Change::Items(tracks) => Change::Items(
                tracks
                    .into_iter()
                    .map(|items| TrackItems {
                        track_id: items.track_id,
                        before: items.after,
                        after: items.before,
                    })
                    .collect(),
            ),
        }
    }
}

impl History {
    /// Records a new edit; redo is cleared.
    pub(crate) fn push(&mut self, entry: Entry) {
        self.redo.clear();
        self.push_undo(entry);
    }

    pub(crate) fn undo_label(&self) -> Option<&str> {
        self.undo.back().map(|entry| entry.label.as_str())
    }

    pub(crate) fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|entry| entry.label.as_str())
    }

    pub(crate) fn next_undo(&self) -> Option<&Entry> {
        self.undo.back()
    }

    pub(crate) fn next_redo(&self) -> Option<&Entry> {
        self.redo.last()
    }

    /// Replaces the newest undo entry, now undone, by `redo` on the redo stack.
    pub(crate) fn undone(&mut self, redo: Entry) {
        self.undo.pop_back();
        self.redo.push(redo);
    }

    /// Replaces the newest redo entry, now redone, by `undo` on the undo stack.
    pub(crate) fn redone(&mut self, undo: Entry) {
        self.redo.pop();
        self.push_undo(undo);
    }

    fn push_undo(&mut self, entry: Entry) {
        if self.undo.len() == HISTORY_DEPTH {
            self.undo.pop_front();
        }
        self.undo.push_back(entry);
    }
}
