use serde_json::{Value, json};

use crate::arrangement::ArrangementCommand;
use crate::control::{BusRef, EngineState, MixPeaks, SendLevel, TrackRef, peak_db};
use crate::engine::{MeterPoint, MixMeters};
use crate::project::{
    Bus, MAX_BUSES, Project, ProjectError, Send, Track, check_project, checked_bus_name,
};
use crate::project_control::{
    check_one_target, clear_moved_clips, commit, current_project, resolve_bus, resolve_track, send,
    swap_project,
};

const DEFAULT_BUS_NAME: &str = "Bus";

pub(crate) fn bus_add(state: &mut EngineState, name: Option<&str>) -> Result<Value, String> {
    let project = current_project(state)?;
    if project.buses.len() >= MAX_BUSES {
        return Err(ProjectError::TooManyBuses.to_string());
    }
    let name = match name {
        Some(name) => checked_bus_name(&project.buses, name, None)?,
        None => free_bus_name(project),
    };
    let id = project
        .buses
        .iter()
        .map(|bus| bus.id + 1)
        .max()
        .unwrap_or(0);
    let mut project = project.clone();
    project.buses.push(Bus {
        id,
        name,
        volume_db: 0.0,
        pan: 0.0,
        mute: false,
        solo: false,
        modules: Vec::new(),
    });
    let index = project.buses.len() - 1;
    let payload = json!({"bus": bus_label(&project, index)});
    swap_project(state, project, &mut Vec::new())?;
    Ok(payload)
}

/// Removes the Bus and every Send to it.
pub(crate) fn bus_remove(state: &mut EngineState, bus: &BusRef) -> Result<Value, String> {
    let project = current_project(state)?;
    let index = resolve_bus(project, bus)?;
    let payload = json!({"bus": bus_label(project, index)});
    let mut project = project.clone();
    let removed = project.buses.remove(index);
    for track in &mut project.tracks {
        track.sends.retain(|send| send.bus != removed.id);
    }
    swap_project(state, project, &mut Vec::new())?;
    clear_moved_clips(state, MeterPoint::Bus, index, MAX_BUSES);
    Ok(payload)
}

/// The values of a `bus_set` request; `None` leaves that value as it is.
pub(crate) struct BusChange {
    pub name: Option<String>,
    pub volume: Option<f32>,
    pub pan: Option<f32>,
    pub mute: Option<bool>,
    pub solo: Option<bool>,
}

/// Checks every value against the Project's rules before changing anything; a rename needs no rebuild, the rest are commands.
pub(crate) fn bus_set(
    state: &mut EngineState,
    bus: &BusRef,
    change: &BusChange,
) -> Result<Value, String> {
    let project = current_project(state)?;
    let index = resolve_bus(project, bus)?;
    let mut project = project.clone();
    if let Some(name) = &change.name {
        project.buses[index].name = checked_bus_name(&project.buses, name, Some(index))?;
    }
    let new = &mut project.buses[index];
    new.volume_db = change.volume.unwrap_or(new.volume_db);
    new.pan = change.pan.unwrap_or(new.pan);
    new.mute = change.mute.unwrap_or(new.mute);
    new.solo = change.solo.unwrap_or(new.solo);
    check_project(&project).map_err(|err| err.to_string())?;
    let commands: Vec<ArrangementCommand> = [
        change
            .volume
            .map(|db| ArrangementCommand::BusVolume { bus: index, db }),
        change
            .pan
            .map(|pan| ArrangementCommand::BusPan { bus: index, pan }),
        change
            .mute
            .map(|on| ArrangementCommand::BusMute { bus: index, on }),
        change
            .solo
            .map(|on| ArrangementCommand::BusSolo { bus: index, on }),
    ]
    .into_iter()
    .flatten()
    .collect();
    send(state, &commands)?;
    let bus = &project.buses[index];
    let payload = json!({"bus": {
        "id": bus.id,
        "index": index,
        "name": bus.name,
        "volume_db": bus.volume_db,
        "pan": bus.pan,
        "mute": bus.mute,
        "solo": bus.solo,
    }});
    commit(state, project);
    Ok(payload)
}

