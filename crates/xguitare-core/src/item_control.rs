use serde_json::{Value, json};

use crate::control::{EngineState, TrackRef};
use crate::project::{Item, Project};
use crate::project_control::{
    clipboard, current_project, edit_items, editable_project, fill_clipboard, gone_take_file,
    item_json, resolve_track,
};

const NO_ITEMS: &str = "give at least one item";
const CLIPBOARD_EMPTY: &str = "clipboard is empty";

/// An Item in the clipboard: `item.position` counted from the start of the group copied, `track` its Track's distance from the Track of the first Item named.
#[derive(Clone)]
pub(crate) struct CopiedItem {
    pub track: isize,
    pub item: Item,
}

/// Moves the Items `ids` by one `delta`, clamped so none starts before frame 0, onto the top of their Tracks; with `track`, the first one lands on that Track and the others keep their Track distance to it.
pub(crate) fn item_move(
    state: &mut EngineState,
    ids: &[u64],
    delta: i64,
    track: Option<&TrackRef>,
) -> Result<Value, String> {
    let (project, _) = editable_project(state)?;
    let ids = distinct(ids)?;
    let mut places = ids
        .iter()
        .map(|&id| find(project, id))
        .collect::<Result<Vec<_>, _>>()?;
    let to_first = track
        .map(|track| resolve_track(project, track))
        .transpose()?;
    let first = places[0].0;
    let tracks = project.tracks.len();
    let target = |from: usize| match to_first {
        Some(to) => (from + to).checked_sub(first).filter(|&to| to < tracks),
        None => Some(from),
    };
    if let Some((id, _)) = ids
        .iter()
        .zip(&places)
        .find(|(_, (from, _))| target(*from).is_none())
    {
        return Err(format!(
            "item {id} would move to a track that does not exist"
        ));
    }
    let lowest = places
        .iter()
        .map(|&(track, index)| project.tracks[track].items[index].position)
        .min()
        .unwrap_or(0);
    let delta = i128::from(delta).max(-i128::from(lowest));
    let mut project = project.clone();
    places.sort_unstable();
    let mut moved: Vec<_> = places
        .iter()
        .rev()
        .map(|&(track, index)| (track, project.tracks[track].items.remove(index)))
        .collect();
    moved.reverse();
    let mut order = Vec::with_capacity(moved.len());
    for (from, mut item) in moved {
        item.position = u64::try_from(i128::from(item.position) + delta).unwrap_or(u64::MAX);
        order.push(item.id);
        project.tracks[target(from).expect("checked above")]
            .items
            .push(item);
    }
    let reply = changed(&project, &order);
    edit_items(state, project, count_label("move", order.len()))?;
    Ok(reply)
}

/// Moves the Item's start and end to the timeline frames given, each clamped to its active Take's extent.
pub(crate) fn item_trim(
    state: &mut EngineState,
    id: u64,
    start: Option<u64>,
    end: Option<u64>,
) -> Result<Value, String> {
    let (project, _) = editable_project(state)?;
    let (track, index) = find(project, id)?;
    let item = &project.tracks[track].items[index];
    let take_start = i128::from(item.position) - i128::from(item.offset);
    let first = take_start.max(0);
    let last = take_start + i128::from(item.takes[item.take].frames);
    let clamp = |frame: u64| i128::from(frame).clamp(first, last);
    let new_start = start.map_or(i128::from(item.position), clamp);
    let new_end = end.map_or(i128::from(item.end()), clamp);
    if new_end <= new_start {
        return Err(format!("item {id} would have no frames left"));
    }
    if (new_start, new_end) == (i128::from(item.position), i128::from(item.end())) {
        return Ok(changed(project, &[id]));
    }
    let mut project = project.clone();
    let item = &mut project.tracks[track].items[index];
    item.position = new_start as u64;
    item.offset = (new_start - take_start) as u64;
    if end.is_some() || item.length.is_some() {
        item.length = Some((new_end - new_start) as u64);
    }
    let reply = changed(&project, &[id]);
    edit_items(state, project, "trim".to_string())?;
    Ok(reply)
}

