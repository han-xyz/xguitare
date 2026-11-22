use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::{Value, json};
use xguitare_core::control::{
    EngineState, follow_sample_rate, handle, handle_line, handle_request, handle_with_ports,
    open_connection, reprepare,
};
use xguitare_core::engine::{AudioQueues, Command, EngineShared, engine_queues, raise_peak};
use xguitare_core::metronome::MetronomeSettings;
use xguitare_core::ports::{AudioPorts, AvailablePort};
use xguitare_core::project::{AUTOSAVE_FILE, Item, PROJECT_FILE, Project, Take};
use xguitare_core::project_control::{
    add_finished_takes, autosave_if_due, begin_project_read, finish_closed, finish_stopped,
    read_project, take_project_events,
};
use xguitare_core::project_state::{ProjectEvent, ReadProject};
use xguitare_core::take_capture::{FinishedTake, Latency, TakeSpan};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BankEntry, BuildWarning, Config, Ports};
use xguitare_protocol::{Position, Request, Response, TrackRef, TransportAction};

const CAPTURE: &str = "card:capture_1";

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct FakePorts;

impl AudioPorts for FakePorts {
    fn audio_ports(&self) -> (Vec<AvailablePort>, Vec<AvailablePort>) {
        let port = |name: &str| AvailablePort {
            name: name.to_string(),
            is_physical: true,
        };
        (vec![port(CAPTURE)], vec![port("card:playback_1")])
    }

    fn own_ports(&self) -> Ports {
        Ports {
            input: "xguitare:in".to_string(),
            output_left: "xguitare:out_l".to_string(),
            output_right: "xguitare:out_r".to_string(),
        }
    }

    fn connect(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }

