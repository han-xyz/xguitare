use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde_json::{Value, json};
use xguitare_core::control::{ConnectionId, EngineState, PeakMeters, handle_line, open_connection};
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues};
use xguitare_core::metronome::Metronome;
use xguitare_core::project::{Item, PROJECT_FILE, Project, Take, Track};
use xguitare_core::project_control::{ProjectEvent, Projects, take_project_events};
use xguitare_core::render::RenderTask;
use xguitare_core::render_control::{RenderFinished, cancel_render, finished_render};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, Config};

const TAKE_FRAMES: usize = 4_800;
const HALF_SECOND: u32 = 24_000;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Session {
    state: EngineState,
    _audio: AudioQueues,
    connection: ConnectionId,
    dir: TempDir,
}

fn session(test: &str) -> Session {
    let dir =
        std::env::temp_dir().join(format!("xguitare-renderctl-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bank_dir = dir.join("bank");
    fs::create_dir_all(&bank_dir).unwrap();
    let config_path = dir.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let mut config = Config::load(Some(&config_path)).unwrap();
    config.bank_dir.clone_from(&bank_dir);
    config.projects_dir = dir.join("projects");
    let bank = Bank::load_dir(&bank_dir).unwrap();
    let shared = Arc::new(EngineShared::default());
    shared.sample_rate.store(48_000, Ordering::Relaxed);
    let (queues, audio, tap) = engine_queues(64);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let mut state = EngineState {
        bank,
        active: None,
        shared,
        queues,
        tuner,
        config,
        connections: Vec::new(),
        sample_rate: 48_000.0,
        max_frames: 128,
        revision: 1,
        modified: false,
        peaks: PeakMeters::default(),
        saved: HashMap::new(),
        projects: Projects::default(),
        metronome: Metronome::default(),
    };
    let connection = open_connection(&mut state);
    Session {
        state,
        _audio: audio,
        connection,
        dir: TempDir(dir),
    }
}

fn call(s: &mut Session, line: &str) -> Value {
    let (response, _) = handle_line(&mut s.state, None, s.connection, line);
    serde_json::from_str(&response).unwrap()
}

fn ok(s: &mut Session, line: &str) -> Value {
    let response = call(s, line);
    assert_eq!(response["ok"], true, "{line} answered {response}");
    response
}

fn error(s: &mut Session, line: &str) -> String {
    let response = call(s, line);
    assert_eq!(response["ok"], false, "{line} answered {response}");
    response["error"].as_str().unwrap().to_string()
}

fn song_dir(s: &Session) -> PathBuf {
    s.dir.0.join("projects/song")
}

fn write_take(path: &Path, samples: &[f32]) {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

/// Opens Project `Song` with one Track; with `items`, the Track has one 0.1 s Item at frame 0.
fn song(test: &str, items: bool) -> Session {
    let mut s = session(test);
    ok(&mut s, r#"{"cmd":"project_new","name":"Song"}"#);
    ok(&mut s, r#"{"cmd":"project_close"}"#);
    take_project_events(&mut s.state);
    let dir = song_dir(&s);
    let mut project = Project::load(&dir).unwrap().0;
    let mut track = Track::from_preset(
        "Guitar",
        &xguitare_core::Preset {
            name: "Guitar".to_string(),
            modules: Vec::new(),
        },
    );
    if items {
        write_take(&dir.join("takes/guitar-1.wav"), &vec![0.25; TAKE_FRAMES]);
        track.items = vec![Item {
            id: 0,
            position: 0,
            offset: 0,
            length: None,
            take: 0,
            takes: vec![Take {
                file: "takes/guitar-1.wav".to_string(),
                frames: TAKE_FRAMES as u64,
            }],
        }];
    }
    project.tracks.push(track);
    project.save(&dir).unwrap();
    assert!(dir.join(PROJECT_FILE).is_file());
    ok(&mut s, r#"{"cmd":"project_open","name":"Song"}"#);
    take_project_events(&mut s.state);
    s
}

fn render_tasks(s: &mut Session) -> Vec<RenderTask> {
    take_project_events(&mut s.state)
        .into_iter()
        .filter_map(|event| match event {
            ProjectEvent::Render(task) => Some(task),
            _ => None,
        })
        .collect()
}

/// Runs the Render the last request started, as the Daemon's thread does.
fn run_task(s: &mut Session) {
    let mut tasks = render_tasks(s);
    assert_eq!(tasks.len(), 1);
    tasks.remove(0).run();
}

fn render_status(s: &mut Session) -> Value {
    ok(s, r#"{"cmd":"status"}"#)["render"].clone()
}

fn revision(s: &mut Session) -> u64 {
    ok(s, r#"{"cmd":"status"}"#)["revision"].as_u64().unwrap()
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn refusals_are_verbatim() {
    let mut s = session("refusals");
    assert_eq!(error(&mut s, r#"{"cmd":"render"}"#), "no project open");
    assert_eq!(
        error(&mut s, r#"{"cmd":"render_cancel"}"#),
        "no render running"
    );
    let mut s = song("refusals-empty", false);
    assert_eq!(error(&mut s, r#"{"cmd":"render"}"#), "nothing to render");
    let mut s = song("refusals-items", true);
    assert_eq!(
        error(&mut s, r#"{"cmd":"render","range":"loop"}"#),
        "no loop region"
    );
    for name in ["", "a/b", "..", "a..b", ".hidden", "/etc/passwd"] {
        let line = json!({"cmd": "render", "name": name}).to_string();
        assert_eq!(
            error(&mut s, &line),
            format!("invalid render name '{name}'")
        );
    }
    for (name, shown) in [("a\nb", "a\\nb"), ("bell\u{7}", "bell\\u{7}")] {
        let line = json!({"cmd": "render", "name": name}).to_string();
        assert_eq!(
            error(&mut s, &line),
            format!("invalid render name '{shown}'")
        );
    }
    assert!(
        error(&mut s, r#"{"cmd":"render","bits":32}"#)
            .starts_with(r#"invalid request: bits must be 16, 24 or "32f""#)
    );
    assert!(
        !song_dir(&s).join("renders").exists()
            || fs::read_dir(song_dir(&s).join("renders"))
                .unwrap()
                .next()
                .is_none()
    );
    ok(&mut s, r#"{"cmd":"render"}"#);
    assert_eq!(
        error(&mut s, r#"{"cmd":"render"}"#),
        "a render is already running"
    );
}

#[test]
fn a_render_runs_reports_its_progress_and_never_overwrites() {
    let mut s = song("runs", true);
    assert_eq!(render_status(&mut s), Value::Null);
    let before = revision(&mut s);
    let renders = song_dir(&s).join("renders");
    let first = renders.join("song.wav");
    let started = ok(&mut s, r#"{"cmd":"render"}"#);
    assert_eq!(started["file"], path_text(&first));
    assert_eq!(revision(&mut s), before + 1);
    assert_eq!(
        render_status(&mut s),
        json!({"name": "song", "range": "project", "progress": 0.0, "state": "running", "file": path_text(&first)})
    );
    run_task(&mut s);
    assert!(
        matches!(finished_render(&mut s.state), Some(RenderFinished::Rendered(file)) if file == first)
    );
    assert!(finished_render(&mut s.state).is_none());
    assert_eq!(revision(&mut s), before + 2);
    let done = render_status(&mut s);
    assert_eq!(done["state"], "done");
    assert_eq!(done["progress"], 1.0);
    let reader = WavReader::open(&first).unwrap();
    assert_eq!(reader.spec().bits_per_sample, 24);
    assert_eq!(reader.duration(), TAKE_FRAMES as u32 + HALF_SECOND);

    let second = ok(&mut s, r#"{"cmd":"render"}"#);
    assert_eq!(second["file"], path_text(&renders.join("song-2.wav")));
    run_task(&mut s);
    fs::write(renders.join("song-3.wav"), "").unwrap();
    let fourth = ok(&mut s, r#"{"cmd":"render","bits":"32f"}"#);
    assert_eq!(fourth["file"], path_text(&renders.join("song-4.wav")));
    run_task(&mut s);
    let named = ok(&mut s, r#"{"cmd":"render","bits":16,"name":"Final Mix"}"#);
    assert_eq!(named["file"], path_text(&renders.join("Final Mix.wav")));
    run_task(&mut s);
    assert_eq!(render_status(&mut s)["name"], "Final Mix");
    let mut files: Vec<String> = fs::read_dir(&renders)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "Final Mix.wav",
            "song-2.wav",
            "song-3.wav",
            "song-4.wav",
            "song.wav"
        ]
    );
}

#[test]
fn the_unsaved_loop_region_is_rendered() {
    let mut s = song("unsaved-loop", true);
    ok(
        &mut s,
        r#"{"cmd":"loop","start":1000,"end":3000,"on":false}"#,
    );
    let started = ok(&mut s, r#"{"cmd":"render","range":"loop"}"#);
    run_task(&mut s);
    assert_eq!(render_status(&mut s)["range"], "loop");
    let file = PathBuf::from(started["file"].as_str().unwrap());
    assert_eq!(WavReader::open(file).unwrap().duration(), 2_000);
}

#[test]
fn a_cancelled_render_removes_its_part_file() {
    let mut s = song("cancel", true);
    let started = ok(&mut s, r#"{"cmd":"render"}"#);
    let before = revision(&mut s);
    ok(&mut s, r#"{"cmd":"render_cancel"}"#);
    run_task(&mut s);
    assert!(matches!(
        finished_render(&mut s.state),
        Some(RenderFinished::Cancelled)
    ));
    assert_eq!(revision(&mut s), before + 1);
    assert_eq!(render_status(&mut s)["state"], "cancelled");
    assert_eq!(
        error(&mut s, r#"{"cmd":"render_cancel"}"#),
        "no render running"
    );
    let file = PathBuf::from(started["file"].as_str().unwrap());
    assert!(!file.exists());
    let renders = song_dir(&s).join("renders");
    assert_eq!(
        fs::read_dir(renders).unwrap().count(),
        0,
        "no part file is left"
    );
}

#[test]
fn closing_the_project_or_stopping_cancels_the_render() {
    let mut s = song("close", true);
    ok(&mut s, r#"{"cmd":"render"}"#);
    let mut tasks = render_tasks(&mut s);
    ok(&mut s, r#"{"cmd":"project_close"}"#);
    tasks.remove(0).run();
    let render = render_status(&mut s);
    assert_eq!(render["state"], "cancelled", "{render}");

    let mut s = song("stop", true);
    ok(&mut s, r#"{"cmd":"render"}"#);
    let mut tasks = render_tasks(&mut s);
    cancel_render(&mut s.state);
    tasks.remove(0).run();
    assert_eq!(render_status(&mut s)["state"], "cancelled");
}

#[test]
fn a_new_or_opened_project_shows_no_render() {
    let mut s = song("render-cleared", true);
    ok(&mut s, r#"{"cmd":"render"}"#);
    run_task(&mut s);
    assert_eq!(render_status(&mut s)["state"], "done");
    ok(&mut s, r#"{"cmd":"project_close"}"#);
    ok(&mut s, r#"{"cmd":"project_open","name":"Song"}"#);
    assert_eq!(render_status(&mut s), Value::Null);

    ok(&mut s, r#"{"cmd":"render"}"#);
    run_task(&mut s);
    ok(&mut s, r#"{"cmd":"project_close"}"#);
    ok(&mut s, r#"{"cmd":"project_new","name":"Other"}"#);
    assert_eq!(render_status(&mut s), Value::Null);
}

#[test]
fn a_part_file_already_there_is_left_alone_and_the_next_name_is_used() {
    let mut s = song("part-exists", true);
    let started = ok(&mut s, r#"{"cmd":"render"}"#);
    let renders = song_dir(&s).join("renders");
    assert_eq!(started["file"], path_text(&renders.join("song.wav")));
    let outside = s.dir.0.join("outside.txt");
    fs::write(&outside, "keep").unwrap();
    std::os::unix::fs::symlink(&outside, renders.join("song.wav.part")).unwrap();
    run_task(&mut s);
    let second = renders.join("song-2.wav");
    assert!(
        matches!(finished_render(&mut s.state), Some(RenderFinished::Rendered(file)) if file == second)
    );
    assert_eq!(render_status(&mut s)["file"], path_text(&second));
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
    assert!(
        fs::symlink_metadata(renders.join("song.wav.part"))
            .unwrap()
            .is_symlink()
    );
    assert!(!renders.join("song.wav").exists());
    assert!(WavReader::open(&second).is_ok());
}

#[test]
fn a_file_created_while_rendering_is_never_replaced() {
    let mut s = song("final-exists", true);
    ok(&mut s, r#"{"cmd":"render"}"#);
    let renders = song_dir(&s).join("renders");
    fs::write(renders.join("song.wav"), "keep").unwrap();
    run_task(&mut s);
    let second = renders.join("song-2.wav");
    assert!(
        matches!(finished_render(&mut s.state), Some(RenderFinished::Rendered(file)) if file == second)
    );
    assert_eq!(
        fs::read_to_string(renders.join("song.wav")).unwrap(),
        "keep"
    );
    assert!(WavReader::open(&second).is_ok());
    assert!(!renders.join("song.wav.part").exists());
    assert!(!renders.join("song-2.wav.part").exists());
}

#[test]
fn renders_must_be_a_real_directory() {
    let mut s = song("renders-symlink", true);
    let outside = s.dir.0.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, song_dir(&s).join("renders")).unwrap();
    assert_eq!(
        error(&mut s, r#"{"cmd":"render"}"#),
        "renders is not a directory"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    let mut s = song("renders-file", true);
    fs::write(song_dir(&s).join("renders"), "").unwrap();
    assert_eq!(
        error(&mut s, r#"{"cmd":"render"}"#),
        "renders is not a directory"
    );
}