/// Splits each Item that `at` falls strictly inside, of `ids` or else on the selected Track, into two that share its Takes; the right one gets a new id and stays at the Item's place in the stack.
pub(crate) fn item_split(
    state: &mut EngineState,
    ids: Option<&[u64]>,
    at: u64,
) -> Result<Value, String> {
    let (project, selected) = editable_project(state)?;
    let candidates = match ids {
        Some(ids) => distinct(ids)?
            .into_iter()
            .map(|id| find(project, id))
            .collect::<Result<Vec<_>, _>>()?,
        None => project.tracks.get(selected).map_or_else(Vec::new, |track| {
            (0..track.items.len())
                .map(|index| (selected, index))
                .collect()
        }),
    };
    let inside: Vec<(usize, u64)> = candidates
        .into_iter()
        .map(|(track, index)| (track, &project.tracks[track].items[index]))
        .filter(|(_, item)| item.position < at && at < item.end())
        .map(|(track, item)| (track, item.id))
        .collect();
    if inside.is_empty() {
        return Err(format!("nothing to split at {at}"));
    }
    let mut project = project.clone();
    let mut halves = Vec::with_capacity(2 * inside.len());
    for (track, id) in inside {
        let right_id = project.next_item_id;
        project.next_item_id += 1;
        let items = &mut project.tracks[track].items;
        let index = items
            .iter()
            .position(|item| item.id == id)
            .expect("found above");
        let left = &mut items[index];
        let cut = at - left.position;
        let mut right = left.clone();
        left.length = Some(cut);
        right.id = right_id;
        right.position = at;
        right.offset += cut;
        right.length = right.length.map(|length| length - cut);
        items.insert(index + 1, right);
        halves.extend([id, right_id]);
    }
    let reply = changed(&project, &halves);
    edit_items(state, project, "split".to_string())?;
    Ok(reply)
}

pub(crate) fn item_delete(state: &mut EngineState, ids: &[u64]) -> Result<Value, String> {
    let (project, _) = editable_project(state)?;
    let ids = distinct(ids)?;
    let mut places = ids
        .iter()
        .map(|&id| find(project, id))
        .collect::<Result<Vec<_>, _>>()?;
    let mut project = project.clone();
    places.sort_unstable();
    for &(track, index) in places.iter().rev() {
        project.tracks[track].items.remove(index);
    }
    edit_items(state, project, count_label("delete", ids.len()))?;
    Ok(json!({ "deleted": ids }))
}

/// Makes the Item's Take at index `take` the one that plays, its length cut to what that Take holds after the Item's offset.
pub(crate) fn item_take(state: &mut EngineState, id: u64, take: usize) -> Result<Value, String> {
    let (project, _) = editable_project(state)?;
    let (track, index) = find(project, id)?;
    let item = &project.tracks[track].items[index];
    let Some(frames) = item.takes.get(take).map(|take| take.frames) else {
        return Err(format!("item {id} has no take {take}"));
    };
    if item.offset >= frames {
        return Err(format!("item {id} starts past the end of take {take}"));
    }
    let mut project = project.clone();
    let item = &mut project.tracks[track].items[index];
    item.take = take;
    item.length = item.length.map(|length| length.min(frames - item.offset));
    let reply = changed(&project, &[id]);
    edit_items(state, project, format!("take {}", take + 1))?;
    Ok(reply)
}

/// Copies the Items `ids`, each onto the top of its own Track, so the copies start where the group of `ids` ends; the copies get new ids.
pub(crate) fn item_duplicate(state: &mut EngineState, ids: &[u64]) -> Result<Value, String> {
    let (project, _) = editable_project(state)?;
    let places = sorted_places(project, ids)?;
    let (start, end) = group_extent(project, &places);
    let mut project = project.clone();
    let mut copies = Vec::with_capacity(places.len());
    for (track, index) in places {
        let mut copy = project.tracks[track].items[index].clone();
        copy.id = project.next_item_id;
        project.next_item_id += 1;
        copy.position = copy.position.saturating_add(end - start);
        copies.push(copy.id);
        project.tracks[track].items.push(copy);
    }
    let reply = changed(&project, &copies);
    edit_items(state, project, count_label("duplicate", copies.len()))?;
    Ok(reply)
}