    fn disconnect(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let dir = std::env::temp_dir().join(format!("xguitare-projctl-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bank_dir = dir.join("bank");
    fs::create_dir_all(&bank_dir).unwrap();
    fs::write(
        bank_dir.join("01-clean.toml"),
        "name = \"Clean\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = -3\n",
    )
    .unwrap();
    fs::write(
        bank_dir.join("02-amp.toml"),
        "name = \"Amp\"\n[[module]]\ntype = \"gain\"\n[[module]]\ntype = \"drive\"\n",
    )
    .unwrap();
    let config_path = dir.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let mut config = Config::load(Some(&config_path)).unwrap();
    config.bank_dir.clone_from(&bank_dir);
    config.projects_dir = dir.join("projects");
    let bank = Bank::load_dir(&bank_dir).unwrap();
    let BankEntry::Valid { file, preset } = &bank.entries[0] else {
        panic!("01-clean.toml should be valid");
    };
    let active = Some((file.clone(), preset.clone()));
    let shared = Arc::new(EngineShared::default());
    shared.sample_rate.store(48_000, Ordering::Relaxed);
    let (queues, audio, tap) = engine_queues(64);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state =
        EngineState::new(bank, shared, queues, tuner, config, 48_000.0, 128).with_active(active);
    (state, audio, TempDir(dir))
}

fn ok((response, _): (Response, Vec<BuildWarning>)) -> Value {
    match response {
        Response::Ok(payload) => payload,
        Response::Err { error, .. } => panic!("expected ok, got error {error}"),
    }
}

fn error((response, _): (Response, Vec<BuildWarning>)) -> String {
    match response {
        Response::Err { error, .. } => error,
        Response::Ok(payload) => panic!("expected error, got {payload}"),
    }
}

fn new_project(state: &mut EngineState, name: &str) -> Value {
    ok(handle(
        state,
        Request::ProjectNew {
            name: name.to_string(),
        },
    ))
}

fn add_track(state: &mut EngineState, name: &str) -> Value {
    ok(handle(
        state,
        Request::TrackAdd {
            name: Some(name.to_string()),
        },
    ))
}

fn index(track: usize) -> TrackRef {
    TrackRef::Index(track)
}

fn named(track: &str) -> TrackRef {
    TrackRef::Name(track.to_string())
}

fn track_set(track: TrackRef) -> Request {
    Request::TrackSet {
        track,
        input: None,
        volume: None,
        pan: None,
        mute: None,
        solo: None,
        arm: None,
        name: None,
        to_master: None,
    }
}

fn arm(state: &mut EngineState, track: usize) {
    let mut request = track_set(index(track));
    if let Request::TrackSet { arm, .. } = &mut request {
        *arm = Some(true);
    }
    ok(handle(state, request));
}

fn transport(action: TransportAction, position: Option<Position>) -> Request {
    Request::Transport { action, position }
}

fn status(state: &mut EngineState) -> Value {
    ok(handle(state, Request::Status))
}

/// Closes the Project as the Daemon does when no Take was still being written.
fn close(state: &mut EngineState) {
    ok(handle(state, Request::ProjectClose));
    for event in take_project_events(state) {
        if let ProjectEvent::Closed(closed) = event {
            finish_closed(state, closed, &[], |_| Latency::default());
        }
    }
}

fn take(track_id: u64, file: &str, start: u64, frames: u64) -> FinishedTake {
    FinishedTake {
        track_id,
        file: file.to_string(),
        span: TakeSpan {
            start,
            frames,
            loop_take: false,
        },
    }
}

/// The Track id each Track's reader carries in the last `Readers` event.
fn reader_ids(state: &mut EngineState) -> Vec<(String, u64)> {
    take_project_events(state)
        .into_iter()
        .filter_map(|event| match event {
            ProjectEvent::Readers(readers) => Some(
                readers
                    .into_iter()
                    .map(|reader| (reader.name, reader.track_id))
                    .collect(),
            ),
            _ => None,
        })
        .next_back()
        .unwrap_or_default()
}

fn saved(state: &EngineState, folder: &str) -> Project {
    Project::load(&state.config.projects_dir.join(folder))
        .unwrap()
        .0
}

fn drain_commands(audio: &mut AudioQueues) -> Vec<Command> {
    let mut commands = Vec::new();
    while let Ok(command) = audio.commands.pop() {
        commands.push(command);
    }
    commands
}

fn arrangement_commands(audio: &mut AudioQueues) -> Vec<String> {
    drain_commands(audio)
        .into_iter()
        .filter_map(|command| match command {
            Command::Arrangement(command) => Some(format!("{command:?}")),
            _ => None,
        })
        .collect()
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

#[test]
fn project_new_creates_the_folder_at_the_engine_rate_and_runs_it() {
    let (mut state, mut audio, _dir) = state("new");
    state.sample_rate = 44_100.0;
    let payload = new_project(&mut state, "My Song");
    let folder = state.config.projects_dir.join("my-song");
    assert_eq!(
        payload,
        json!({"name": "My Song", "path": folder.display().to_string()})
    );
    assert_eq!(saved(&state, "my-song").sample_rate, 44_100);
    assert!(folder.join("takes").is_dir());
    assert!(
        drain_commands(&mut audio)
            .iter()
            .any(|command| matches!(command, Command::SwapArrangement(_)))
    );
    assert_eq!(status(&mut state)["project"], "My Song");
    assert_eq!(
        error(handle(
            &mut state,
            Request::ProjectNew {
                name: "Other".to_string()
            }
        )),
        "a project is open; close it first"
    );
}

#[test]
fn project_names_stay_inside_projects_dir() {
    let (mut state, _audio, dir) = state("names");
    new_project(&mut state, "../../escape");
    assert!(state.config.projects_dir.join("escape").is_dir());
    assert!(!dir.0.join("escape").exists());
    close(&mut state);
    for name in ["..", "../bank", "/etc"] {
        assert_eq!(
            error(handle(
                &mut state,
                Request::ProjectOpen {
                    name: name.to_string(),
                    restore: false
                }
            )),
            format!("no project '{name}'")
        );
    }
    assert_eq!(
        error(handle(
            &mut state,
            Request::ProjectNew {
                name: "../..".to_string()
            }
        )),
        "project name must not be empty"
    );
}

#[test]
fn project_errors_without_an_open_project() {
    let (mut state, _audio, _dir) = state("none");
    for request in [
        Request::ProjectSave,
        Request::ProjectClose,
        Request::TrackAdd { name: None },
        Request::TrackRemove { track: index(0) },
        track_set(index(0)),
        Request::TrackSelect { track: index(0) },
        transport(TransportAction::Play, None),
        Request::Loop {
            start: 0,
            end: 1,
            on: true,
        },
        Request::Tempo {
            bpm: Some(100.0),
            meter: None,
        },
        Request::Click {
            on: Some(true),
            count_in: None,
        },
        Request::Set {
            param: "gain.level".to_string(),
            value: 0.0,
            track: Some(index(0)),
            bus: None,
            master: false,
        },
    ] {
        assert_eq!(error(handle(&mut state, request)), "no project open");
    }
}

#[test]
fn project_open_refuses_a_rate_other_than_the_engine_rate() {
    let (mut state, _audio, _dir) = state("rate");
    new_project(&mut state, "Song");
    close(&mut state);
    state.sample_rate = 96_000.0;
    assert_eq!(
        error(handle(
            &mut state,
            Request::ProjectOpen {
                name: "Song".to_string(),
                restore: false
            }
        )),
        "project rate 48000 Hz differs from the engine rate 96000 Hz"
    );
    assert_eq!(status(&mut state)["project"], Value::Null);
}

#[test]
fn project_open_refuses_a_project_from_a_newer_format() {
    let (mut state, _audio, _dir) = state("newer-format");
    new_project(&mut state, "Song");
    close(&mut state);
    let file = state.config.projects_dir.join("song").join(PROJECT_FILE);
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, text.replacen("format = 1", "format = 2", 1)).unwrap();
    assert_eq!(
        error(handle(
            &mut state,
            Request::ProjectOpen {
                name: "Song".to_string(),
                restore: false
            }
        )),
        "project format 2 is newer than this xguitare (reads up to 1)"
    );
    assert_eq!(status(&mut state)["project"], Value::Null);
}

#[test]
fn project_open_reports_a_newer_autosave_and_restores_it() {
    let (mut state, _audio, _dir) = state("restore");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    close(&mut state);
    let folder = state.config.projects_dir.join("song");
    let mut autosave = Project::load(&folder).unwrap().0;
    let mut bass = autosave.tracks[0].clone();
    bass.name = "Bass".to_string();
    autosave.tracks.push(bass);
    let open = |restore| Request::ProjectOpen {
        name: "Song".to_string(),
        restore,
    };
    std::thread::sleep(Duration::from_millis(20));
    autosave.save_autosave(&folder).unwrap();
    let opened = ok(handle(&mut state, open(false)));
    assert_eq!(opened["autosave_newer"], true);
    assert_eq!(opened["restored"], false);
    assert_eq!(status(&mut state)["tracks"].as_array().unwrap().len(), 1);
    close(&mut state);
    std::thread::sleep(Duration::from_millis(20));
    autosave.save_autosave(&folder).unwrap();
    let restored = ok(handle(&mut state, open(true)));
    assert_eq!(restored["restored"], true);
    assert_eq!(status(&mut state)["tracks"].as_array().unwrap().len(), 2);
}

#[test]
fn project_open_answers_with_the_load_warnings() {
    let (mut state, _audio, _dir) = state("load-warnings");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    close(&mut state);
    let folder = state.config.projects_dir.join("song");
    let open = |restore| Request::ProjectOpen {
        name: "Song".to_string(),
        restore,
    };
    let opened = ok(handle(&mut state, open(false)));
    assert_eq!(opened["warnings"], json!([]));
    close(&mut state);
    let mut project = Project::load(&folder).unwrap().0;
    project.tracks[0].items = vec![Item {
        id: 1,
        position: 0,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![Take {
            file: "takes/gone.wav".to_string(),
            frames: 10,
        }],
    }];
    project.next_item_id = 2;
    project.save(&folder).unwrap();
    project.save_autosave(&folder).unwrap();
    for file in [PROJECT_FILE, AUTOSAVE_FILE] {
        let path = folder.join(file);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("frames = 10", "frames = 0")).unwrap();
    }
    for restore in [false, true] {
        let opened = ok(handle(&mut state, open(restore)));
        assert_eq!(
            opened["warnings"],
            json!([
                "track 'Guitar': dropped item 1 (takes/gone.wav): starts past the end of its take"
            ]),
            "restore {restore}"
        );
        close(&mut state);
    }
}

#[test]
fn autosave_waits_for_a_change_and_the_interval() {
    let (mut state, _audio, _dir) = state("autosave");
    assert!(autosave_if_due(&mut state, Instant::now()).is_none());
    new_project(&mut state, "Song");
    let later = Instant::now() + Duration::from_secs(61);
    assert!(autosave_if_due(&mut state, later).is_none());
    add_track(&mut state, "Guitar");
    assert!(autosave_if_due(&mut state, Instant::now()).is_none());
    assert!(autosave_if_due(&mut state, later).unwrap().is_ok());
    assert!(autosave_if_due(&mut state, later).is_none());
    let folder = state.config.projects_dir.join("song");
    assert_eq!(Project::load_autosave(&folder).unwrap().0.tracks.len(), 1);
    assert_eq!(saved(&state, "song").tracks.len(), 0);
}

#[test]
fn a_transport_stop_autosaves_a_changed_project() {
    let (mut state, _audio, _dir) = state("autosave-stop");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    let folder = state.config.projects_dir.join("song");
    assert!(Project::load_autosave(&folder).is_err());
    ok(handle(&mut state, transport(TransportAction::Stop, None)));
    assert_eq!(Project::load_autosave(&folder).unwrap().0.tracks.len(), 1);
}

#[test]
fn project_save_and_close_write_project_toml() {
    let (mut state, mut audio, _dir) = state("save");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    let folder = state.config.projects_dir.join("song");
    assert_eq!(
        ok(handle(&mut state, Request::ProjectSave)),
        json!({"path": folder.join(PROJECT_FILE).display().to_string()})
    );
    assert_eq!(saved(&state, "song").tracks[0].name, "Guitar");
    add_track(&mut state, "Bass");
    drain_commands(&mut audio);
    assert_eq!(
        ok(handle(&mut state, Request::ProjectClose)),
        json!({"name": "Song", "path": folder.display().to_string()})
    );
    assert_eq!(saved(&state, "song").tracks.len(), 2);
    assert!(
        drain_commands(&mut audio)
            .iter()
            .any(|command| matches!(command, Command::CloseArrangement))
    );
    assert_eq!(status(&mut state)["project"], Value::Null);
    assert_eq!(status(&mut state)["transport"], Value::Null);
}

#[test]
fn project_list_names_each_project_and_marks_broken_ones() {
    let (mut state, _audio, _dir) = state("list");
    assert_eq!(
        ok(handle(&mut state, Request::ProjectList)),
        json!({"projects": []})
    );
    new_project(&mut state, "B Song");
    close(&mut state);
    new_project(&mut state, "A Song");
    close(&mut state);
    let broken = state.config.projects_dir.join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join(PROJECT_FILE), "name = ").unwrap();
    let listed = ok(handle(&mut state, Request::ProjectList));
    let projects = listed["projects"].as_array().unwrap();
    assert_eq!(
        projects[0],
        json!({"name": "A Song", "folder": "a-song", "valid": true})
    );
    assert_eq!(
        projects[1],
        json!({"name": "B Song", "folder": "b-song", "valid": true})
    );
    assert_eq!(projects[2]["folder"], "broken");
    assert_eq!(projects[2]["valid"], false);
    assert!(
        projects[2]["error"].as_str().unwrap().contains("line 1:8"),
        "{}",
        projects[2]
    );
}

#[test]
fn track_add_names_selects_and_limits_tracks() {
    let (mut state, _audio, _dir) = state("track-add");
    new_project(&mut state, "Song");
    assert_eq!(
        ok(handle(&mut state, Request::TrackAdd { name: None })),
        json!({"index": 0, "name": "Track 1"})
    );
    assert_eq!(
        add_track(&mut state, "Bass"),
        json!({"index": 1, "name": "Bass"})
    );
    assert_eq!(status(&mut state)["selected"], 1);
    assert_eq!(
        error(handle(
            &mut state,
            Request::TrackAdd {
                name: Some("bass".to_string())
            }
        )),
        "a track named 'bass' already exists"
    );
    assert_eq!(
        error(handle(
            &mut state,
            Request::TrackAdd {
                name: Some(" \u{7}".to_string())
            }
        )),
        "track name must not be empty"
    );
    for n in 3..=16 {
        add_track(&mut state, &format!("T{n}"));
    }
    assert_eq!(
        error(handle(&mut state, Request::TrackAdd { name: None })),
        "a project has at most 16 tracks"
    );
}

#[test]
fn a_new_track_copies_the_active_presets_modules() {
    let (mut state, _audio, _dir) = state("track-modules");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    ok(handle(&mut state, Request::ProjectSave));
    let modules = &saved(&state, "song").tracks[0].modules;
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].type_name, "gain");
}

