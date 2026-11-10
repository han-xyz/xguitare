use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::control::EngineState;
use crate::project_control::{NO_PROJECT, ProjectEvent, open_project_state};
use crate::render::{
    RENDERS_DIR, Render, RenderBits, RenderJob, RenderRange, RenderState, RenderTask,
};

const RENDER_RUNNING: &str = "a render is already running";
const NO_RENDER_RUNNING: &str = "no render running";
const NOT_A_DIRECTORY: &str = "renders is not a directory";

/// How a Render ended, for the Daemon to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderFinished {
    Rendered(PathBuf),
    Cancelled,
    Failed(String),
}

/// Starts a Render of the open Project's current state into `<project>/renders/<name>.wav`, never over an existing file; the Daemon runs it from the [`ProjectEvent::Render`] this queues.
pub(crate) fn render(
    state: &mut EngineState,
    range: Option<RenderRange>,
    bits: Option<RenderBits>,
    name: Option<&str>,
) -> Result<Value, String> {
    let (dir, project, takes) = open_project_state(state).ok_or(NO_PROJECT)?;
    if is_running(state) {
        return Err(RENDER_RUNNING.to_string());
    }
    let name = match name {
        Some(name) => checked_render_name(name)?.to_string(),
        None => folder_name(dir),
    };
    let range = range.unwrap_or(RenderRange::Project);
    let render = Render::new(
        &state.config,
        project,
        takes,
        range,
        bits.unwrap_or(RenderBits::Pcm24),
    )
    .map_err(|err| err.to_string())?;
    let renders = renders_dir(dir)?;
    let job = Arc::new(RenderJob::new(name, range, renders.clone()));
    let file = job.file();
    if file.parent() != Some(renders.as_path()) {
        return Err(format!("invalid render name '{}'", job.name));
    }
    let projects = &mut state.projects;
    projects.render = Some(Arc::clone(&job));
    projects.render_reported = false;
    projects
        .events
        .push(ProjectEvent::Render(RenderTask { render, job }));
    state.revision += 1;
    Ok(json!({"file": file.display().to_string()}))
}

pub(crate) fn render_cancel(state: &mut EngineState) -> Result<Value, String> {
    if !is_running(state) {
        return Err(NO_RENDER_RUNNING.to_string());
    }
    cancel_render(state);
    Ok(json!({}))
}

/// Asks a running Render to stop, as closing the Project and stopping the Daemon do.
pub fn cancel_render(state: &mut EngineState) {
    if let Some(job) = &state.projects.render {
        job.cancel();
    }
}

/// How the last Render ended, once, the first time this is called after it ended; bumps the Revision then.
pub fn finished_render(state: &mut EngineState) -> Option<RenderFinished> {
    let job = state.projects.render.as_ref()?;
    if state.projects.render_reported {
        return None;
    }
    let finished = match job.state() {
        RenderState::Running => return None,
        RenderState::Done => RenderFinished::Rendered(job.file()),
        RenderState::Cancelled => RenderFinished::Cancelled,
        RenderState::Failed => RenderFinished::Failed(job.error().unwrap_or_default()),
    };
    state.projects.render_reported = true;
    state.revision += 1;
    Some(finished)
}

/// The status `render` field: `null` before the first Render, then the running one or the last finished.
pub(crate) fn render_status(state: &EngineState) -> Value {
    let Some(job) = &state.projects.render else {
        return Value::Null;
    };
    let mut status = json!({
        "name": job.name,
        "range": job.range,
        "progress": job.progress(),
        "state": job.state(),
        "file": job.file().display().to_string(),
    });
    if let Some(error) = job.error() {
        status["error"] = json!(error);
    }
    status
}

fn is_running(state: &EngineState) -> bool {
    state
        .projects
        .render
        .as_ref()
        .is_some_and(|job| job.state() == RenderState::Running)
}

/// A render name becomes a file name, so it must be one path segment that is not hidden.
fn checked_render_name(name: &str) -> Result<&str, String> {
    let invalid = name.is_empty()
        || name.contains('/')
        || name.contains('\0')
        || name.contains("..")
        || name.starts_with('.');
    if invalid {
        return Err(format!("invalid render name '{name}'"));
    }
    Ok(name)
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `<dir>/renders`, created when missing; a symlink or anything but a directory there is refused.
fn renders_dir(dir: &Path) -> Result<PathBuf, String> {
    let renders = dir.join(RENDERS_DIR);
    match fs::symlink_metadata(&renders) {
        Ok(metadata) if metadata.is_dir() => Ok(renders),
        Ok(_) => Err(NOT_A_DIRECTORY.to_string()),
        Err(err) if err.kind() == ErrorKind::NotFound => fs::create_dir(&renders)
            .map(|()| renders.clone())
            .map_err(|err| format!("cannot create {}: {err}", renders.display())),
        Err(err) => Err(format!("cannot read {}: {err}", renders.display())),
    }
}
