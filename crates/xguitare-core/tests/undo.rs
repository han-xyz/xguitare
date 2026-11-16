use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::{Value, json};
use xguitare_core::arrangement::ArrangementCommand;
use xguitare_core::control::{
    EngineState, PeakMeters, Request, Response, SendLevel, TrackRef, TransportAction, handle,
};
use xguitare_core::engine::{AudioQueues, Command, EngineShared, engine_queues};
use xguitare_core::metronome::{Metronome, MetronomeSettings};
use xguitare_core::project::{Item, Project, Take};
use xguitare_core::project_control::{Projects, add_finished_takes};
use xguitare_core::take_capture::{FinishedTake, Latency, TakeSpan};
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
    let mut project = Project::load(&folder).unwrap().0;
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

/// The Tracks (mixer values and Sends) and the selected Track of `status`.
fn project_status(state: &mut EngineState) -> Value {
    let status = status(state);
    json!([status["tracks"], status["selected"]])
}

fn bus_add(name: &str) -> Request {
    Request::BusAdd {
        name: Some(name.to_string()),
    }
}

fn send_set(track: usize, bus: &str) -> Request {
    Request::SendSet {
        track: TrackRef::Index(track),
        bus: TrackRef::Name(bus.to_string()),
        level: Some(SendLevel::Db(-6.0)),
        pre: None,
    }
}

fn remove_track(name: &str) -> Request {
    Request::TrackRemove {
        track: TrackRef::Name(name.to_string()),
    }
}

fn trim(id: u64, start: u64) -> Request {
    Request::ItemTrim {
        id,
        start: Some(start),
        end: None,
    }
}

fn rack(state: &mut EngineState, track: usize) -> Value {
    ok(handle(
        state,
        Request::Get {
            param: None,
            track: Some(TrackRef::Index(track)),
            bus: None,
            master: false,
        },
    ))
}