#[test]
fn track_remove_and_select_resolve_index_or_name() {
    let (mut state, _audio, _dir) = state("track-remove");
    new_project(&mut state, "Song");
    assert_eq!(
        error(handle(
            &mut state,
            Request::Set {
                param: "gain.level".to_string(),
                value: 0.0,
                track: None,
                bus: None,
                master: false,
            }
        )),
        "the project has no tracks"
    );
    add_track(&mut state, "Guitar");
    add_track(&mut state, "Bass");
    add_track(&mut state, "Keys");
    assert_eq!(
        ok(handle(
            &mut state,
            Request::TrackSelect {
                track: named("Bass")
            }
        )),
        json!({"index": 1, "name": "Bass"})
    );
    assert_eq!(
        ok(handle(&mut state, Request::TrackRemove { track: index(0) })),
        json!({"index": 0, "name": "Guitar"})
    );
    assert_eq!(status(&mut state)["selected"], 0);
    assert_eq!(
        error(handle(&mut state, Request::TrackRemove { track: index(2) })),
        "no track '2'"
    );
    assert_eq!(
        error(handle(
            &mut state,
            Request::TrackSelect {
                track: named("Drums")
            }
        )),
        "no track 'Drums'"
    );
}

#[test]
fn track_set_validates_every_value_before_changing_anything() {
    let (mut state, mut audio, _dir) = state("track-set");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    add_track(&mut state, "Bass");
    drain_commands(&mut audio);
    let mut request = track_set(index(0));
    if let Request::TrackSet {
        volume, mute, pan, ..
    } = &mut request
    {
        *volume = Some(7.0);
        *mute = Some(true);
        *pan = Some(0.5);
    }
    assert_eq!(
        error(handle(&mut state, request.clone())),
        "track 'Guitar' volume must be between -60 and 6 dB"
    );
    if let Request::TrackSet { volume, pan, .. } = &mut request {
        *volume = Some(-6.0);
        *pan = Some(-1.5);
    }
    assert_eq!(
        error(handle(&mut state, request.clone())),
        "track 'Guitar' pan must be between -1 and 1"
    );
    assert_eq!(status(&mut state)["tracks"][0]["mute"], false);
    if let Request::TrackSet { pan, name, .. } = &mut request {
        *pan = Some(-0.5);
        *name = Some("BASS".to_string());
    }
    assert_eq!(
        error(handle(&mut state, request.clone())),
        "a track named 'BASS' already exists"
    );
    if let Request::TrackSet { name, .. } = &mut request {
        *name = None;
    }
    assert_eq!(
        ok(handle(&mut state, request)),
        json!({"index": 0, "name": "Guitar", "input": null, "volume_db": -6.0, "pan": -0.5, "mute": true, "solo": false, "arm": false, "to_master": true})
    );
    assert_eq!(
        arrangement_commands(&mut audio),
        [
            "Volume { track: 0, db: -6.0 }",
            "Pan { track: 0, pan: -0.5 }",
            "Mute { track: 0, on: true }",
        ]
    );
}

#[test]
fn track_input_must_be_a_capture_port_and_is_reconnected() {
    let (mut state, _audio, _dir) = state("track-input");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    take_project_events(&mut state);
    let with_input = |input: &str| {
        let mut request = track_set(index(0));
        if let Request::TrackSet { input: wanted, .. } = &mut request {
            *wanted = Some(input.to_string());
        }
        request
    };
    assert_eq!(
        error(handle_with_ports(
            &mut state,
            &FakePorts,
            with_input("nope:capture")
        )),
        "no capture port 'nope:capture'"
    );
    ok(handle_with_ports(
        &mut state,
        &FakePorts,
        with_input(CAPTURE),
    ));
    assert_eq!(status(&mut state)["tracks"][0]["input"], CAPTURE);
    let events = take_project_events(&mut state);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProjectEvent::Inputs(inputs) if inputs == &[Some(CAPTURE.to_string())])),
        "no Inputs event"
    );
    ok(handle_with_ports(&mut state, &FakePorts, with_input("")));
    assert_eq!(status(&mut state)["tracks"][0]["input"], Value::Null);
}

#[test]
fn recording_refuses_track_removal_and_armed_input_changes() {
    let (mut state, _audio, _dir) = state("recording");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    assert_eq!(
        error(handle(&mut state, transport(TransportAction::Record, None))),
        "no track is armed"
    );
    arm(&mut state, 0);
    ok(handle(&mut state, transport(TransportAction::Record, None)));
    assert_eq!(
        error(handle(&mut state, Request::TrackRemove { track: index(0) })),
        "cannot remove a track while recording"
    );
    let mut request = track_set(index(0));
    if let Request::TrackSet { input, .. } = &mut request {
        *input = Some(CAPTURE.to_string());
    }
    assert_eq!(
        error(handle(&mut state, request.clone())),
        "cannot change an armed track's input while recording"
    );
    ok(handle(&mut state, transport(TransportAction::Stop, None)));
    ok(handle(&mut state, request));
    ok(handle(&mut state, Request::TrackRemove { track: index(0) }));
}

#[test]
fn locate_takes_frames_bars_beats_or_seconds() {
    let (mut state, mut audio, _dir) = state("locate");
    new_project(&mut state, "Song");
    drain_commands(&mut audio);
    let locate = |position| transport(TransportAction::Locate, Some(position));
    assert_eq!(
        ok(handle(
            &mut state,
            locate(Position::Text("3.2".to_string()))
        )),
        json!({"action": "locate", "frames": 216_000})
    );
    ok(handle(
        &mut state,
        locate(Position::Text("1.5s".to_string())),
    ));
    ok(handle(&mut state, locate(Position::Frames(10))));
    ok(handle(&mut state, locate(Position::Text("4".to_string()))));
    assert_eq!(
        arrangement_commands(&mut audio),
        [
            "Locate(216000)",
            "Locate(72000)",
            "Locate(10)",
            "Locate(288000)"
        ]
    );
    for bad in ["0.1", "1.5", "x", "-1s", "1.2.3", "infs", "NaNs", "-0.5s"] {
        assert_eq!(
            error(handle(&mut state, locate(Position::Text(bad.to_string())))),
            "position must be <bar>.<beat> or <seconds>s",
            "{bad}"
        );
    }
    ok(handle(
        &mut state,
        locate(Position::Text("3600s".to_string())),
    ));
    let too_far = [
        Position::Text("1e300s".to_string()),
        Position::Text("3601s".to_string()),
        Position::Text("18446744073709551615.1".to_string()),
        Position::Text("4611686018427387904".to_string()),
        Position::Frames(u64::MAX),
        Position::Frames(172_800_001),
    ];
    for bad in too_far {
        assert_eq!(
            error(handle(&mut state, locate(bad.clone()))),
            "position is more than an hour past the end of the project",
            "{bad:?}"
        );
    }
    assert_eq!(arrangement_commands(&mut audio), ["Locate(172800000)"]);
    assert_eq!(
        error(handle(&mut state, transport(TransportAction::Locate, None))),
        "locate needs a position"
    );
    ok(handle(&mut state, transport(TransportAction::Play, None)));
    ok(handle(&mut state, transport(TransportAction::Stop, None)));
    assert_eq!(arrangement_commands(&mut audio), ["Play", "Stop"]);
}

