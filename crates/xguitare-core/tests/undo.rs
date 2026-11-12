use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::{Value, json};
use xguitare_core::control::{
    EngineState, PeakMeters, Request, Response, SendLevel, TrackRef, TransportAction, handle,
};
use xguitare_core::engine::{AudioQueues, Command, EngineShared, engine_queues};
use xguitare_core::metronome::Metronome;
use xguitare_core::project::{Item, Project, Take};
use xguitare_core::project_control::Projects;
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BuildWarning, Config};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let dir = std::env::temp_dir().join(format!("xguitare-undo-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bank_dir = dir.join("bank");
    fs::create_dir_all(&bank_dir).unwrap();
    let config_path = dir.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let mut config = Config::load(Some(&config_path)).unwrap();
    config.bank_dir.clone_from(&bank_dir);
    config.projects_dir = dir.join("projects");
    let shared = Arc::new(EngineShared::default());
    shared.sample_rate.store(48_000, Ordering::Relaxed);
    let (queues, audio, tap) = engine_queues(64);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state = EngineState {
        bank: Bank::load_dir(&bank_dir).unwrap(),
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
    (state, audio, TempDir(dir))
}

fn ok((response, _): (Response, Vec<BuildWarning>)) -> Value {
    match response {
        Response::Ok(payload) => payload,
        Response::Err { error } => panic!("expected ok, got error {error}"),
    }
}

fn error((response, _): (Response, Vec<BuildWarning>)) -> String {
    match response {
        Response::Err { error } => error,
        Response::Ok(payload) => panic!("expected error, got {payload}"),
    }
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

fn take(file: &str, frames: u64) -> Take {
    Take {
        file: file.to_string(),
        frames,
    }
}

fn item(id: u64, position: u64, file: &str) -> Item {
    Item {
        id,
        position,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![take(file, 4800)],
    }
}

fn folder(state: &EngineState) -> PathBuf {
    state.config.projects_dir.join("song")
}

/// Opens a Project "Song" with Tracks Guitar, Bass and Keys and the Take files `takes/a.wav` and `takes/b.wav`; Item 1 has both Takes.
fn open_song(state: &mut EngineState) {
    ok(handle(
        state,
        Request::ProjectNew {
            name: "Song".to_string(),
        },
    ));
    for name in ["Guitar", "Bass", "Keys"] {
        ok(handle(
            state,
            Request::TrackAdd {
                name: Some(name.to_string()),
            },
        ));
    }
    ok(handle(state, Request::ProjectClose));
    let folder = folder(state);
    write_take(&folder.join("takes/a.wav"), &vec![0.25; 4800]);
    write_take(&folder.join("takes/b.wav"), &vec![0.5; 4800]);
    let mut project = Project::load(&folder).unwrap();
    let mut looped = item(1, 1000, "takes/a.wav");
    looped.takes.push(take("takes/b.wav", 4800));
    project.tracks[0].items = vec![looped, item(2, 3000, "takes/b.wav")];
    project.tracks[1].items = vec![item(3, 0, "takes/b.wav")];
    project.next_item_id = 4;
    project.save(&folder).unwrap();
    ok(handle(
        state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
}

fn items(state: &mut EngineState) -> Value {
    ok(handle(state, Request::Items))
}

fn status(state: &mut EngineState) -> Value {
    ok(handle(state, Request::Status))
}

/// The `undo` and `redo` labels of `status`.
fn labels(state: &mut EngineState) -> (Value, Value) {
    let status = status(state);
    (status["undo"].clone(), status["redo"].clone())
}

fn undo(state: &mut EngineState) -> Value {
    ok(handle(state, Request::ProjectUndo))
}

fn redo(state: &mut EngineState) -> Value {
    ok(handle(state, Request::ProjectRedo))
}

fn drain(audio: &mut AudioQueues) -> Vec<Command> {
    let mut commands = Vec::new();
    while let Ok(command) = audio.commands.pop() {
        commands.push(command);
    }
    commands
}

fn track_set(track: usize, volume: Option<f32>, arm: Option<bool>) -> Request {
    Request::TrackSet {
        track: TrackRef::Index(track),
        input: None,
        volume,
        pan: Some(-0.5),
        mute: Some(true),
        solo: None,
        arm,
        name: None,
        to_master: None,
    }
}

fn rack_add(track: usize) -> Request {
    Request::RackAdd {
        r#type: "drive".to_string(),
        after: None,
        at: None,
        track: Some(TrackRef::Index(track)),
        bus: None,
        master: false,
    }
}

fn trim(id: u64, start: u64) -> Request {
    Request::ItemTrim {
        id,
        start: Some(start),
        end: None,
    }
}

#[test]
fn each_item_edit_undoes_and_redoes_to_an_identical_project() {
    let (mut state, mut audio, _dir) = state("items");
    open_song(&mut state);
    let edits = [
        (
            Request::ItemMove {
                ids: vec![1, 3],
                delta: 500,
                track: Some(TrackRef::Index(1)),
            },
            "move 2 Items",
        ),
        (
            Request::ItemMove {
                ids: vec![2],
                delta: -100,
                track: None,
            },
            "move Item",
        ),
        (trim(2, 3200), "trim"),
        (
            Request::ItemSplit {
                ids: Some(vec![1]),
                at: 2000,
            },
            "split",
        ),
        (Request::ItemTake { id: 1, take: 1 }, "take 2"),
        (Request::ItemDelete { ids: vec![1, 2] }, "delete 2 Items"),
        (Request::ItemDelete { ids: vec![3] }, "delete Item"),
    ];
    for (edit, label) in edits {
        let before = items(&mut state);
        ok(handle(&mut state, edit));
        let after = items(&mut state);
        assert_ne!(after, before, "{label} changes the Project");
        assert_eq!(labels(&mut state), (json!(label), Value::Null));
        let revision = state.revision;
        assert_eq!(undo(&mut state), json!({ "label": label }));
        assert!(state.revision > revision);
        assert_eq!(items(&mut state), before, "{label} undone");
        assert_eq!(labels(&mut state).1, json!(label));
        assert_eq!(redo(&mut state), json!({ "label": label }));
        assert_eq!(items(&mut state), after, "{label} redone");
        drain(&mut audio);
    }
}

#[test]
fn a_new_edit_clears_redo() {
    let (mut state, _audio, _dir) = state("clears");
    open_song(&mut state);
    ok(handle(&mut state, trim(2, 3200)));
    undo(&mut state);
    assert_eq!(labels(&mut state), (Value::Null, json!("trim")));
    ok(handle(&mut state, Request::ItemDelete { ids: vec![3] }));
    assert_eq!(labels(&mut state), (json!("delete Item"), Value::Null));
    assert_eq!(
        error(handle(&mut state, Request::ProjectRedo)),
        "nothing to redo"
    );
}

#[test]
fn the_history_keeps_the_last_200_entries() {
    let (mut state, mut audio, _dir) = state("depth");
    open_song(&mut state);
    for start in 0..201 {
        ok(handle(&mut state, trim(2, 3001 + start)));
        drain(&mut audio);
    }
    for _ in 0..200 {
        undo(&mut state);
        drain(&mut audio);
    }
    assert_eq!(
        error(handle(&mut state, Request::ProjectUndo)),
        "nothing to undo"
    );
    assert_eq!(
        items(&mut state)["tracks"][0]["items"][1]["position"],
        3001,
        "the first trim stays"
    );
}

#[test]
fn mixer_rack_tempo_and_loop_requests_leave_the_history_alone() {
    let (mut state, _audio, _dir) = state("untouched");
    open_song(&mut state);
    ok(handle(&mut state, trim(2, 3200)));
    ok(handle(&mut state, Request::ItemDelete { ids: vec![3] }));
    undo(&mut state);
    let before = labels(&mut state);
    assert_eq!(before, (json!("trim"), json!("delete Item")));
    for request in [
        track_set(0, Some(-3.0), None),
        rack_add(0),
        Request::MasterSet {
            volume: SendLevel::Db(-1.0),
        },
        Request::Tempo {
            bpm: Some(90.0),
            meter: None,
        },
        Request::Loop {
            start: 0,
            end: 48_000,
            on: true,
        },
    ] {
        ok(handle(&mut state, request));
    }
    assert_eq!(labels(&mut state), before);
}

#[test]
fn undo_and_redo_are_refused_while_recording() {
    let (mut state, _audio, _dir) = state("recording");
    open_song(&mut state);
    ok(handle(&mut state, trim(2, 3200)));
    ok(handle(&mut state, Request::ItemDelete { ids: vec![3] }));
    undo(&mut state);
    ok(handle(&mut state, track_set(1, None, Some(true))));
    ok(handle(
        &mut state,
        Request::Transport {
            action: TransportAction::Record,
            position: None,
        },
    ));
    let before = items(&mut state);
    assert_eq!(
        error(handle(&mut state, Request::ProjectUndo)),
        "cannot undo while recording"
    );
    assert_eq!(
        error(handle(&mut state, Request::ProjectRedo)),
        "cannot redo while recording"
    );
    assert_eq!(items(&mut state), before);
    assert_eq!(labels(&mut state), (json!("trim"), json!("delete Item")));
}

#[test]
fn undo_and_redo_need_an_open_project_and_an_entry() {
    let (mut state, _audio, _dir) = state("empty");
    for (request, message) in [
        (Request::ProjectUndo, "no project open"),
        (Request::ProjectRedo, "no project open"),
    ] {
        assert_eq!(error(handle(&mut state, request)), message);
    }
    assert_eq!(labels(&mut state), (Value::Null, Value::Null));
    open_song(&mut state);
    assert_eq!(labels(&mut state), (Value::Null, Value::Null));
    assert_eq!(
        error(handle(&mut state, Request::ProjectUndo)),
        "nothing to undo"
    );
    assert_eq!(
        error(handle(&mut state, Request::ProjectRedo)),
        "nothing to redo"
    );
}

#[test]
fn closing_the_project_clears_the_history() {
    let (mut state, _audio, _dir) = state("close");
    open_song(&mut state);
    ok(handle(&mut state, trim(2, 3200)));
    ok(handle(&mut state, Request::ProjectClose));
    ok(handle(
        &mut state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
    assert_eq!(labels(&mut state), (Value::Null, Value::Null));
}

#[test]
fn item_ids_are_never_reused_after_undo_and_redo() {
    let (mut state, _audio, _dir) = state("ids");
    open_song(&mut state);
    let split = |state: &mut EngineState, at: u64| {
        ok(handle(
            state,
            Request::ItemSplit {
                ids: Some(vec![2]),
                at,
            },
        ))["items"][1]["id"]
            .clone()
    };
    assert_eq!(split(&mut state, 4000), 4);
    undo(&mut state);
    assert_eq!(split(&mut state, 4000), 5, "id 4 is not given again");
    undo(&mut state);
    redo(&mut state);
    assert_eq!(items(&mut state)["tracks"][0]["items"][2]["id"], 5);
    assert_eq!(split(&mut state, 3500), 6);
}