fn fill_commands_to(state: &mut EngineState, free: usize) {
    let filler = || {
        Command::Metronome(MetronomeSettings {
            on: false,
            bpm: 120.0,
            meter: (4, 4),
            volume_db: -12.0,
            sample_rate: 48_000.0,
        })
    };
    while state.queues.commands.slots() > free {
        assert!(state.queues.commands.push(filler()).is_ok());
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
        let revision = state.revision;
        assert_eq!(redo(&mut state), json!({ "label": label }));
        assert!(state.revision > revision);
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

#[test]
fn a_removed_track_undone_comes_back_with_its_rack_mixer_and_items_in_its_place() {
    let (mut state, mut audio, _dir) = state("remove");
    open_song(&mut state);
    ok(handle(&mut state, rack_add(1)));
    ok(handle(
        &mut state,
        Request::Set {
            param: "drive.drive".to_string(),
            value: 7.5,
            track: Some(TrackRef::Index(1)),
            bus: None,
            master: false,
        },
    ));
    ok(handle(&mut state, track_set(1, Some(-6.0), None)));
    ok(handle(&mut state, bus_add("Verb")));
    ok(handle(&mut state, send_set(1, "Verb")));
    ok(handle(
        &mut state,
        Request::TrackSelect {
            track: TrackRef::Index(1),
        },
    ));
    let before = (items(&mut state), project_status(&mut state));
    let bass_rack = rack(&mut state, 1);
    drain(&mut audio);
    ok(handle(
        &mut state,
        Request::TrackRemove {
            track: TrackRef::Name("Bass".to_string()),
        },
    ));
    let after = items(&mut state);
    assert_eq!(labels(&mut state).0, json!("remove Track Bass"));
    assert_eq!(undo(&mut state), json!({"label": "remove Track Bass"}));
    assert_eq!((items(&mut state), project_status(&mut state)), before);
    assert_eq!(rack(&mut state, 1), bass_rack);
    assert_eq!(redo(&mut state), json!({"label": "remove Track Bass"}));
    assert_eq!(items(&mut state), after);
    undo(&mut state);
    assert_eq!(items(&mut state), before.0);
}

#[test]
fn an_added_track_undone_goes_and_redone_comes_back_as_it_was() {
    let (mut state, _audio, _dir) = state("add");
    open_song(&mut state);
    let before = items(&mut state);
    ok(handle(
        &mut state,
        Request::TrackAdd {
            name: Some("Drums".to_string()),
        },
    ));
    ok(handle(&mut state, rack_add(3)));
    let after = (items(&mut state), rack(&mut state, 3));
    assert_eq!(undo(&mut state), json!({"label": "add Track Drums"}));
    assert_eq!(items(&mut state), before);
    assert_eq!(redo(&mut state), json!({"label": "add Track Drums"}));
    assert_eq!(
        (items(&mut state), rack(&mut state, 3)),
        after,
        "the Track comes back with the Rack it had when it was undone"
    );
}

#[test]
fn a_busy_engine_leaves_the_history_and_the_project_unchanged() {
    let (mut state, mut audio, _dir) = state("busy");
    open_song(&mut state);
    ok(handle(&mut state, Request::ItemDelete { ids: vec![1, 3] }));
    ok(handle(
        &mut state,
        Request::TrackRemove {
            track: TrackRef::Index(2),
        },
    ));
    undo(&mut state);
    drain(&mut audio);
    let before = (items(&mut state), labels(&mut state));
    fill_commands_to(&mut state, 0);
    let revision = state.revision;
    for request in [Request::ProjectUndo, Request::ProjectRedo] {
        assert_eq!(error(handle(&mut state, request)), "engine busy");
    }
    fill_commands_to(&mut state, 1);
    assert_eq!(
        error(handle(&mut state, Request::ProjectUndo)),
        "engine busy",
        "the delete changed two Tracks and one slot is free"
    );
    assert_eq!(state.revision, revision);
    assert_eq!((items(&mut state), labels(&mut state)), before);
    assert!(
        !drain(&mut audio).iter().any(|command| matches!(
            command,
            Command::Arrangement(ArrangementCommand::SetTrackItems { .. })
                | Command::SwapArrangement(_)
        )),
        "nothing reached the Engine"
    );
}

#[test]
fn a_recorded_take_undone_keeps_its_file_and_redone_plays_again() {
    let (mut state, mut audio, _dir) = state("record");
    open_song(&mut state);
    let before = items(&mut state);
    let keys_id = before["tracks"][2]["id"].as_u64().unwrap();
    let file = "takes/keys-1.wav";
    write_take(&folder(&state).join(file), &vec![0.75; 2400]);
    let finished = FinishedTake {
        track_id: keys_id,
        file: file.to_string(),
        span: TakeSpan {
            start: 9600,
            frames: 2400,
            loop_take: false,
        },
    };
    assert_eq!(
        add_finished_takes(&mut state, &[finished], |_| Latency::default()),
        Vec::<String>::new()
    );
    let after = items(&mut state);
    assert_eq!(after["tracks"][2]["items"][0]["takes"], json!([file]));
    assert_eq!(labels(&mut state), (json!("record Keys"), Value::Null));
    drain(&mut audio);
    assert_eq!(undo(&mut state), json!({"label": "record Keys"}));
    assert_eq!(items(&mut state), before);
    assert!(folder(&state).join(file).is_file(), "the Take file is kept");
    assert_eq!(redo(&mut state), json!({"label": "record Keys"}));
    assert_eq!(items(&mut state), after);
    let swapped: Vec<Vec<f32>> = drain(&mut audio)
        .into_iter()
        .filter_map(|command| match command {
            Command::Arrangement(ArrangementCommand::SetTrackItems { track: 2, items }) => {
                Some(items.iter().map(|span| span.samples[0]).collect())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        swapped,
        [vec![], vec![0.75]],
        "undone to nothing, redone to the Take"
    );
}

/// Capture and playback latency of 64 frames each: a Take recorded from frame 0 skips its first 128 frames.
const LATENCY_128: Latency = Latency {
    capture: 64,
    playback: 64,
};

/// Adds a Take of `frames` recorded on Keys from frame 0 with [`LATENCY_128`]; the failures and the Take's file.
fn record_keys_from_0(state: &mut EngineState, frames: usize) -> (Vec<String>, &'static str) {
    let keys_id = items(state)["tracks"][2]["id"].as_u64().unwrap();
    let file = "takes/keys-1.wav";
    write_take(&folder(state).join(file), &vec![0.75; frames]);
    let finished = FinishedTake {
        track_id: keys_id,
        file: file.to_string(),
        span: TakeSpan {
            start: 0,
            frames: frames as u64,
            loop_take: false,
        },
    };
    (
        add_finished_takes(state, &[finished], |_| LATENCY_128),
        file,
    )
}

#[test]
fn a_take_no_longer_than_its_latency_compensation_is_kept_as_a_file_but_not_placed() {
    let (mut state, _audio, _dir) = state("record-short");
    open_song(&mut state);
    let before = items(&mut state);
    let (failures, file) = record_keys_from_0(&mut state, 128);
    assert_eq!(failures, Vec::<String>::new());
    assert_eq!(items(&mut state), before);
    assert_eq!(labels(&mut state), (Value::Null, Value::Null));
    ok(handle(&mut state, Request::ProjectSave));
    let unused = ok(handle(&mut state, Request::TakesUnused));
    assert_eq!(unused["files"][0]["file"], file);
}

#[test]
fn a_take_one_frame_longer_than_its_latency_compensation_is_placed() {
    let (mut state, _audio, _dir) = state("record-just-long-enough");
    open_song(&mut state);
    let (failures, file) = record_keys_from_0(&mut state, 129);
    assert_eq!(failures, Vec::<String>::new());
    let placed = &items(&mut state)["tracks"][2]["items"][0];
    assert_eq!(
        (&placed["takes"], &placed["offset"], &placed["length"]),
        (&json!([file]), &json!(128), &json!(1))
    );
    ok(handle(&mut state, Request::ProjectSave));
}

#[test]
fn an_undone_track_removal_drops_its_sends_to_a_bus_removed_since() {
    let (mut state, _audio, _dir) = state("send-gone");
    open_song(&mut state);
    ok(handle(&mut state, bus_add("Verb")));
    ok(handle(&mut state, bus_add("Delay")));
    ok(handle(&mut state, send_set(1, "Verb")));
    ok(handle(&mut state, send_set(1, "Delay")));
    ok(handle(&mut state, trim(3, 100)));
    ok(handle(&mut state, remove_track("Bass")));
    ok(handle(
        &mut state,
        Request::BusRemove {
            bus: TrackRef::Name("Verb".to_string()),
        },
    ));
    assert_eq!(undo(&mut state), json!({"label": "remove Track Bass"}));
    let sends = &status(&mut state)["tracks"][1]["sends"];
    assert_eq!(
        sends,
        &json!([{"bus": 0, "level_db": -6.0, "pre": false}]),
        "only the Send to Delay, now Bus 0, is back"
    );
    assert_eq!(undo(&mut state), json!({"label": "trim"}));
    assert_eq!(items(&mut state)["tracks"][1]["items"][0]["position"], 0);
}

#[test]
fn a_restored_track_whose_name_is_taken_comes_back_under_a_free_one() {
    let (mut state, _audio, _dir) = state("name-taken");
    open_song(&mut state);
    ok(handle(&mut state, trim(3, 100)));
    ok(handle(&mut state, remove_track("Bass")));
    let mut rename = track_set(1, None, None);
    if let Request::TrackSet {
        name, pan, mute, ..
    } = &mut rename
    {
        *name = Some("Bass".to_string());
        *pan = None;
        *mute = None;
    }
    ok(handle(&mut state, rename));
    assert_eq!(undo(&mut state), json!({"label": "remove Track Bass 2"}));
    let names: Vec<Value> = items(&mut state)["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| track["name"].clone())
        .collect();
    assert_eq!(names, [json!("Guitar"), json!("Bass 2"), json!("Bass")]);
    assert_eq!(labels(&mut state).1, json!("remove Track Bass 2"));
    assert_eq!(undo(&mut state), json!({"label": "trim"}));
    assert_eq!(
        items(&mut state)["tracks"][1]["items"][0]["position"],
        0,
        "the Items entry finds the Track by its id"
    );
}

#[test]
fn a_busy_engine_leaves_an_undone_track_removal_and_the_loaded_takes_alone() {
    let (mut state, mut audio, _dir) = state("busy-restore");
    open_song(&mut state);
    ok(handle(&mut state, remove_track("Bass")));
    drain(&mut audio);
    let before = (
        items(&mut state),
        labels(&mut state),
        status(&mut state)["loaded_audio_bytes"].clone(),
    );
    fill_commands_to(&mut state, 0);
    assert_eq!(
        error(handle(&mut state, Request::ProjectUndo)),
        "engine busy"
    );
    assert_eq!(
        (
            items(&mut state),
            labels(&mut state),
            status(&mut state)["loaded_audio_bytes"].clone()
        ),
        before
    );
}

fn finished(track_id: u64, file: &str, start: u64) -> FinishedTake {
    FinishedTake {
        track_id,
        file: file.to_string(),
        span: TakeSpan {
            start,
            frames: 2400,
            loop_take: false,
        },
    }
}

#[test]
fn undoing_an_item_edit_keeps_mixer_and_rack_changes_made_after_it() {
    let (mut state, _audio, _dir) = state("keeps-mixer");
    open_song(&mut state);
    ok(handle(&mut state, trim(2, 3200)));
    ok(handle(&mut state, track_set(0, Some(-3.0), None)));
    ok(handle(&mut state, rack_add(0)));
    let rack_after = rack(&mut state, 0);
    undo(&mut state);
    assert_eq!(status(&mut state)["tracks"][0]["volume_db"], -3.0);
    assert_eq!(rack(&mut state, 0), rack_after);
}

#[test]
fn an_item_edit_on_another_track_after_a_removal_undoes_in_order() {
    let (mut state, _audio, _dir) = state("interleaved");
    open_song(&mut state);
    let before = items(&mut state);
    ok(handle(&mut state, remove_track("Keys")));
    ok(handle(&mut state, trim(2, 3200)));
    assert_eq!(undo(&mut state), json!({"label": "trim"}));
    assert_eq!(undo(&mut state), json!({"label": "remove Track Keys"}));
    assert_eq!(items(&mut state), before);
}

#[test]
fn takes_added_after_an_undo_clear_redo_and_each_is_its_own_entry() {
    let (mut state, mut audio, _dir) = state("two-takes");
    open_song(&mut state);
    let before = items(&mut state);
    let ids: Vec<u64> = before["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| track["id"].as_u64().unwrap())
        .collect();
    for file in ["takes/keys-1.wav", "takes/bass-1.wav"] {
        write_take(&folder(&state).join(file), &vec![0.75; 2400]);
    }
    ok(handle(&mut state, trim(2, 3200)));
    undo(&mut state);
    let trimmed_back = items(&mut state);
    assert_eq!(
        add_finished_takes(
            &mut state,
            &[
                finished(ids[2], "takes/keys-1.wav", 9600),
                finished(ids[1], "takes/bass-1.wav", 9600),
            ],
            |_| Latency::default()
        ),
        Vec::<String>::new()
    );
    drain(&mut audio);
    assert_eq!(labels(&mut state), (json!("record Bass"), Value::Null));
    assert_eq!(undo(&mut state), json!({"label": "record Bass"}));
    assert_eq!(
        items(&mut state)["tracks"][1]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        items(&mut state)["tracks"][2]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(undo(&mut state), json!({"label": "record Keys"}));
    assert_eq!(items(&mut state), trimmed_back);
    assert_eq!(
        error(handle(&mut state, Request::ProjectUndo)),
        "nothing to undo",
        "the trim was undone before the Takes came"
    );
}
