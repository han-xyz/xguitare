use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::{Value, json};
use xguitare_core::control::{
    EngineState, PeakMeters, Position, Request, Response, TrackRef, TransportAction, handle,
    handle_with_ports,
};
use xguitare_core::engine::{AudioQueues, Command, EngineShared, engine_queues, raise_peak};
use xguitare_core::ports::{AudioPorts, AvailablePort};
use xguitare_core::project::{PROJECT_FILE, Project};
use xguitare_core::project_control::{
    ProjectEvent, Projects, add_finished_takes, autosave_if_due, take_project_events,
};
use xguitare_core::take_capture::{FinishedTake, Latency, TakeSpan};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BankEntry, BuildWarning, Config, Ports};

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
    let state = EngineState {
        bank,
        active,
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

fn saved(state: &EngineState, folder: &str) -> Project {
    Project::load(&state.config.projects_dir.join(folder)).unwrap()
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
    ok(handle(&mut state, Request::ProjectClose));
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
        },
    ] {
        assert_eq!(error(handle(&mut state, request)), "no project open");
    }
}

#[test]
fn project_open_refuses_a_rate_other_than_the_engine_rate() {
    let (mut state, _audio, _dir) = state("rate");
    new_project(&mut state, "Song");
    ok(handle(&mut state, Request::ProjectClose));
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
fn project_open_reports_a_newer_autosave_and_restores_it() {
    let (mut state, _audio, _dir) = state("restore");
    new_project(&mut state, "Song");
    add_track(&mut state, "Guitar");
    ok(handle(&mut state, Request::ProjectClose));
    let folder = state.config.projects_dir.join("song");
    let mut autosave = Project::load(&folder).unwrap();
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
    ok(handle(&mut state, Request::ProjectClose));
    std::thread::sleep(Duration::from_millis(20));
    autosave.save_autosave(&folder).unwrap();
    let restored = ok(handle(&mut state, open(true)));
    assert_eq!(restored["restored"], true);
    assert_eq!(status(&mut state)["tracks"].as_array().unwrap().len(), 2);
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
    assert_eq!(Project::load_autosave(&folder).unwrap().tracks.len(), 1);
    assert_eq!(saved(&state, "song").tracks.len(), 0);
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
    ok(handle(&mut state, Request::ProjectClose));
    new_project(&mut state, "A Song");
    ok(handle(&mut state, Request::ProjectClose));
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
                track: None
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
        json!({"index": 0, "name": "Guitar", "input": null, "volume_db": -6.0, "pan": -0.5, "mute": true, "solo": false, "arm": false})
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
    for bad in ["0.1", "1.5", "x", "-1s", "1.2.3", "infs"] {
        assert_eq!(
            error(handle(&mut state, locate(Position::Text(bad.to_string())))),
            "position must be <bar>.<beat> or <seconds>s",
            "{bad}"
        );
    }
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
        },
    ));
    ok(handle(
        &mut state,
        Request::Load {
            preset: "Amp".to_string(),
            track: Some(index(0)),
        },
    ));
    let got = ok(handle(
        &mut state,
        Request::Get {
            param: Some("gain.level".to_string()),
            track: Some(index(0)),
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
        json!({"name": "Guitar", "input": null, "volume_db": 0.0, "pan": 0.0, "mute": false, "solo": false, "armed": false, "input_peak_db": -6.0})
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
        Some(ProjectEvent::Opened { dir, sample_rate: 48_000 }) if *dir == folder
    ));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProjectEvent::Readers(readers) if readers.len() == 1 && readers[0].0 == "Guitar"))
    );
    write_take(&folder.join("takes/guitar-1.wav"), &[0.25; 100]);
    let finished = FinishedTake {
        track: 0,
        file: "takes/guitar-1.wav".to_string(),
        span: TakeSpan {
            start: 1000,
            frames: 100,
            loop_take: false,
        },
    };
    let latency = Latency {
        capture: 100,
        playback: 50,
    };
    assert_eq!(
        add_finished_takes(&mut state, &[(finished, latency)]),
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
