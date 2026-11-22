use std::sync::atomic::Ordering;
use std::time::Instant;

use serde_json::{Value, json};

use crate::arrangement::ArrangementCommand;
use crate::build::ENGINE_BUSY;
use crate::control::EngineState;
use crate::engine::Command;
use crate::metronome::{
    BAD_BPM, BAD_VOLUME, DEFAULT_BPM, DEFAULT_METER, DEFAULT_VOLUME_DB, MetronomeSettings,
    VOLUME_DB,
};
use crate::project::{ProjectError, TEMPO_BPM, is_meter};
use crate::project_state::open_project_state;
use crate::project_sync::{CLICK_COMMANDS, send};
use crate::settings_control::save_state;
use crate::state::SavedMetronome;
use crate::transport_control::{click, tempo};

/// Changes the Metronome and returns its state. With no Project open it plays its own tempo and meter, saved in `state.toml` with the volume; with one open `on` is the Project's click and `bpm` and `meter` its tempo and meter.
pub(crate) fn metronome(
    state: &mut EngineState,
    on: Option<bool>,
    bpm: Option<f64>,
    meter: Option<(u32, u32)>,
    volume: Option<f64>,
) -> Result<Value, String> {
    if bpm.is_some_and(|bpm| !TEMPO_BPM.contains(&bpm)) {
        return Err(BAD_BPM.to_string());
    }
    if meter.is_some_and(|meter| !is_meter(meter)) {
        return Err(ProjectError::Meter.to_string());
    }
    if volume.is_some_and(|db| !VOLUME_DB.contains(&db)) {
        return Err(BAD_VOLUME.to_string());
    }
    let project_open = open_project_state(&state.projects).is_some();
    let needed = if project_open {
        usize::from(bpm.is_some() || meter.is_some())
            + if on.is_some() { CLICK_COMMANDS } else { 0 }
            + usize::from(volume.is_some())
    } else {
        1
    };
    if state.queues.commands.slots() < needed {
        return Err(ENGINE_BUSY.to_string());
    }
    let mut saved = state.config.state.clone();
    saved.metronome.volume_db = volume.or(saved.metronome.volume_db);
    if !project_open {
        saved.metronome.bpm = bpm.or(saved.metronome.bpm);
        saved.metronome.meter = meter.or(saved.metronome.meter);
    }
    if saved != state.config.state {
        save_state(&state.config, &saved)?;
        state.config.state = saved;
    }
    if project_open {
        if bpm.is_some() || meter.is_some() {
            tempo(state, bpm, meter)?;
        }
        if on.is_some() {
            click(state, on, None)?;
        }
        if let Some(db) = volume {
            send(state, &[ArrangementCommand::ClickVolume(db as f32)])?;
        }
    } else {
        let on = on.unwrap_or(state.metronome.on);
        let settings = practice_settings(state, on);
        state
            .queues
            .commands
            .push(Command::Metronome(settings))
            .map_err(|_| ENGINE_BUSY.to_string())?;
        state.metronome.on = on;
    }
    state.revision += 1;
    Ok(metronome_status(state))
}

/// Taps the tempo; from the second tap of a series sets it as `metronome` does with `bpm`.
pub(crate) fn metronome_tap(state: &mut EngineState) -> Result<Value, String> {
    let Some(bpm) = state.metronome.taps.tap(Instant::now()) else {
        return Ok(json!({ "bpm": null }));
    };
    metronome(state, None, Some(bpm), None, None)?;
    Ok(json!({ "bpm": bpm }))
}

/// The `metronome` field of `status`: the Project's click, tempo and meter while one is open; `beat` is 0 while `on` is false, also during a count-in with the click off.
pub(crate) fn metronome_status(state: &EngineState) -> Value {
    let saved = &state.config.state.metronome;
    let (on, bpm, meter) = match open_project_state(&state.projects) {
        Some((_, project, _)) => (project.click, project.tempo_bpm, project.meter),
        None => (state.metronome.on, saved_bpm(saved), saved_meter(saved)),
    };
    json!({
        "on": on,
        "bpm": bpm,
        "meter": [meter.0, meter.1],
        "volume_db": saved_volume_db(saved),
        "beats_played": state.shared.metronome_beats_played.load(Ordering::Relaxed),
        "beat": if on { state.shared.metronome_beat.load(Ordering::Relaxed) } else { 0 },
    })
}

/// What the switcher needs to play the Metronome with no Project open, at the Engine's sample rate.
pub(crate) fn practice_settings(state: &EngineState, on: bool) -> MetronomeSettings {
    let saved = &state.config.state.metronome;
    MetronomeSettings {
        on,
        bpm: saved_bpm(saved),
        meter: saved_meter(saved),
        volume_db: saved_volume_db(saved) as f32,
        sample_rate: state.sample_rate,
    }
}

/// The Metronome volume, which the Project click takes too.
pub(crate) fn click_volume_db(state: &EngineState) -> f32 {
    saved_volume_db(&state.config.state.metronome) as f32
}

fn saved_bpm(saved: &SavedMetronome) -> f64 {
    saved.bpm.unwrap_or(DEFAULT_BPM)
}

fn saved_meter(saved: &SavedMetronome) -> (u32, u32) {
    saved.meter.unwrap_or(DEFAULT_METER)
}

fn saved_volume_db(saved: &SavedMetronome) -> f64 {
    saved.volume_db.unwrap_or(DEFAULT_VOLUME_DB)
}