#[test]
fn loop_tempo_and_click_are_checked_then_sent() {
    let (mut state, mut audio, _dir) = state("tempo");
    new_project(&mut state, "Song");
    drain_commands(&mut audio);
    assert_eq!(
        error(handle(
            &mut state,
            Request::Loop {
                start: 10,
                end: 10,
                on: true
            }
        )),
        "loop start must be before loop end"
    );
    assert_eq!(
        error(handle(
            &mut state,
            Request::Tempo {
                bpm: Some(400.0),
                meter: None
            }
        )),
        "tempo must be between 20 and 300 bpm"
    );
    assert_eq!(
        error(handle(
            &mut state,
            Request::Tempo {
                bpm: None,
                meter: Some((3, 5))
            }
        )),
        "meter must be 1 to 16 beats of a 2, 4, 8 or 16 note"
    );
    assert_eq!(
        error(handle(
            &mut state,
            Request::Click {
                on: None,
                count_in: Some(3)
            }
        )),
        "count-in must be 0, 1 or 2 bars"
    );
    assert_eq!(arrangement_commands(&mut audio), Vec::<String>::new());
    assert_eq!(
        ok(handle(
            &mut state,
            Request::Loop {
                start: 0,
                end: 96_000,
                on: true
            }
        )),
        json!({"start": 0, "end": 96_000, "on": true})
    );
    assert_eq!(
        ok(handle(
            &mut state,
            Request::Tempo {
                bpm: Some(90.0),
                meter: Some((3, 4))
            }
        )),
        json!({"bpm": 90.0, "meter": [3, 4]})
    );
    assert_eq!(
        ok(handle(
            &mut state,
            Request::Click {
                on: Some(true),
                count_in: Some(1)
            }
        )),
        json!({"on": true, "count_in": 1})
    );
    assert_eq!(
        arrangement_commands(&mut audio),
        [
            "Loop(LoopRegion { start: 0, end: 96000, on: true })",
            "Tempo { bpm: 90.0, meter: (3, 4) }",
            "Click(true)",
            "CountIn(1)",
            "ClickVolume(-12.0)",
        ]
    );
    let status = status(&mut state);
    assert_eq!(status["tempo"], json!({"bpm": 90.0, "meter": [3, 4]}));
    assert_eq!(
        status["loop"],
        json!({"start": 0, "end": 96_000, "on": true})
    );
    assert_eq!(status["click"], json!({"on": true, "count_in": 1}));
    ok(handle(&mut state, Request::ProjectSave));
    let project = saved(&state, "song");
    assert_eq!((project.tempo_bpm, project.meter), (90.0, (3, 4)));
}

#[test]
fn rack_requests_edit_the_selected_or_named_track() {
    let (mut state, mut audio, _dir) = state("track-rack");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    add_track(&mut state, "Bass");
    drain_commands(&mut audio);
    let set = ok(handle(
        &mut state,
        Request::Set {
            param: "gain.level".to_string(),
            value: 4.0,
            track: Some(named("Guitar")),
            bus: None,
            master: false,
        },
    ));
    assert_eq!(
        set,
        json!({"param": "gain.level", "value": 4.0, "clamped": false})
    );
    assert_eq!(
        arrangement_commands(&mut audio),
        ["Param { track: 0, module: 0, param: 0, value: 4.0 }"]
    );
    ok(handle(
        &mut state,
        Request::Bypass {
            id: "gain".to_string(),
            on: true,
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(
        arrangement_commands(&mut audio),
        ["Bypass { track: 1, module: 0, on: true }"]
    );
    ok(handle(
        &mut state,
        Request::RackAdd {
            r#type: "drive".to_string(),
            after: None,
            at: None,
            track: None,
            bus: None,
            master: false,
        },
    ));
    ok(handle(
        &mut state,
        Request::Load {
            preset: "Amp".to_string(),
            track: Some(index(0)),
            bus: None,
            master: false,
        },
    ));
    let got = ok(handle(
        &mut state,
        Request::Get {
            param: Some("gain.level".to_string()),
            track: Some(index(0)),
            bus: None,
            master: false,
        },
    ));
    assert_eq!(got["params"][0]["value"], 0.0);
    ok(handle(&mut state, Request::ProjectSave));
    let project = saved(&state, "song");
    let types = |track: usize| -> Vec<String> {
        project.tracks[track]
            .modules
            .iter()
            .map(|module| module.type_name.clone())
            .collect()
    };
    assert_eq!(types(0), ["gain", "drive"]);
    assert_eq!(types(1), ["gain", "drive"]);
    assert!(project.tracks[1].modules[0].bypass);
    let (_, live) = state.active.as_ref().unwrap();
    assert_eq!(live.name, "Clean");
    assert_eq!(live.modules.len(), 1);
    assert_eq!(live.modules[0].params[0].1, -3.0);
}

#[test]
fn status_reports_transport_position_and_tracks() {
    let (mut state, _audio, _dir) = state("status");
    let idle = status(&mut state);
    assert_eq!(idle["project"], Value::Null);
    assert_eq!(idle["tracks"], json!([]));
    assert_eq!(idle["disk_overflows"], 0);
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    state.shared.playhead.store(216_000, Ordering::Relaxed);
    state.shared.disk_overflows.store(2, Ordering::Relaxed);
    raise_peak(&state.shared.track_input_peak_bits[0], 0.5);
    let status = status(&mut state);
    assert_eq!(status["transport"], "stopped");
    assert_eq!(
        status["position"],
        json!({"frames": 216_000, "bars_beats": "3.2"})
    );
    assert_eq!(status["disk_overflows"], 2);
    assert_eq!(
        status["tracks"][0],
        json!({"id": 0, "name": "Guitar", "input": null, "volume_db": 0.0, "pan": 0.0, "mute": false, "solo": false, "armed": false, "input_peak_db": -6.0, "to_master": true, "sends": [], "peak_db": [-120.0, -120.0], "clip": false})
    );
}

#[test]
fn finished_takes_become_items_and_close_hands_the_project_over() {
    let (mut state, _audio, _dir) = state("takes");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    let folder = state.config.projects_dir.join("song");
    let events = take_project_events(&mut state);
    assert!(matches!(
        events.first(),
        Some(ProjectEvent::Opened { dir, sample_rate: 48_000, .. }) if *dir == folder
    ));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProjectEvent::Readers(readers) if readers.len() == 1 && readers[0].name == "Guitar"))
    );
    write_take(&folder.join("takes/guitar-1.wav"), &[0.25; 100]);
    let Some(ProjectEvent::Readers(readers)) = events.last() else {
        panic!("track_add should end with a Readers event");
    };
    let finished = take(readers[0].track_id, "takes/guitar-1.wav", 1000, 100);
    let latency = Latency {
        capture: 100,
        playback: 50,
    };
    assert_eq!(
        add_finished_takes(&mut state, &[finished], |_| latency),
        Vec::<String>::new()
    );
    assert!(
        take_project_events(&mut state)
            .iter()
            .any(|event| matches!(event, ProjectEvent::Readers(_)))
    );
    ok(handle(&mut state, Request::ProjectClose));
    let item = &saved(&state, "song").tracks[0].items[0];
    assert_eq!((item.position, item.offset), (850, 0));
    assert_eq!(item.takes[0].file, "takes/guitar-1.wav");
    assert!(take_project_events(&mut state).iter().any(|event| matches!(
        event,
        ProjectEvent::Closed(closed) if closed.dir == folder && closed.project.tracks[0].items.len() == 1
    )));
}

