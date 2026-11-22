use serde_json::{Value, json};

use crate::arrangement::ArrangementCommand;
use crate::control::{EngineState, MixPeaks, peak_db};
use crate::engine::{MeterPoint, MixMeters};
use crate::project::{
    Bus, MAX_BUSES, MAX_TRACKS, Project, ProjectError, Send, Track, check_project, checked_bus_name,
};
use crate::project_control::check_one_target;
use crate::project_state::{current_project, new_bus_id, resolve_bus, resolve_track};
use crate::project_sync::{commit, send, swap_project, swap_project_moving};
use xguitare_protocol::{BusRef, BusStatus, MasterStatus, SendLevel, SendStatus, TrackRef};

const DEFAULT_BUS_NAME: &str = "Bus";

pub(crate) fn bus_add(state: &mut EngineState, name: Option<&str>) -> Result<Value, String> {
    let project = current_project(&state.projects)?;
    if project.buses.len() >= MAX_BUSES {
        return Err(ProjectError::TooManyBuses.to_string());
    }
    let name = match name {
        Some(name) => checked_bus_name(&project.buses, name, None)?,
        None => free_bus_name(project),
    };
    let mut project = project.clone();
    let id = new_bus_id(&mut state.projects);
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
    let project = current_project(&state.projects)?;
    let index = resolve_bus(project, bus)?;
    let payload = json!({"bus": bus_label(project, index)});
    let mut project = project.clone();
    let removed = project.buses.remove(index);
    for track in &mut project.tracks {
        track.sends.retain(|send| send.bus != removed.id);
    }
    swap_project_moving(state, project, (MAX_TRACKS, index), &mut Vec::new())?;
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
    let project = current_project(&state.projects)?;
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
    let project = current_project(&state.projects)?;
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
        Some(SendLevel::Db(db)) if !db.is_finite() => {
            return Err(
                ProjectError::SendLevel(project.tracks[track_index].name.clone()).to_string(),
            );
        }
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

pub(crate) fn master_set(state: &mut EngineState, volume: SendLevel) -> Result<Value, String> {
    let mut project = current_project(&state.projects)?.clone();
    let volume = match volume {
        SendLevel::Db(db) if !db.is_finite() => {
            return Err(ProjectError::MasterVolume.to_string());
        }
        SendLevel::Db(db) => db,
        SendLevel::Off => f32::NEG_INFINITY,
    };
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
        let index = resolve_track(current_project(&state.projects)?, track)?;
        (Some(MeterPoint::Track(index)), json!({ "track": index }))
    } else if let Some(bus) = bus {
        let index = resolve_bus(current_project(&state.projects)?, bus)?;
        (Some(MeterPoint::Bus(index)), json!({ "bus": index }))
    } else if master {
        current_project(&state.projects)?;
        (Some(MeterPoint::Master), json!({ "master": true }))
    } else {
        (None, json!({}))
    };
    state.shared.mix_meters.clip_reset(point);
    Ok(payload)
}

/// The Track's Sends that are not off, each with its Bus's index.
pub(crate) fn sends_status(project: &Project, track: &Track) -> Vec<SendStatus> {
    track
        .sends
        .iter()
        .filter(|send| send.level_db != f32::NEG_INFINITY)
        .filter_map(|send| {
            let bus = project.buses.iter().position(|bus| bus.id == send.bus)?;
            Some(SendStatus {
                bus,
                level_db: Some(on_wire(send.level_db)),
                pre: send.pre,
            })
        })
        .collect()
}

pub(crate) fn buses_status(
    project: &Project,
    mix: &MixPeaks,
    meters: &MixMeters,
) -> Vec<BusStatus> {
    project
        .buses
        .iter()
        .enumerate()
        .map(|(index, bus)| BusStatus {
            id: bus.id,
            name: bus.name.clone(),
            volume_db: volume_status(bus.volume_db),
            pan: on_wire(bus.pan),
            mute: bus.mute,
            solo: bus.solo,
            peak_db: peaks_db(mix.buses[index]),
            clip: meters.clip(MeterPoint::Bus(index)),
        })
        .collect()
}

pub(crate) fn master_status(project: &Project, mix: &MixPeaks, meters: &MixMeters) -> MasterStatus {
    MasterStatus {
        volume_db: volume_status(project.master.volume_db),
        peak_db: peaks_db(mix.master),
        clip: meters.clip(MeterPoint::Master),
    }
}

/// Left and right peaks in dB, as `peak_db`.
pub(crate) fn peaks_db([left, right]: [f32; 2]) -> [Option<f64>; 2] {
    [peak_db(left), peak_db(right)]
}

/// A volume in dB as `status` shows it: `None` when off or not a finite number.
pub(crate) fn volume_status(volume_db: f32) -> Option<f64> {
    volume_db.is_finite().then(|| on_wire(volume_db))
}

/// An `f32` as a reply carries it: widened exactly, which is what `serde_json` wrote for an `f32` before the replies were typed, so that the wire text stays as it was.
pub(crate) fn on_wire(value: f32) -> f64 {
    f64::from(value)
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
