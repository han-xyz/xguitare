use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use hound::{SampleFormat, WavSpec, WavWriter};
use xguitare_core::control::{EngineState, handle};
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues};
use xguitare_core::project::{Item, Project, Take};
use xguitare_core::project_control::take_project_events;
use xguitare_core::project_state::ProjectEvent;
use xguitare_core::render::RenderTask;
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, Config};
use xguitare_protocol::{IndexOrName, Request, Response, SendLevel, TrackRef};

const UPDATE_SWITCH: &str = "UPDATE_FIXTURES";
const TEMP_TOKEN: &str = "<temp>";
const PRESET: &str = r#"name = "Fixture"

[[module]]
type = "gate"
bypass = true

[[module]]
type = "gain"

[module.params]
level = -3

[[module]]
type = "autowah"

[[module]]
type = "cabinet"
ir = "ir.wav"
"#;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Session {
    state: EngineState,
    _audio: AudioQueues,
    dir: TempDir,
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../xguitare-protocol/tests/fixtures/replies")
}

fn write_wav(path: &Path, samples: &[f32]) {
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

fn session(test: &str) -> Session {
    let dir = std::env::temp_dir().join(format!("xguitare-replies-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bank_dir = dir.join("bank");
    fs::create_dir_all(&bank_dir).unwrap();
    fs::write(bank_dir.join("01-fixture.toml"), PRESET).unwrap();
    write_wav(&bank_dir.join("ir.wav"), &[1.0, 0.5, 0.25]);
    let config_path = dir.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let mut config = Config::load(Some(&config_path)).unwrap();
    config.bank_dir.clone_from(&bank_dir);
    config.projects_dir = dir.join("projects");
    let shared = Arc::new(EngineShared::default());
    shared.sample_rate.store(48_000, Ordering::Relaxed);
    shared.buffer_size.store(128, Ordering::Relaxed);
    let (queues, audio, tap) = engine_queues(64);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state = EngineState::new(
        Bank::load_dir(&bank_dir).unwrap(),
        shared,
        queues,
        tuner,
        config,
        48_000.0,
        128,
    );
    Session {
        state,
        _audio: audio,
        dir: TempDir(dir),
    }
}

fn ok(s: &mut Session, request: Request) {
    let (response, _) = handle(&mut s.state, request);
    assert!(
        matches!(response, Response::Ok(_)),
        "expected ok, got {response:?}"
    );
}

/// The reply's wire text with the per-run temp dir replaced by a fixed token.
fn reply_text(s: &mut Session, request: Request) -> String {
    let (response, _) = handle(&mut s.state, request);
    assert!(
        matches!(response, Response::Ok(_)),
        "expected ok, got {response:?}"
    );
    let dir = s.dir.0.display().to_string();
    serde_json::to_string(&response)
        .unwrap()
        .replace(&dir, TEMP_TOKEN)
}

fn check_fixture(name: &str, text: &str) {
    let path = fixture_dir().join(format!("{name}.json"));
    let line = format!("{text}\n");
    if std::env::var_os(UPDATE_SWITCH).is_some() {
        fs::create_dir_all(fixture_dir()).unwrap();
        fs::write(&path, &line).unwrap();
        return;
    }
    let Ok(expected) = fs::read_to_string(&path) else {
        panic!(
            "no fixture {}; run with {UPDATE_SWITCH}=1 to write it",
            path.display()
        );
    };
    assert_eq!(line, expected, "reply differs from {}", path.display());
}

fn get(param: &str) -> Request {
    Request::Get {
        param: Some(param.to_string()),
        track: None,
        bus: None,
        master: false,
    }
}

fn load(preset: &str) -> Request {
    Request::Load {
        preset: preset.to_string(),
        track: None,
        bus: None,
        master: false,
    }
}

fn name(text: &str) -> IndexOrName {
    IndexOrName::Name(text.to_string())
}

/// Opens Project "Song" with Track "Guitar" holding one Item with its Take and Track "Bass" holding none.
fn open_song(s: &mut Session) {
    ok(
        s,
        Request::ProjectNew {
            name: "Song".to_string(),
        },
    );
    for track in ["Guitar", "Bass"] {
        ok(
            s,
            Request::TrackAdd {
                name: Some(track.to_string()),
            },
        );
    }
    ok(s, Request::ProjectClose);
    let folder = s.state.config.projects_dir.join("song");
    write_wav(&folder.join("takes/guitar-1.wav"), &vec![0.25; 4_800]);
    let mut project = Project::load(&folder).unwrap().0;
    project.tracks[0].items = vec![Item {
        id: 1,
        position: 96_000,
        offset: 100,
        length: Some(2_000),
        take: 0,
        takes: vec![Take {
            file: "takes/guitar-1.wav".to_string(),
            frames: 4_800,
        }],
    }];
    project.next_item_id = 2;
    project.save(&folder).unwrap();
    ok(
        s,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    );
}

fn render_task(s: &mut Session) -> RenderTask {
    take_project_events(&mut s.state)
        .into_iter()
        .find_map(|event| match event {
            ProjectEvent::Render(task) => Some(task),
            _ => None,
        })
        .expect("render starts a RenderTask")
}

#[test]
fn get_replies_match_their_fixtures() {
    let mut s = session("get");
    ok(&mut s, load("Fixture"));
    for (fixture, param) in [
        ("get-plain", "gain.level"),
        ("get-choice", "autowah.mode"),
        ("get-cabinet-ir", "cabinet.level"),
        ("get-bypassed", "gate"),
    ] {
        let text = reply_text(&mut s, get(param));
        check_fixture(fixture, &text);
    }
}

#[test]
fn items_reply_matches_its_fixture() {
    let mut s = session("items");
    open_song(&mut s);
    let text = reply_text(&mut s, Request::Items);
    check_fixture("items", &text);
}

#[test]
fn status_replies_match_their_fixtures() {
    let mut s = session("status");
    let text = reply_text(&mut s, Request::Status);
    check_fixture("status-no-project", &text);
    open_song(&mut s);
    ok(
        &mut s,
        Request::BusAdd {
            name: Some("Verb".to_string()),
        },
    );
    ok(
        &mut s,
        Request::SendSet {
            track: name("Guitar"),
            bus: name("Verb"),
            level: Some(SendLevel::Db(-6.0)),
            pre: Some(true),
        },
    );
    ok(
        &mut s,
        Request::MasterSet {
            volume: SendLevel::Db(-3.0),
        },
    );
    take_project_events(&mut s.state);
    ok(
        &mut s,
        Request::Render {
            range: None,
            bits: None,
            name: None,
        },
    );
    let task = render_task(&mut s);
    let text = reply_text(&mut s, Request::Status);
    check_fixture("status-project-render-running", &text);
    task.job.fail("disk full".to_string());
    ok(
        &mut s,
        Request::MasterSet {
            volume: SendLevel::Off,
        },
    );
    let text = reply_text(&mut s, Request::Status);
    check_fixture("status-project-render-failed", &text);
}

#[test]
fn status_writes_volumes_pans_and_sends_as_before_the_typed_reply() {
    let mut s = session("widened");
    open_song(&mut s);
    ok(
        &mut s,
        Request::BusAdd {
            name: Some("Verb".to_string()),
        },
    );
    ok(
        &mut s,
        Request::BusSet {
            bus: name("Verb"),
            name: None,
            volume: Some(-3.3),
            pan: None,
            mute: None,
            solo: None,
        },
    );
    ok(
        &mut s,
        Request::TrackSet {
            track: TrackRef::Index(0),
            input: None,
            volume: Some(-6.05),
            pan: Some(0.1),
            mute: None,
            solo: None,
            arm: None,
            name: None,
            to_master: None,
        },
    );
    ok(
        &mut s,
        Request::SendSet {
            track: name("Guitar"),
            bus: name("Verb"),
            level: Some(SendLevel::Db(-6.05)),
            pre: None,
        },
    );
    let text = reply_text(&mut s, Request::Status);
    for wire in [
        r#""name":"Verb","pan":0.0,"peak_db":[-120.0,-120.0],"solo":false,"volume_db":-3.299999952316284}"#,
        r#""level_db":-6.050000190734863"#,
        r#""name":"Guitar","pan":0.10000000149011612,"#,
        r#""volume_db":-6.050000190734863},{"armed""#,
    ] {
        assert!(text.contains(wire), "{wire} not in {text}");
    }
}