#[test]
fn status_names_each_track_by_the_id_items_gives_it() {
    let (mut state, _audio, _dir) = state("status-ids");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    add_track(&mut state, "Bass");
    ok(handle(&mut state, Request::TrackRemove { track: index(0) }));
    add_track(&mut state, "Keys");
    let ids = |payload: &Value| -> Vec<Value> {
        payload["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|track| track["id"].clone())
            .collect()
    };
    let items = ok(handle(&mut state, Request::Items));
    assert_eq!(ids(&status(&mut state)), ids(&items));
    assert_eq!(ids(&items), [json!(1), json!(2)]);
}

#[test]
fn a_finished_take_has_peaks() {
    let (mut state, _audio, _dir) = state("finished-peaks");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    let folder = state.config.projects_dir.join("song");
    write_take(&folder.join("takes/guitar-1.wav"), &[-0.5; 300]);
    let track_id = reader_ids(&mut state)[0].1;
    let finished = take(track_id, "takes/guitar-1.wav", 0, 300);
    add_finished_takes(&mut state, &[finished], |_| Latency::default());
    let reply = ok(handle(
        &mut state,
        Request::TakePeaks {
            file: "takes/guitar-1.wav".to_string(),
            first: None,
            count: None,
        },
    ));
    assert_eq!(reply["peaks"], json!([-64, -64, -64, -64]));
}

fn item_counts(state: &mut EngineState) -> Vec<usize> {
    let items = ok(handle(state, Request::Items));
    items["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| track["items"].as_array().unwrap().len())
        .collect()
}

#[test]
fn takes_finished_while_the_engine_is_busy_are_added_on_a_later_tick() {
    let (mut state, mut audio, _dir) = state("busy");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    let folder = state.config.projects_dir.join("song");
    write_take(&folder.join("takes/guitar-1.wav"), &[0.25; 100]);
    let track_id = reader_ids(&mut state)[0].1;
    let finished = take(track_id, "takes/guitar-1.wav", 1000, 100);
    while state
        .queues
        .commands
        .push(Command::CloseArrangement)
        .is_ok()
    {}
    assert_eq!(
        add_finished_takes(&mut state, &[finished], |_| Latency::default()),
        Vec::<String>::new()
    );
    assert_eq!(item_counts(&mut state), [0]);
    drain_commands(&mut audio);
    assert_eq!(
        add_finished_takes(&mut state, &[], |_| Latency::default()),
        Vec::<String>::new()
    );
    assert_eq!(item_counts(&mut state), [1]);
    assert!(
        take_project_events(&mut state)
            .iter()
            .any(|event| matches!(event, ProjectEvent::Readers(_)))
    );
}

#[test]
fn takes_still_waiting_for_the_engine_are_saved_on_close() {
    let (mut state, mut audio, _dir) = state("busy-close");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    let folder = state.config.projects_dir.join("song");
    write_take(&folder.join("takes/guitar-1.wav"), &[0.25; 100]);
    let track_id = reader_ids(&mut state)[0].1;
    let finished = take(track_id, "takes/guitar-1.wav", 1000, 100);
    while state
        .queues
        .commands
        .push(Command::CloseArrangement)
        .is_ok()
    {}
    add_finished_takes(&mut state, &[finished], |_| Latency::default());
    drain_commands(&mut audio);
    close(&mut state);
    assert_eq!(saved(&state, "song").tracks[0].items.len(), 1);
}

fn swapped_arrangement_frames(audio: &mut AudioQueues) -> Vec<usize> {
    drain_commands(audio)
        .into_iter()
        .filter_map(|command| match command {
            Command::SwapArrangement(arrangement) => Some(arrangement.max_frames()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_quantum_change_rebuilds_the_open_projects_arrangement() {
    let (mut state, mut audio, _dir) = state("quantum");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    drain_commands(&mut audio);
    take_project_events(&mut state);
    reprepare(&mut state, 256).unwrap();
    assert_eq!(swapped_arrangement_frames(&mut audio), [256]);
    assert_eq!(reader_ids(&mut state).len(), 1);
}

#[test]
fn a_rate_change_stops_the_project_until_the_rates_match_again() {
    let (mut state, mut audio, _dir) = state("rate-change");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    drain_commands(&mut audio);
    let mismatch = "project rate 48000 Hz differs from the engine rate 44100 Hz";
    state.shared.sample_rate.store(44_100, Ordering::Relaxed);
    follow_sample_rate(&mut state).unwrap().unwrap();
    assert!(
        drain_commands(&mut audio)
            .iter()
            .any(|command| matches!(command, Command::CloseArrangement))
    );
    assert_eq!(status(&mut state)["project_warning"], mismatch);
    for action in [TransportAction::Play, TransportAction::Record] {
        assert_eq!(error(handle(&mut state, transport(action, None))), mismatch);
    }
    add_track(&mut state, "Bass");
    assert_eq!(swapped_arrangement_frames(&mut audio), Vec::<usize>::new());

    state.shared.sample_rate.store(48_000, Ordering::Relaxed);
    follow_sample_rate(&mut state).unwrap().unwrap();
    assert_eq!(swapped_arrangement_frames(&mut audio), [128]);
    assert_eq!(status(&mut state)["project_warning"], Value::Null);
    ok(handle(&mut state, transport(TransportAction::Play, None)));
}

#[test]
fn projects_dir_from_settings_must_be_absolute() {
    let (mut state, _audio, dir) = state("projects-dir");
    let set = |path: &str| Request::SetSettings {
        bank_kind: None,
        ir_dir: None,
        projects_dir: Some(PathBuf::from(path)),
    };
    assert_eq!(
        error(handle(&mut state, set("relative/songs"))),
        "projects_dir must be an absolute path"
    );
    let wanted = dir.0.join("songs/new");
    let payload = ok(handle(&mut state, set(wanted.to_str().unwrap())));
    assert_eq!(payload["projects_dir"], wanted.display().to_string());
    assert!(wanted.is_dir());
    assert_eq!(state.config.projects_dir, wanted);
    let saved = fs::read_to_string(&state.config.state_file).unwrap();
    assert!(saved.contains("projects_dir"), "{saved}");
    let file = dir.0.join("a-file");
    fs::write(&file, "").unwrap();
    assert!(
        error(handle(&mut state, set(file.join("x").to_str().unwrap()))).starts_with(&format!(
            "cannot create projects_dir {}: ",
            file.join("x").display()
        ))
    );
}

#[test]
fn a_project_still_closing_cannot_be_opened_until_its_last_takes_are_saved() {
    let (mut state, _audio, _dir) = state("closing");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    let id = reader_ids(&mut state)[0].1;
    arm(&mut state, 0);
    ok(handle(&mut state, transport(TransportAction::Record, None)));
    ok(handle(&mut state, transport(TransportAction::Stop, None)));
    ok(handle(&mut state, Request::ProjectClose));
    let open = Request::ProjectOpen {
        name: "Song".to_string(),
        restore: false,
    };
    let still_closing =
        Response::err_code("a project is still closing; try again", "still_closing");
    assert_eq!(handle(&mut state, open.clone()).0, still_closing);
    assert_eq!(
        handle(
            &mut state,
            Request::ProjectNew {
                name: "Other".to_string()
            }
        )
        .0,
        still_closing
    );
    let Some(ProjectEvent::Closed(closed)) = take_project_events(&mut state).pop() else {
        panic!("close should end with a Closed event");
    };
    let folder = closed.dir.clone();
    write_take(&folder.join("takes/guitar-1.wav"), &[0.5; 10]);
    let last = finish_closed(
        &mut state,
        closed,
        &[take(id, "takes/guitar-1.wav", 500, 10)],
        |_| Latency::default(),
    );
    assert_eq!(last.added, ["takes/guitar-1.wav"]);
    assert_eq!(last.saved, Some(folder.join(PROJECT_FILE)));
    assert_eq!(saved(&state, "song").tracks[0].items[0].position, 500);
    ok(handle(&mut state, open));
    assert_eq!(status(&mut state)["project"], "Song");
}

#[test]
fn a_project_closed_without_recording_can_be_opened_at_once() {
    let (mut state, _audio, _dir) = state("closed-quiet");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    ok(handle(&mut state, Request::ProjectClose));
    let Some(ProjectEvent::Closed(quiet)) = take_project_events(&mut state).pop() else {
        panic!("close should end with a Closed event");
    };
    let open = |name: &str| Request::ProjectOpen {
        name: name.to_string(),
        restore: false,
    };
    ok(handle(&mut state, open("Song")));
    arm(&mut state, 0);
    ok(handle(&mut state, transport(TransportAction::Record, None)));
    ok(handle(&mut state, Request::ProjectClose));
    finish_closed(&mut state, quiet, &[], |_| Latency::default());
    assert_eq!(
        error(handle(&mut state, open("Song"))),
        "a project is still closing; try again"
    );
}

#[test]
fn the_daemon_stop_autosaves_the_last_takes() {
    let (mut state, _audio, _dir) = state("stopped");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    ok(handle(&mut state, Request::ProjectSave));
    let id = reader_ids(&mut state)[0].1;
    let closed = xguitare_core::project_control::close_for_stop(&mut state).unwrap();
    let folder = closed.dir.clone();
    write_take(&folder.join("takes/guitar-1.wav"), &[0.5; 10]);
    let last = finish_stopped(closed, &[take(id, "takes/guitar-1.wav", 0, 10)], |_| {
        Latency::default()
    });
    assert_eq!(
        last.saved,
        Some(folder.join(xguitare_core::project::AUTOSAVE_FILE))
    );
    assert_eq!(
        Project::load_autosave(&folder).unwrap().0.tracks[0]
            .items
            .len(),
        1
    );
    assert_eq!(saved(&state, "song").tracks[0].items.len(), 0);
}

#[test]
fn a_take_finished_after_its_track_moved_lands_on_that_track() {
    let (mut state, _audio, _dir) = state("moved");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    add_track(&mut state, "Bass");
    let ids = reader_ids(&mut state);
    ok(handle(&mut state, Request::TrackRemove { track: index(0) }));
    let folder = state.config.projects_dir.join("song");
    write_take(&folder.join("takes/bass-1.wav"), &[0.5; 10]);
    write_take(&folder.join("takes/guitar-1.wav"), &[0.5; 10]);
    let mut latency_asked = Vec::new();
    let failures = add_finished_takes(
        &mut state,
        &[
            take(ids[1].1, "takes/bass-1.wav", 100, 10),
            take(ids[0].1, "takes/guitar-1.wav", 100, 10),
        ],
        |track| {
            latency_asked.push(track);
            Latency::default()
        },
    );
    assert_eq!(
        failures,
        ["take takes/guitar-1.wav has no track; its file is kept"]
    );
    assert_eq!(latency_asked, [0]);
    ok(handle(&mut state, Request::ProjectSave));
    let project = saved(&state, "song");
    assert_eq!(project.tracks[0].name, "Bass");
    assert_eq!(project.tracks[0].items[0].takes[0].file, "takes/bass-1.wav");
}

#[test]
fn click_and_count_in_are_saved_with_the_project() {
    let (mut state, mut audio, _dir) = state("click-saved");
    new_project(&mut state, "Song");
    ok(handle(
        &mut state,
        Request::Click {
            on: Some(true),
            count_in: Some(2),
        },
    ));
    close(&mut state);
    let project = saved(&state, "song");
    assert_eq!((project.click, project.count_in), (true, 2));
    drain_commands(&mut audio);
    ok(handle(
        &mut state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
    assert_eq!(
        status(&mut state)["click"],
        json!({"on": true, "count_in": 2})
    );
    assert_eq!(
        arrangement_commands(&mut audio),
        ["Click(true)", "CountIn(2)", "ClickVolume(-12.0)"]
    );
}

#[test]
fn track_names_of_digits_only_are_refused() {
    let (mut state, _audio, _dir) = state("digits");
    new_project(&mut state, "Song");
    assert_eq!(
        error(handle(
            &mut state,
            Request::TrackAdd {
                name: Some("2".to_string())
            }
        )),
        "track name must not be only digits"
    );
}

#[test]
fn a_refused_set_settings_creates_no_projects_dir() {
    let (mut state, _audio, dir) = state("projects-dir-refused");
    let wanted = dir.0.join("songs");
    let error = error(handle(
        &mut state,
        Request::SetSettings {
            bank_kind: None,
            ir_dir: Some(PathBuf::from("relative")),
            projects_dir: Some(wanted.clone()),
        },
    ));
    assert_eq!(error, "ir_dir 'relative' is not a directory");
    assert!(!wanted.exists());
}

#[test]
fn items_list_each_tracks_items_and_status_counts_loaded_audio() {
    let (mut state, _audio, _dir) = state("items");
    assert_eq!(error(handle(&mut state, Request::Items)), "no project open");
    let idle = status(&mut state);
    assert_eq!(
        (&idle["count_in_remaining"], &idle["loaded_audio_bytes"]),
        (&json!(0), &json!(0))
    );
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    add_track(&mut state, "Bass");
    let folder = state.config.projects_dir.join("song");
    let ids = reader_ids(&mut state);
    write_take(&folder.join("takes/guitar-1.wav"), &[0.25; 100]);
    let revision = status(&mut state)["revision"].clone();
    let latency = Latency {
        capture: 100,
        playback: 50,
    };
    let finished = take(ids[0].1, "takes/guitar-1.wav", 100, 100);
    assert_eq!(
        add_finished_takes(&mut state, &[finished], |_| latency),
        Vec::<String>::new()
    );
    state.shared.count_in_remaining.store(3, Ordering::Relaxed);
    let recorded = status(&mut state);
    assert_ne!(recorded["revision"], revision);
    assert_eq!(recorded["count_in_remaining"], 3);
    assert_eq!(recorded["loaded_audio_bytes"], 400);
    assert_eq!(
        ok(handle(&mut state, Request::Items)),
        json!({
            "sample_rate": 48_000,
            "tracks": [
                {"id": ids[0].1, "name": "Guitar", "items": [
                    {"id": 1, "position": 0, "bars_beats": "1.1", "offset": 50, "length": 50, "take": 0, "takes": ["takes/guitar-1.wav"], "take_frames": 100}
                ]},
                {"id": ids[1].1, "name": "Bass", "items": []}
            ]
        })
    );
    close(&mut state);
    assert_ne!(status(&mut state)["revision"], recorded["revision"]);
    assert_eq!(error(handle(&mut state, Request::Items)), "no project open");
}

fn metronome(
    on: Option<bool>,
    bpm: Option<f64>,
    meter: Option<(u32, u32)>,
    volume: Option<f64>,
) -> Request {
    Request::Metronome {
        on,
        bpm,
        meter,
        volume,
    }
}

fn metronome_settings(audio: &mut AudioQueues) -> Vec<MetronomeSettings> {
    drain_commands(audio)
        .into_iter()
        .filter_map(|command| match command {
            Command::Metronome(settings) => Some(settings),
            _ => None,
        })
        .collect()
}

#[test]
fn metronome_starts_off_at_the_defaults() {
    let (mut state, _audio, _dir) = state("metronome-defaults");
    assert_eq!(
        status(&mut state)["metronome"],
        json!({"on": false, "bpm": 120.0, "meter": [4, 4], "volume_db": -12.0, "beat": 0, "beats_played": 0})
    );
}

#[test]
fn metronome_request_plays_and_returns_the_new_state() {
    let (mut state, mut audio, _dir) = state("metronome-request");
    let revision = state.revision;
    let payload = ok(handle(
        &mut state,
        metronome(Some(true), Some(97.5), Some((3, 4)), Some(-6.0)),
    ));
    assert_eq!(
        payload,
        json!({"on": true, "bpm": 97.5, "meter": [3, 4], "volume_db": -6.0, "beat": 0, "beats_played": 0})
    );
    assert!(state.revision > revision);
    assert_eq!(
        metronome_settings(&mut audio),
        [MetronomeSettings {
            on: true,
            bpm: 97.5,
            meter: (3, 4),
            volume_db: -6.0,
            sample_rate: 48_000.0,
        }]
    );
    state.shared.metronome_beat.store(2, Ordering::Relaxed);
    let payload = ok(handle(&mut state, metronome(Some(false), None, None, None)));
    assert_eq!(payload["on"], false);
    assert_eq!(payload["bpm"], 97.5);
    assert!(!metronome_settings(&mut audio)[0].on);
    assert_eq!(status(&mut state)["metronome"]["beat"], 0, "0 while off");
}

#[test]
fn metronome_errors_are_verbatim_and_change_nothing() {
    let (mut state, mut audio, _dir) = state("metronome-errors");
    let revision = state.revision;
    for (request, expected) in [
        (
            metronome(None, Some(19.9), None, None),
            "bpm must be between 20 and 300",
        ),
        (
            metronome(None, Some(f64::NAN), None, None),
            "bpm must be between 20 and 300",
        ),
        (
            metronome(None, None, Some((17, 4)), None),
            "meter must be 1 to 16 beats of a 2, 4, 8 or 16 note",
        ),
        (
            metronome(None, None, Some((4, 3)), None),
            "meter must be 1 to 16 beats of a 2, 4, 8 or 16 note",
        ),
        (
            metronome(Some(true), None, None, Some(0.5)),
            "volume must be between -40 and 0 dB",
        ),
        (
            metronome(None, None, None, Some(-40.5)),
            "volume must be between -40 and 0 dB",
        ),
    ] {
        assert_eq!(error(handle(&mut state, request)), expected);
    }
    assert_eq!(state.revision, revision);
    assert_eq!(metronome_settings(&mut audio), []);
    assert_eq!(status(&mut state)["metronome"]["on"], false);
}

#[test]
fn metronome_settings_are_saved_in_state_toml_but_not_on() {
    let (mut state, _audio, _dir) = state("metronome-saved");
    ok(handle(
        &mut state,
        metronome(Some(true), Some(90.0), Some((6, 8)), Some(-20.0)),
    ));
    let reloaded =
        Config::load(Some(&state.config.state_file.with_file_name("config.toml"))).unwrap();
    assert_eq!(reloaded.state_warning, None);
    assert_eq!(reloaded.state.metronome.bpm, Some(90.0));
    assert_eq!(reloaded.state.metronome.meter, Some((6, 8)));
    assert_eq!(reloaded.state.metronome.volume_db, Some(-20.0));
    let text = fs::read_to_string(&state.config.state_file).unwrap();
    assert!(!text.lines().any(|line| line.starts_with("on")), "{text}");
}

#[test]
fn metronome_tap_gives_no_bpm_then_the_tapped_tempo() {
    let (mut state, mut audio, _dir) = state("metronome-tap");
    let revision = state.revision;
    assert_eq!(
        ok(handle(&mut state, Request::MetronomeTap)),
        json!({"bpm": null})
    );
    assert_eq!(state.revision, revision);
    let second = ok(handle(&mut state, Request::MetronomeTap));
    assert_eq!(
        second,
        json!({"bpm": 300.0}),
        "two taps at once clamp to 300"
    );
    assert!(state.revision > revision);
    assert_eq!(status(&mut state)["metronome"]["bpm"], 300.0);
    assert_eq!(metronome_settings(&mut audio)[0].bpm, 300.0);
}

#[test]
fn with_a_project_open_the_metronome_is_the_project_click() {
    let (mut state, mut audio, _dir) = state("metronome-project");
    ok(handle(
        &mut state,
        metronome(None, Some(90.0), Some((3, 4)), Some(-20.0)),
    ));
    new_project(&mut state, "Song");
    drain_commands(&mut audio);
    let payload = ok(handle(&mut state, metronome(Some(true), None, None, None)));
    assert_eq!(
        payload,
        json!({"on": true, "bpm": 120.0, "meter": [4, 4], "volume_db": -20.0, "beat": 0, "beats_played": 0})
    );
    let status_now = status(&mut state);
    assert_eq!(status_now["click"]["on"], true);
    assert_eq!(
        arrangement_commands(&mut audio),
        ["Click(true)", "CountIn(0)", "ClickVolume(-20.0)"]
    );
    ok(handle(
        &mut state,
        Request::Click {
            on: Some(false),
            count_in: None,
        },
    ));
    assert_eq!(
        status(&mut state)["metronome"]["on"],
        false,
        "and the click toggles it"
    );
    state.shared.metronome_beat.store(3, Ordering::Relaxed);
    assert_eq!(
        status(&mut state)["metronome"]["beat"],
        0,
        "a count-in with the click off"
    );
    ok(handle(
        &mut state,
        metronome(None, Some(100.0), Some((6, 8)), Some(-30.0)),
    ));
    let commands = arrangement_commands(&mut audio);
    assert!(
        commands.contains(&"Tempo { bpm: 100.0, meter: (6, 8) }".to_string()),
        "{commands:?}"
    );
    assert!(
        commands.contains(&"ClickVolume(-30.0)".to_string()),
        "{commands:?}"
    );
    assert_eq!(
        status(&mut state)["tempo"],
        json!({"bpm": 100.0, "meter": [6, 8]})
    );
    ok(handle(&mut state, Request::MetronomeTap));
    ok(handle(&mut state, Request::MetronomeTap));
    assert_eq!(
        status(&mut state)["tempo"]["bpm"],
        300.0,
        "tap sets the Project tempo"
    );
    close(&mut state);
    assert_eq!(
        status(&mut state)["metronome"],
        json!({"on": false, "bpm": 90.0, "meter": [3, 4], "volume_db": -30.0, "beat": 0, "beats_played": 0})
    );
}

#[test]
fn opening_a_project_switches_the_practice_metronome_off() {
    let (mut state, _audio, _dir) = state("metronome-off-on-open");
    ok(handle(&mut state, metronome(Some(true), None, None, None)));
    new_project(&mut state, "Song");
    close(&mut state);
    assert_eq!(status(&mut state)["metronome"]["on"], false);
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
fn a_busy_engine_leaves_the_metronome_unchanged() {
    let (mut state, mut audio, _dir) = state("metronome-busy");
    let saved = |state: &EngineState| fs::read_to_string(&state.config.state_file).ok();
    fill_commands_to(&mut state, 0);
    let (before, file, revision) = (
        status(&mut state)["metronome"].clone(),
        saved(&state),
        state.revision,
    );
    assert_eq!(
        error(handle(
            &mut state,
            metronome(Some(true), Some(90.0), None, Some(-20.0))
        )),
        "engine busy"
    );
    assert_eq!(status(&mut state)["metronome"], before);
    assert_eq!(saved(&state), file);
    assert_eq!(state.revision, revision);

    drain_commands(&mut audio);
    new_project(&mut state, "Song");
    drain_commands(&mut audio);
    fill_commands_to(&mut state, 3);
    let (before, file, revision) = (
        status(&mut state)["metronome"].clone(),
        saved(&state),
        state.revision,
    );
    assert_eq!(
        error(handle(
            &mut state,
            metronome(Some(true), None, None, Some(-20.0))
        )),
        "engine busy",
        "on/off takes three commands and the volume one"
    );
    assert_eq!(status(&mut state)["metronome"], before);
    assert_eq!(saved(&state), file);
    assert_eq!(state.revision, revision);
    ok(handle(
        &mut state,
        metronome(None, Some(100.0), None, Some(-20.0)),
    ));
    assert_eq!(status(&mut state)["metronome"]["volume_db"], -20.0);
}

#[test]
fn locate_is_bounded_by_the_end_each_item_plays() {
    let (mut state, mut audio, _dir) = state("locate-end");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    close(&mut state);
    let folder = state.config.projects_dir.join("song");
    let mut project = saved(&state, "song");
    let take = |file: &str, frames| xguitare_core::project::Take {
        file: file.to_string(),
        frames,
    };
    write_take(&folder.join("takes/guitar-2.wav"), &[0.25; 1000]);
    write_take(&folder.join("takes/guitar-3.wav"), &[0.25; 1000]);
    let items = &mut project.tracks[0].items;
    items.push(xguitare_core::project::Item {
        id: 1,
        position: 1000,
        offset: 200,
        length: None,
        take: 1,
        takes: vec![
            take("takes/guitar-1.wav", 10_000),
            take("takes/guitar-2.wav", 1000),
        ],
    });
    items.push(xguitare_core::project::Item {
        id: 2,
        position: 1500,
        offset: 0,
        length: Some(100),
        take: 0,
        takes: vec![take("takes/guitar-3.wav", 1000)],
    });
    project.next_item_id = 3;
    project.save(&folder).unwrap();
    ok(handle(
        &mut state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
    drain_commands(&mut audio);
    let hour = 3600 * 48_000;
    let locate = |frames| transport(TransportAction::Locate, Some(Position::Frames(frames)));
    ok(handle(&mut state, locate(1800 + hour)));
    assert_eq!(
        error(handle(&mut state, locate(1801 + hour))),
        "position is more than an hour past the end of the project"
    );
    let lengths: Vec<Value> = ok(handle(&mut state, Request::Items))["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| json!([item["id"], item["length"]]))
        .collect();
    assert_eq!(lengths, [json!([1, 800]), json!([2, 100])]);
}

fn open_song() -> Request {
    Request::ProjectOpen {
        name: "Song".to_string(),
        restore: false,
    }
}

/// Reads Song as the socket's first phase and the read between the locks do.
fn read_song(state: &EngineState) -> ReadProject {
    match begin_project_read(state, &open_song()) {
        Some(Ok(ticket)) => read_project(&ticket),
        Some(Err(response)) => panic!("expected a ticket, got {response:?}"),
        None => panic!("project_open should start a read"),
    }
}

fn install(state: &mut EngineState, read: ReadProject) -> Response {
    let connection = open_connection(state);
    handle_request(state, None, connection, open_song(), Some(read)).0
}

/// Song with Track Guitar holding a loaded Take and an Item its load drops with a warning.
fn song_with_take_and_warning(state: &mut EngineState) {
    new_project(state, "Song");
    add_track(state, "Guitar");
    close(state);
    let folder = state.config.projects_dir.join("song");
    write_take(&folder.join("takes/a.wav"), &[0.25; 4]);
    let mut project = Project::load(&folder).unwrap().0;
    let item = |id, file: &str, frames| Item {
        id,
        position: 0,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![Take {
            file: file.to_string(),
            frames,
        }],
    };
    project.tracks[0].items = vec![item(1, "takes/a.wav", 4), item(2, "takes/gone.wav", 10)];
    project.next_item_id = 3;
    project.save(&folder).unwrap();
    let path = folder.join(PROJECT_FILE);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("frames = 10", "frames = 0")).unwrap();
}

#[test]
fn a_read_project_installs_like_a_plain_open() {
    let (mut state, _audio, _dir) = state("two-phase");
    song_with_take_and_warning(&mut state);
    let file = state.config.projects_dir.join("song").join(PROJECT_FILE);
    let saved_text = fs::read_to_string(&file).unwrap();
    let read = read_song(&state);
    let two_phase = serde_json::to_string(&install(&mut state, read)).unwrap();
    let two_phase_status = status(&mut state);
    close(&mut state);
    fs::write(&file, saved_text).unwrap();
    let connection = open_connection(&mut state);
    let (one_phase, _) = handle_line(
        &mut state,
        None,
        connection,
        r#"{"cmd":"project_open","name":"Song"}"#,
    );
    let mut one_phase_status = status(&mut state);
    one_phase_status["revision"] = two_phase_status["revision"].clone();
    assert_eq!(two_phase, one_phase);
    assert!(two_phase.contains("dropped item 2"), "{two_phase}");
    assert_eq!(two_phase_status, one_phase_status);
    assert_eq!(two_phase_status["loaded_audio_bytes"], 16);
}

#[test]
fn an_open_project_refuses_before_any_file_is_read() {
    let (mut state, _audio, _dir) = state("refuse-first");
    assert!(begin_project_read(&state, &Request::Status).is_none());
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    assert_eq!(
        begin_project_read(&state, &open_song())
            .unwrap()
            .unwrap_err(),
        Response::err("a project is open; close it first")
    );
    arm(&mut state, 0);
    ok(handle(&mut state, transport(TransportAction::Record, None)));
    ok(handle(&mut state, transport(TransportAction::Stop, None)));
    ok(handle(&mut state, Request::ProjectClose));
    assert_eq!(
        begin_project_read(&state, &open_song())
            .unwrap()
            .unwrap_err(),
        Response::err_code("a project is still closing; try again", "still_closing")
    );
}

#[test]
fn a_project_opened_between_read_and_install_refuses_the_second() {
    let (mut state, _audio, _dir) = state("open-between");
    new_project(&mut state, "Song");
    close(&mut state);
    let read = read_song(&state);
    ok(handle(&mut state, open_song()));
    assert_eq!(
        install(&mut state, read),
        Response::err("a project is open; close it first")
    );
}

#[test]
fn a_project_opened_and_closed_between_read_and_install_is_read_again() {
    let (mut state, _audio, _dir) = state("close-between");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    close(&mut state);
    let read = read_song(&state);
    ok(handle(&mut state, open_song()));
    add_track(&mut state, "Bass");
    ok(handle(&mut state, Request::ProjectSave));
    close(&mut state);
    let Response::Ok(_) = install(&mut state, read) else {
        panic!("the second read should open Song");
    };
    let names: Vec<Value> = status(&mut state)["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| track["name"].clone())
        .collect();
    assert_eq!(names, [json!("Guitar"), json!("Bass")]);
}

#[test]
fn a_projects_dir_changed_between_read_and_install_is_read_again() {
    let (mut state, _audio, dir) = state("dir-between");
    new_project(&mut state, "Song");
    close(&mut state);
    let read = read_song(&state);
    let other = dir.0.join("other-projects");
    ok(handle(
        &mut state,
        Request::SetSettings {
            bank_kind: None,
            ir_dir: None,
            projects_dir: Some(other),
        },
    ));
    assert_eq!(
        install(&mut state, read),
        Response::err("no project 'Song'")
    );
    assert_eq!(status(&mut state)["project"], Value::Null);
}

#[test]
fn read_project_reports_a_take_that_cannot_be_loaded() {
    let (mut state, _audio, _dir) = state("bad-take");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    close(&mut state);
    let folder = state.config.projects_dir.join("song");
    let mut project = Project::load(&folder).unwrap().0;
    project.tracks[0].items = vec![Item {
        id: 1,
        position: 0,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![Take {
            file: "takes/gone.wav".to_string(),
            frames: 10,
        }],
    }];
    project.next_item_id = 2;
    project.save(&folder).unwrap();
    let read = read_song(&state);
    let Response::Err { error: message, .. } = install(&mut state, read) else {
        panic!("a missing Take should refuse the open");
    };
    assert!(
        message.starts_with("cannot load take takes/gone.wav: "),
        "{message}"
    );
    assert_eq!(error(handle(&mut state, open_song())), message);
    assert_eq!(status(&mut state)["project"], Value::Null);
}