/// Changes the level and pre switch of the Track's Send to the Bus; an off Send keeps its pre switch until the Project is saved.
pub(crate) fn send_set(
    state: &mut EngineState,
    track: &TrackRef,
    bus: &BusRef,
    level: Option<SendLevel>,
    pre: Option<bool>,
) -> Result<Value, String> {
    let project = current_project(state)?;
    let track_index = resolve_track(project, track)?;
    let bus_index = resolve_bus(project, bus)?;
    let bus_id = project.buses[bus_index].id;
    let mut project = project.clone();
    let sends = &mut project.tracks[track_index].sends;
    let position = sends.iter().position(|send| send.bus == bus_id);
    let send_index = position.unwrap_or_else(|| {
        sends.push(Send {
            bus: bus_id,
            level_db: f32::NEG_INFINITY,
            pre: false,
        });
        sends.len() - 1
    });
    let entry = &mut sends[send_index];
    entry.level_db = match level {
        Some(SendLevel::Db(db)) => db,
        Some(SendLevel::Off) => f32::NEG_INFINITY,
        None => entry.level_db,
    };
    entry.pre = pre.unwrap_or(entry.pre);
    let entry = *entry;
    check_project(&project).map_err(|err| err.to_string())?;
    let (track, bus) = (track_index, bus_index);
    let commands: Vec<ArrangementCommand> = [
        level.map(|_| ArrangementCommand::SendLevel {
            track,
            bus,
            db: entry.level_db,
        }),
        pre.map(|on| ArrangementCommand::SendPre { track, bus, on }),
    ]
    .into_iter()
    .flatten()
    .collect();
    send(state, &commands)?;
    commit(state, project);
    Ok(json!({
        "track": track,
        "bus": bus,
        "level": level_on_wire(entry.level_db),
        "pre": entry.pre,
    }))
}

pub(crate) fn master_set(state: &mut EngineState, volume: f32) -> Result<Value, String> {
    let mut project = current_project(state)?.clone();
    project.master.volume_db = volume;
    check_project(&project).map_err(|err| err.to_string())?;
    send(state, &[ArrangementCommand::MasterVolume { db: volume }])?;
    commit(state, project);
    Ok(json!({ "volume_db": volume }))
}

/// Clears the clip flag of the named Track, Bus or Master, or of all with none named; the Revision stays.
pub(crate) fn clip_reset(
    state: &EngineState,
    track: Option<&TrackRef>,
    bus: Option<&BusRef>,
    master: bool,
) -> Result<Value, String> {
    check_one_target(track.is_some(), bus.is_some(), master)?;
    let (point, payload) = if let Some(track) = track {
        let index = resolve_track(current_project(state)?, track)?;
        (Some(MeterPoint::Track(index)), json!({ "track": index }))
    } else if let Some(bus) = bus {
        let index = resolve_bus(current_project(state)?, bus)?;
        (Some(MeterPoint::Bus(index)), json!({ "bus": index }))
    } else if master {
        current_project(state)?;
        (Some(MeterPoint::Master), json!({ "master": true }))
    } else {
        (None, json!({}))
    };
    state.shared.mix_meters.clip_reset(point);
    Ok(payload)
}

/// The Track's Sends that are not off, each with its Bus's index.
pub(crate) fn sends_status(project: &Project, track: &Track) -> Value {
    let sends: Vec<Value> = track
        .sends
        .iter()
        .filter(|send| send.level_db != f32::NEG_INFINITY)
        .filter_map(|send| {
            let bus = project.buses.iter().position(|bus| bus.id == send.bus)?;
            Some(json!({"bus": bus, "level_db": send.level_db, "pre": send.pre}))
        })
        .collect();
    json!(sends)
}

pub(crate) fn buses_status(project: &Project, mix: &MixPeaks, meters: &MixMeters) -> Value {
    let buses: Vec<Value> = project
        .buses
        .iter()
        .enumerate()
        .map(|(index, bus)| {
            json!({
                "id": bus.id,
                "name": bus.name,
                "volume_db": bus.volume_db,
                "pan": bus.pan,
                "mute": bus.mute,
                "solo": bus.solo,
                "peak_db": peaks_db(mix.buses[index]),
                "clip": meters.clip(MeterPoint::Bus(index)),
            })
        })
        .collect();
    json!(buses)
}

pub(crate) fn master_status(project: &Project, mix: &MixPeaks, meters: &MixMeters) -> Value {
    json!({
        "volume_db": project.master.volume_db,
        "peak_db": peaks_db(mix.master),
        "clip": meters.clip(MeterPoint::Master),
    })
}

/// Left and right peaks in dB, as `peak_db`.
pub(crate) fn peaks_db([left, right]: [f32; 2]) -> Value {
    json!([peak_db(left), peak_db(right)])
}

fn level_on_wire(level_db: f32) -> SendLevel {
    if level_db == f32::NEG_INFINITY {
        SendLevel::Off
    } else {
        SendLevel::Db(level_db)
    }
}

fn bus_label(project: &Project, index: usize) -> Value {
    let bus = &project.buses[index];
    json!({"id": bus.id, "index": index, "name": bus.name})
}

#[allow(
    clippy::maybe_infinite_iter,
    reason = "a Project has at most 8 Buses, so some number is free"
)]
fn free_bus_name(project: &Project) -> String {
    (1..)
        .map(|n| format!("{DEFAULT_BUS_NAME} {n}"))
        .find(|name| checked_bus_name(&project.buses, name, None).is_ok())
        .expect("a Project has at most 8 Buses, so some number is free")
}