/// Puts the Items `ids` in the open Project's clipboard, replacing what it held.
pub(crate) fn item_copy(state: &mut EngineState, ids: &[u64]) -> Result<Value, String> {
    let project = current_project(state)?;
    let ids = distinct(ids)?;
    let first = find(project, ids[0])?.0;
    let places = sorted_places(project, &ids)?;
    let (start, _) = group_extent(project, &places);
    let copied = places
        .into_iter()
        .map(|(track, index)| {
            let mut item = project.tracks[track].items[index].clone();
            item.position -= start;
            CopiedItem {
                track: track.cast_signed() - first.cast_signed(),
                item,
            }
        })
        .collect();
    fill_clipboard(state, copied);
    Ok(json!({ "copied": ids }))
}

/// Places the clipboard onto the top of its Tracks, its first Item on `track` and the group's start at `at`; Items that would land on a Track that does not exist are left out and counted in `dropped`.
pub(crate) fn item_paste(
    state: &mut EngineState,
    track: &TrackRef,
    at: u64,
) -> Result<Value, String> {
    let (project, _) = editable_project(state)?;
    let copied = clipboard(state);
    if copied.is_empty() {
        return Err(CLIPBOARD_EMPTY.to_string());
    }
    let to_first = resolve_track(project, track)?;
    let mut project = project.clone();
    let mut pasted = Vec::with_capacity(copied.len());
    for entry in copied {
        let Some(track) = to_first
            .checked_add_signed(entry.track)
            .filter(|&track| track < project.tracks.len())
        else {
            continue;
        };
        let mut item = entry.item.clone();
        item.id = project.next_item_id;
        project.next_item_id += 1;
        item.position = item.position.saturating_add(at);
        pasted.push((track, item));
    }
    if let Some(file) = gone_take_file(state, pasted.iter().map(|(_, item)| item)) {
        return Err(format!("cannot paste: take {file} no longer exists"));
    }
    let dropped = copied.len() - pasted.len();
    let ids: Vec<u64> = pasted.iter().map(|(_, item)| item.id).collect();
    for (track, item) in pasted {
        project.tracks[track].items.push(item);
    }
    let mut reply = changed(&project, &ids);
    reply["dropped"] = json!(dropped);
    edit_items(state, project, count_label("paste", ids.len()))?;
    Ok(reply)
}

/// `verb Item` for one Item, `verb <n> Items` for more.
fn count_label(verb: &str, items: usize) -> String {
    if items == 1 {
        format!("{verb} Item")
    } else {
        format!("{verb} {items} Items")
    }
}

/// `ids` with repeats left out; at least one.
fn distinct(ids: &[u64]) -> Result<Vec<u64>, String> {
    if ids.is_empty() {
        return Err(NO_ITEMS.to_string());
    }
    let mut seen = Vec::with_capacity(ids.len());
    for &id in ids {
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    Ok(seen)
}

/// The Track index and the index in its Items of the Item `id`.
fn find(project: &Project, id: u64) -> Result<(usize, usize), String> {
    project
        .tracks
        .iter()
        .enumerate()
        .find_map(|(track, found)| {
            found
                .items
                .iter()
                .position(|item| item.id == id)
                .map(|index| (track, index))
        })
        .ok_or_else(|| format!("no such item: {id}"))
}

/// The places of the Items `ids`, as [`find`] gives them, in Track and stacking order.
fn sorted_places(project: &Project, ids: &[u64]) -> Result<Vec<(usize, usize)>, String> {
    let mut places = distinct(ids)?
        .into_iter()
        .map(|id| find(project, id))
        .collect::<Result<Vec<_>, _>>()?;
    places.sort_unstable();
    Ok(places)
}

/// The earliest start and the latest end of the Items at `places`.
fn group_extent(project: &Project, places: &[(usize, usize)]) -> (u64, u64) {
    let items = || {
        places
            .iter()
            .map(|&(track, index)| &project.tracks[track].items[index])
    };
    let start = items().map(|item| item.position).min().unwrap_or(0);
    let end = items().map(Item::end).max().unwrap_or(0);
    (start, end)
}

/// The reply of an edit: the Items `ids` of `project`, each with its Track index.
fn changed(project: &Project, ids: &[u64]) -> Value {
    let items: Vec<Value> = ids
        .iter()
        .filter_map(|&id| find(project, id).ok())
        .map(|(track, index)| {
            let mut item = item_json(project, &project.tracks[track].items[index]);
            item["track"] = json!(track);
            item
        })
        .collect();
    json!({ "items": items })
}
