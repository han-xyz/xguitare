use serde_json::{Value, json};
use xguitare_protocol::{LoopRegion, Position, TransportAction, position_frame};

use crate::arrangement::ArrangementCommand;
use crate::control::EngineState;
use crate::project::{Item, Project, check_project};
use crate::project_control::{NO_PROJECT, autosave_on_stop, open_project_mut};
use crate::project_sync::{click_commands, commit, project_rate_warning, send};

const NOTHING_ARMED: &str = "no track is armed";
const NEEDS_POSITION: &str = "locate needs a position";
const POSITION_TOO_FAR: &str = "position is more than an hour past the end of the project";

/// How far past the Project's last Item or loop end `locate` may move the playhead.
const LOCATE_SECONDS_PAST_END: f64 = 3600.0;

pub(crate) fn transport(
    state: &mut EngineState,
    action: TransportAction,
    position: Option<&Position>,
) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let (command, frames) = match action {
        TransportAction::Play | TransportAction::Record
            if let Some(mismatch) = project_rate_warning(state) =>
        {
            return Err(mismatch);
        }
        TransportAction::Play => (ArrangementCommand::Play, None),
        TransportAction::Stop => (ArrangementCommand::Stop, None),
        TransportAction::Record => {
            if !open.project.tracks.iter().any(|track| track.arm) {
                return Err(NOTHING_ARMED.to_string());
            }
            (ArrangementCommand::Record, None)
        }
        TransportAction::Locate => {
            let frame = locate_frame(&open.project, position.ok_or(NEEDS_POSITION)?)?;
            (ArrangementCommand::Locate(frame), Some(frame))
        }
    };
    send(state, &[command])?;
    let open = open_project_mut(state);
    match action {
        TransportAction::Record => {
            open.recording = true;
            open.recorded = true;
        }
        TransportAction::Play | TransportAction::Stop => open.recording = false,
        TransportAction::Locate => {}
    }
    let mut payload = json!({ "action": action });
    if action == TransportAction::Stop {
        // a failed write leaves the Project changed, so the timer retries and logs it
        autosave_on_stop(open);
    }
    if let Some(frames) = frames {
        payload["frames"] = json!(frames);
    }
    Ok(payload)
}

pub(crate) fn set_loop(state: &mut EngineState, region: LoopRegion) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let mut project = open.project.clone();
    project.loop_region = region;
    check_project(&project).map_err(|err| err.to_string())?;
    send(state, &[ArrangementCommand::Loop(region)])?;
    commit(state, project);
    Ok(json!({"start": region.start, "end": region.end, "on": region.on}))
}

pub(crate) fn tempo(
    state: &mut EngineState,
    bpm: Option<f64>,
    meter: Option<(u32, u32)>,
) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let mut project = open.project.clone();
    project.tempo_bpm = bpm.unwrap_or(project.tempo_bpm);
    project.meter = meter.unwrap_or(project.meter);
    check_project(&project).map_err(|err| err.to_string())?;
    let (bpm, meter) = (project.tempo_bpm, project.meter);
    send(state, &[ArrangementCommand::Tempo { bpm, meter }])?;
    commit(state, project);
    Ok(json!({"bpm": bpm, "meter": [meter.0, meter.1]}))
}

pub(crate) fn click(
    state: &mut EngineState,
    on: Option<bool>,
    count_in: Option<u32>,
) -> Result<Value, String> {
    let open = state.projects.open.as_ref().ok_or(NO_PROJECT)?;
    let mut project = open.project.clone();
    project.click = on.unwrap_or(project.click);
    project.count_in = count_in.unwrap_or(project.count_in);
    check_project(&project).map_err(|err| err.to_string())?;
    let (on, count_in) = (project.click, project.count_in);
    send(state, &click_commands(state, &project))?;
    commit(state, project);
    Ok(json!({"on": on, "count_in": count_in}))
}

/// The frame of `<bar>.<beat>` or `<bar>` (both from 1, the beat within the meter), `<seconds>s` or a frame number, at most an hour past the Project's end.
fn locate_frame(project: &Project, position: &Position) -> Result<u64, String> {
    let frame = match position {
        Position::Frames(frames) => *frames as f64,
        Position::Text(text) => text_frame(project, text.trim())?,
    };
    let last =
        project_end(project) as f64 + LOCATE_SECONDS_PAST_END * f64::from(project.sample_rate);
    if frame > last {
        return Err(POSITION_TOO_FAR.to_string());
    }
    Ok(frame as u64)
}

/// The frame a text position names, rounded; not bounded.
fn text_frame(project: &Project, text: &str) -> Result<f64, String> {
    position_frame(
        text,
        project.sample_rate,
        project.tempo_bpm,
        project.meter.0,
    )
}

/// The last frame any Item or the loop region reaches.
fn project_end(project: &Project) -> u64 {
    let items = project.tracks.iter().flat_map(|track| &track.items);
    items
        .map(Item::end)
        .chain([project.loop_region.end])
        .max()
        .unwrap_or(0)
}
