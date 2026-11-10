use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use xguitare_core::control::{
    BusRef, ConnectionId, EngineState, PeakMeters, Request, SendLevel, TrackRef, handle_line,
    open_connection, passthrough_rack,
};
use xguitare_core::engine::{
    AudioQueues, Command, EngineShared, FadeLengths, MeterPoint, RackSwitcher, engine_queues,
};
use xguitare_core::project::{PROJECT_FILE, Project};
use xguitare_core::project_control::{Projects, autosave_if_due};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BankEntry, Config};

const ONE_TARGET: &str = "give track, bus or master, not more than one";
const CABINET: &str = "a cabinet is not allowed on a Bus or Master";
const BLOCK: usize = 128;
/// Blocks of `BLOCK` frames that cover a 20 ms crossfade at 48 kHz.
const FADE_BLOCKS: usize = 8;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Session {
    state: EngineState,
    audio: AudioQueues,
    connection: ConnectionId,
    _dir: TempDir,
}

fn session(test: &str) -> Session {
    let dir = std::env::temp_dir().join(format!("xguitare-mixctl-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bank_dir = dir.join("bank");
    fs::create_dir_all(&bank_dir).unwrap();
    fs::write(
        bank_dir.join("01-clean.toml"),
        "name = \"Clean\"\n[[module]]\ntype = \"gain\"\n",
    )
    .unwrap();
    fs::write(dir.join("cab.wav"), "").unwrap();
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
    let mut state = EngineState {
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
    let connection = open_connection(&mut state);
    Session {
        state,
        audio,
        connection,
        _dir: TempDir(dir),
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

/// A Project `Song` with Tracks `Guitar` and `Keys` and a Bus `Verb`; the queued commands drained.
fn mixer(test: &str) -> Session {
    let mut s = session(test);
    ok(&mut s, r#"{"cmd":"project_new","name":"Song"}"#);
    ok(&mut s, r#"{"cmd":"track_add","name":"Guitar"}"#);
    ok(&mut s, r#"{"cmd":"track_add","name":"Keys"}"#);
    ok(&mut s, r#"{"cmd":"bus_add","name":"Verb"}"#);
    drain(&mut s);
    s
}

fn drain(s: &mut Session) -> Vec<Command> {
    let mut commands = Vec::new();
    while let Ok(command) = s.audio.commands.pop() {
        commands.push(command);
    }
    commands
}

fn arrangement_commands(s: &mut Session) -> Vec<String> {
    drain(s)
        .into_iter()
        .filter_map(|command| match command {
            Command::Arrangement(command) => Some(format!("{command:?}")),
            _ => None,
        })
        .collect()
}

fn swapped(s: &mut Session) -> bool {
    drain(s)
        .iter()
        .any(|command| matches!(command, Command::SwapArrangement(_)))
}

fn revision(s: &EngineState) -> u64 {
    s.revision
}

fn saved(s: &mut Session) -> Project {
    ok(s, r#"{"cmd":"project_save"}"#);
    Project::load(&s.state.config.projects_dir.join("song")).unwrap()
}

#[test]
fn bus_add_names_numbers_and_limits_buses() {
    let mut s = session("bus-add");
    assert_eq!(error(&mut s, r#"{"cmd":"bus_add"}"#), "no project open");
    ok(&mut s, r#"{"cmd":"project_new","name":"Song"}"#);
    drain(&mut s);
    let before = revision(&s.state);
    let added = ok(&mut s, r#"{"cmd":"bus_add"}"#);
    assert_eq!(added["bus"], json!({"id": 0, "index": 0, "name": "Bus 1"}));
    assert!(swapped(&mut s), "bus_add did not rebuild the Arrangement");
    assert!(revision(&s.state) > before);
    let added = ok(&mut s, r#"{"cmd":"bus_add","name":" Verb "}"#);
    assert_eq!(added["bus"], json!({"id": 1, "index": 1, "name": "Verb"}));
    assert_eq!(
        error(&mut s, r#"{"cmd":"bus_add","name":"verb"}"#),
        "a bus named 'verb' already exists"
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"bus_add","name":"  "}"#),
        "bus name must not be empty"
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"bus_add","name":"12"}"#),
        "bus name must not be only digits"
    );
    for _ in 2..8 {
        ok(&mut s, r#"{"cmd":"bus_add"}"#);
    }
    assert_eq!(error(&mut s, r#"{"cmd":"bus_add"}"#), "at most 8 buses");
    let project = saved(&mut s);
    let names: Vec<&str> = project.buses.iter().map(|bus| bus.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Bus 1", "Verb", "Bus 2", "Bus 3", "Bus 4", "Bus 5", "Bus 6", "Bus 7"
        ]
    );
}

#[test]
fn bus_remove_removes_its_sends_and_keeps_the_other_ids() {
    let mut s = mixer("bus-remove");
    ok(&mut s, r#"{"cmd":"bus_add","name":"Delay"}"#);
    ok(
        &mut s,
        r#"{"cmd":"send_set","track":0,"bus":"Verb","level":-6}"#,
    );
    ok(
        &mut s,
        r#"{"cmd":"send_set","track":0,"bus":"Delay","level":-3}"#,
    );
    drain(&mut s);
    assert_eq!(
        error(&mut s, r#"{"cmd":"bus_remove","bus":"Echo"}"#),
        "no bus 'Echo'"
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"bus_remove","bus":5}"#),
        "no bus '5'"
    );
    let removed = ok(&mut s, r#"{"cmd":"bus_remove","bus":"Verb"}"#);
    assert_eq!(removed["bus"], json!({"id": 0, "index": 0, "name": "Verb"}));
    assert!(
        swapped(&mut s),
        "bus_remove did not rebuild the Arrangement"
    );
    let project = saved(&mut s);
    assert_eq!(project.buses.len(), 1);
    assert_eq!(project.buses[0].id, 1);
    assert_eq!(project.tracks[0].sends.len(), 1);
    assert_eq!(project.tracks[0].sends[0].bus, 1);
    let status = ok(&mut s, r#"{"cmd":"status"}"#);
    assert_eq!(
        status["tracks"][0]["sends"],
        json!([{"bus": 0, "level_db": -3.0, "pre": false}])
    );
}

#[test]
fn bus_set_checks_every_value_then_sends_commands() {
    let mut s = mixer("bus-set");
    assert_eq!(
        error(&mut s, r#"{"cmd":"bus_set","bus":0,"volume":7}"#),
        "bus 'Verb' volume must be between -60 and 6 dB"
    );
    assert_eq!(
        error(
            &mut s,
            r#"{"cmd":"bus_set","bus":0,"pan":-1.5,"mute":true}"#
        ),
        "bus 'Verb' pan must be between -1 and 1"
    );
    ok(&mut s, r#"{"cmd":"bus_add","name":"Delay"}"#);
    assert_eq!(
        error(&mut s, r#"{"cmd":"bus_set","bus":0,"name":"DELAY"}"#),
        "a bus named 'DELAY' already exists"
    );
    drain(&mut s);
    let before = revision(&s.state);
    let set = ok(
        &mut s,
        r#"{"cmd":"bus_set","bus":"Verb","name":"Room","volume":-3,"pan":0.5,"mute":true,"solo":true}"#,
    );
    assert_eq!(
        set["bus"],
        json!({"id": 0, "index": 0, "name": "Room", "volume_db": -3.0, "pan": 0.5, "mute": true, "solo": true})
    );
    assert_eq!(
        arrangement_commands(&mut s),
        [
            "BusVolume { bus: 0, db: -3.0 }",
            "BusPan { bus: 0, pan: 0.5 }",
            "BusMute { bus: 0, on: true }",
            "BusSolo { bus: 0, on: true }",
        ]
    );
    assert!(revision(&s.state) > before);
    let bus = &saved(&mut s).buses[0];
    assert_eq!(
        (
            bus.name.as_str(),
            bus.volume_db,
            bus.pan,
            bus.mute,
            bus.solo
        ),
        ("Room", -3.0, 0.5, true, true)
    );
}

#[test]
fn send_set_takes_a_level_in_db_or_off_and_pre() {
    let mut s = mixer("send-set");
    let set = ok(
        &mut s,
        r#"{"cmd":"send_set","track":"Guitar","bus":"Verb","level":-6}"#,
    );
    assert_eq!(
        set,
        json!({"ok": true, "track": 0, "bus": 0, "level": -6.0, "pre": false})
    );
    assert_eq!(
        arrangement_commands(&mut s),
        ["SendLevel { track: 0, bus: 0, db: -6.0 }"]
    );
    let set = ok(&mut s, r#"{"cmd":"send_set","track":0,"bus":0,"pre":true}"#);
    assert_eq!(set["level"], -6.0);
    assert_eq!(set["pre"], true);
    assert_eq!(
        arrangement_commands(&mut s),
        ["SendPre { track: 0, bus: 0, on: true }"]
    );
    let status = ok(&mut s, r#"{"cmd":"status"}"#);
    assert_eq!(
        status["tracks"][0]["sends"],
        json!([{"bus": 0, "level_db": -6.0, "pre": true}])
    );
    assert_eq!(status["tracks"][1]["sends"], json!([]));
    let off = ok(
        &mut s,
        r#"{"cmd":"send_set","track":0,"bus":0,"level":"off"}"#,
    );
    assert_eq!(off["level"], "off");
    assert_eq!(
        arrangement_commands(&mut s),
        ["SendLevel { track: 0, bus: 0, db: -inf }"]
    );
    let status = ok(&mut s, r#"{"cmd":"status"}"#);
    assert_eq!(status["tracks"][0]["sends"], json!([]));
    assert_eq!(saved(&mut s).tracks[0].sends, []);
    assert_eq!(
        error(&mut s, r#"{"cmd":"send_set","track":1,"bus":0,"level":7}"#),
        "track 'Keys' send level must be -inf or between -60 and 6 dB"
    );
    assert_eq!(
        error(
            &mut s,
            r#"{"cmd":"send_set","track":1,"bus":0,"level":-1e40}"#
        ),
        "track 'Keys' send level must be -inf or between -60 and 6 dB"
    );
    assert_eq!(
        error(
            &mut s,
            r#"{"cmd":"send_set","track":1,"bus":"Echo","level":0}"#
        ),
        "no bus 'Echo'"
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"send_set","track":3,"bus":0,"level":0}"#),
        "no track '3'"
    );
    let loud = error(
        &mut s,
        r#"{"cmd":"send_set","track":1,"bus":0,"level":"loud"}"#,
    );
    assert!(
        loud.starts_with(r#"invalid request: level must be a number in dB or "off""#),
        "{loud}"
    );
    assert_eq!(arrangement_commands(&mut s), Vec::<String>::new());
}

#[test]
fn master_set_checks_the_volume_and_sends_it() {
    let mut s = session("master-set");
    assert_eq!(
        error(&mut s, r#"{"cmd":"master_set","volume":0}"#),
        "no project open"
    );
    ok(&mut s, r#"{"cmd":"project_new","name":"Song"}"#);
    drain(&mut s);
    assert_eq!(
        error(&mut s, r#"{"cmd":"master_set","volume":-61}"#),
        "master volume must be -inf or between -60 and 6 dB"
    );
    let set = ok(&mut s, r#"{"cmd":"master_set","volume":-3}"#);
    assert_eq!(set, json!({"ok": true, "volume_db": -3.0}));
    assert_eq!(arrangement_commands(&mut s), ["MasterVolume { db: -3.0 }"]);
    assert_eq!(saved(&mut s).master.volume_db, -3.0);
    assert_eq!(
        error(&mut s, r#"{"cmd":"master_set","volume":-1e40}"#),
        "master volume must be -inf or between -60 and 6 dB"
    );
    let off = ok(&mut s, r#"{"cmd":"master_set","volume":"off"}"#);
    assert_eq!(off, json!({"ok": true, "volume_db": null}));
    assert_eq!(arrangement_commands(&mut s), ["MasterVolume { db: -inf }"]);
    assert_eq!(saved(&mut s).master.volume_db, f32::NEG_INFINITY);
    let loud = error(&mut s, r#"{"cmd":"master_set","volume":"loud"}"#);
    assert!(
        loud.starts_with(r#"invalid request: level must be a number in dB or "off""#),
        "{loud}"
    );
}

#[test]
fn track_set_switches_to_master() {
    let mut s = mixer("to-master");
    let set = ok(
        &mut s,
        r#"{"cmd":"track_set","track":"Keys","to_master":false}"#,
    );
    assert_eq!(set["to_master"], false);
    assert_eq!(
        arrangement_commands(&mut s),
        ["ToMaster { track: 1, on: false }"]
    );
    let status = ok(&mut s, r#"{"cmd":"status"}"#);
    assert_eq!(status["tracks"][1]["to_master"], false);
    assert_eq!(status["tracks"][0]["to_master"], true);
    assert!(!saved(&mut s).tracks[1].to_master);
}

#[test]
fn rack_requests_take_a_bus_or_master() {
    let mut s = mixer("bus-rack");
    let added = ok(&mut s, r#"{"cmd":"rack_add","type":"reverb","bus":"Verb"}"#);
    assert_eq!(added["id"], "reverb");
    assert!(swapped(&mut s), "rack_add on a Bus did not rebuild");
    ok(&mut s, r#"{"cmd":"rack_add","type":"gain","bus":0}"#);
    ok(
        &mut s,
        r#"{"cmd":"rack_add","type":"compressor","master":true}"#,
    );
    ok(&mut s, r#"{"cmd":"rack_move","id":"gain","to":0,"bus":0}"#);
    drain(&mut s);
    let set = ok(
        &mut s,
        r#"{"cmd":"set","param":"reverb.mix","value":0.5,"bus":"Verb"}"#,
    );
    assert_eq!(set["value"], 0.5);
    let mix = ok(&mut s, r#"{"cmd":"get","param":"reverb.mix","bus":0}"#);
    assert_eq!(mix["params"][0]["value"], 0.5);
    ok(&mut s, r#"{"cmd":"bypass","id":"gain","on":true,"bus":0}"#);
    ok(
        &mut s,
        r#"{"cmd":"set","param":"compressor.threshold","value":-20,"master":true}"#,
    );
    ok(
        &mut s,
        r#"{"cmd":"bypass","id":"compressor","on":true,"master":true}"#,
    );
    let commands = arrangement_commands(&mut s);
    assert_eq!(commands.len(), 4, "{commands:?}");
    assert!(commands[0].starts_with("BusParam { bus: 0, module: 1, param: "));
    assert_eq!(commands[1], "BusBypass { bus: 0, module: 0, on: true }");
    assert!(commands[2].starts_with("MasterParam { module: 0, param: "));
    assert_eq!(commands[3], "MasterBypass { module: 0, on: true }");
    ok(&mut s, r#"{"cmd":"rack_remove","id":"gain","bus":0}"#);
    let project = saved(&mut s);
    let bus = &project.buses[0].modules;
    assert_eq!(bus.len(), 1);
    assert_eq!(bus[0].type_name, "reverb");
    let master = &project.master.modules;
    assert_eq!(master[0].type_name, "compressor");
    assert!(master[0].bypass);
    assert!(project.tracks.iter().all(|track| track.modules.len() == 1));
    let (_, live) = s.state.active.as_ref().unwrap();
    assert_eq!(live.modules.len(), 1);
}

#[test]
fn rack_request_targets_are_checked() {
    let mut s = session("rack-targets");
    assert_eq!(error(&mut s, r#"{"cmd":"get","bus":0}"#), "no project open");
    assert_eq!(
        error(&mut s, r#"{"cmd":"rack_add","type":"gain","master":true}"#),
        "no project open"
    );
    let mut s = mixer("rack-targets-open");
    for line in [
        r#"{"cmd":"get","track":0,"bus":0}"#,
        r#"{"cmd":"set","param":"gain.level","value":1,"bus":0,"master":true}"#,
        r#"{"cmd":"rack_remove","id":"gain","track":"Guitar","master":true}"#,
        r#"{"cmd":"load","preset":"Clean","track":0,"bus":0}"#,
    ] {
        assert_eq!(error(&mut s, line), ONE_TARGET, "{line}");
    }
    assert_eq!(
        error(&mut s, r#"{"cmd":"rack_add","type":"cabinet","bus":0}"#),
        CABINET
    );
    assert_eq!(
        error(
            &mut s,
            r#"{"cmd":"rack_add","type":"cabinet","master":true}"#
        ),
        CABINET
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"rack_ir","ir":"cab.wav","master":true}"#),
        CABINET
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"load","preset":"Clean","bus":"Verb"}"#),
        "a Preset loads only into a Track"
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"load","preset":"Clean","master":true}"#),
        "a Preset loads only into a Track"
    );
    assert_eq!(
        error(
            &mut s,
            r#"{"cmd":"bypass","id":"gain","on":true,"bus":"Echo"}"#
        ),
        "no bus 'Echo'"
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"get","param":"gain","bus":0}"#),
        "no module 'gain'"
    );
    assert_eq!(arrangement_commands(&mut s), Vec::<String>::new());
}

#[test]
fn a_stereo_rack_has_no_layout_rule() {
    let mut s = mixer("stereo-layout");
    ok(&mut s, r#"{"cmd":"rack_add","type":"reverb","bus":0}"#);
    ok(&mut s, r#"{"cmd":"rack_add","type":"drive","bus":0}"#);
    let types: Vec<String> = saved(&mut s).buses[0]
        .modules
        .iter()
        .map(|module| module.type_name.clone())
        .collect();
    assert_eq!(types, ["reverb", "drive"]);
}

#[test]
fn status_reports_buses_master_sends_and_meters() {
    let mut s = session("status");
    let idle = ok(&mut s, r#"{"cmd":"status"}"#);
    assert_eq!(idle["buses"], json!([]));
    assert_eq!(idle["master"], Value::Null);
    let mut s = mixer("status-open");
    ok(
        &mut s,
        r#"{"cmd":"send_set","track":1,"bus":0,"level":-12,"pre":true}"#,
    );
    ok(&mut s, r#"{"cmd":"master_set","volume":-1.5}"#);
    let meters = &s.state.shared.mix_meters;
    meters.raise(MeterPoint::Track(1), [0.5, 0.25]);
    meters.raise(MeterPoint::Bus(0), [1.0, 0.5]);
    meters.raise(MeterPoint::Master, [0.5, 1.2]);
    let status = ok(&mut s, r#"{"cmd":"status"}"#);
    assert_eq!(
        status["tracks"][1],
        json!({"name": "Keys", "input": null, "volume_db": 0.0, "pan": 0.0, "mute": false, "solo": false, "armed": false, "input_peak_db": -120.0, "to_master": true, "sends": [{"bus": 0, "level_db": -12.0, "pre": true}], "peak_db": [-6.0, -12.0], "clip": false})
    );
    assert_eq!(status["tracks"][0]["peak_db"], json!([-120.0, -120.0]));
    assert_eq!(
        status["buses"],
        json!([{"id": 0, "name": "Verb", "volume_db": 0.0, "pan": 0.0, "mute": false, "solo": false, "peak_db": [0.0, -6.0], "clip": true}])
    );
    assert_eq!(
        status["master"],
        json!({"volume_db": -1.5, "peak_db": [-6.0, 1.6], "clip": true})
    );
    let again = ok(&mut s, r#"{"cmd":"status"}"#);
    assert_eq!(again["master"]["peak_db"], json!([-120.0, -120.0]));
    assert_eq!(again["master"]["clip"], true, "the clip flag did not latch");
}

#[test]
fn clip_reset_clears_one_meter_point_or_all() {
    let mut s = mixer("clip-reset");
    let meters = Arc::clone(&s.state.shared);
    let points = [
        MeterPoint::Track(0),
        MeterPoint::Track(1),
        MeterPoint::Bus(0),
        MeterPoint::Master,
    ];
    let clip_all = || {
        for point in points {
            meters.mix_meters.raise(point, [1.0, 1.0]);
        }
    };
    let clips = || points.map(|point| meters.mix_meters.clip(point));
    clip_all();
    let before = revision(&s.state);
    assert_eq!(
        ok(&mut s, r#"{"cmd":"clip_reset","track":"Keys"}"#),
        json!({"ok": true, "track": 1})
    );
    assert_eq!(clips(), [true, false, true, true]);
    assert_eq!(
        ok(&mut s, r#"{"cmd":"clip_reset","bus":"Verb"}"#),
        json!({"ok": true, "bus": 0})
    );
    assert_eq!(clips(), [true, false, false, true]);
    assert_eq!(
        ok(&mut s, r#"{"cmd":"clip_reset","master":true}"#),
        json!({"ok": true, "master": true})
    );
    assert_eq!(clips(), [true, false, false, false]);
    clip_all();
    assert_eq!(ok(&mut s, r#"{"cmd":"clip_reset"}"#), json!({"ok": true}));
    assert_eq!(clips(), [false; 4]);
    assert_eq!(
        revision(&s.state),
        before,
        "clip_reset changed the Revision"
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"clip_reset","track":0,"master":true}"#),
        ONE_TARGET
    );
    assert_eq!(
        error(&mut s, r#"{"cmd":"clip_reset","bus":4}"#),
        "no bus '4'"
    );
    let mut closed = session("clip-reset-closed");
    assert_eq!(
        error(&mut closed, r#"{"cmd":"clip_reset","master":true}"#),
        "no project open"
    );
    ok(&mut closed, r#"{"cmd":"clip_reset"}"#);
}

/// The Engine's audio side on `s`'s queues; `s` drains nothing after this.
fn audio_switcher(s: &mut Session) -> RackSwitcher {
    let (_, unused, _) = engine_queues(4);
    let audio = std::mem::replace(&mut s.audio, unused);
    let rack = Box::new(passthrough_rack(48_000.0, BLOCK));
    let fades = FadeLengths::at(48_000.0, 20.0);
    RackSwitcher::new(rack, BLOCK, fades, audio, Arc::clone(&s.state.shared))
}

fn run_blocks(switcher: &mut RackSwitcher, inputs: &[&[f32]], blocks: usize) {
    let silence = [0.0; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    for _ in 0..blocks {
        switcher.apply_commands();
        switcher.process_tracks(&silence, inputs, &mut left, &mut right);
    }
}

#[test]
fn a_clip_of_the_fading_out_arrangement_does_not_stay_on_a_moved_index() {
    let mut s = mixer("clip-fade");
    ok(&mut s, r#"{"cmd":"bus_add","name":"Delay"}"#);
    ok(&mut s, r#"{"cmd":"track_set","track":"Keys","arm":true}"#);
    ok(
        &mut s,
        r#"{"cmd":"send_set","track":"Keys","bus":"Delay","level":0}"#,
    );
    let mut switcher = audio_switcher(&mut s);
    let (silence, loud) = ([0.0; BLOCK], [1.5; BLOCK]);
    let inputs: [&[f32]; 2] = [&silence, &loud];
    run_blocks(&mut switcher, &inputs, FADE_BLOCKS);
    let meters = &s.state.shared.mix_meters;
    assert!(meters.clip(MeterPoint::Track(1)) && meters.clip(MeterPoint::Bus(1)));
    meters.raise(MeterPoint::Track(0), [1.0, 1.0]);
    meters.raise(MeterPoint::Bus(0), [1.0, 1.0]);
    ok(&mut s, r#"{"cmd":"track_remove","track":"Guitar"}"#);
    ok(&mut s, r#"{"cmd":"bus_remove","bus":"Verb"}"#);
    run_blocks(&mut switcher, &inputs, 3 * FADE_BLOCKS);
    let meters = &s.state.shared.mix_meters;
    for point in [
        MeterPoint::Track(0),
        MeterPoint::Track(1),
        MeterPoint::Bus(0),
        MeterPoint::Bus(1),
    ] {
        assert!(!meters.clip(point), "{point:?} kept the old layout's clip");
    }
}

#[test]
fn mixer_changes_mark_the_project_for_autosave() {
    let mut s = mixer("autosave");
    let later = Instant::now() + Duration::from_secs(61);
    ok(&mut s, r#"{"cmd":"project_save"}"#);
    assert!(autosave_if_due(&mut s.state, later).is_none());
    ok(&mut s, r#"{"cmd":"master_set","volume":-2}"#);
    assert!(autosave_if_due(&mut s.state, later).is_some_and(|saved| saved.is_ok()));
    let folder = s.state.config.projects_dir.join("song");
    assert!(folder.join(PROJECT_FILE).is_file());
}

#[test]
fn mixer_requests_write_the_documented_lines() {
    let send = Request::SendSet {
        track: TrackRef::Index(0),
        bus: BusRef::Name("Verb".to_string()),
        level: Some(SendLevel::Off),
        pre: None,
    };
    assert_eq!(
        send.to_line(),
        r#"{"cmd":"send_set","track":0,"bus":"Verb","level":"off","pre":null}"#
    );
    let get = Request::Get {
        param: None,
        track: None,
        bus: None,
        master: true,
    };
    assert_eq!(get.to_line(), r#"{"cmd":"get","param":null,"master":true}"#);
    let reset = Request::ClipReset {
        track: None,
        bus: Some(BusRef::Index(2)),
        master: false,
    };
    assert_eq!(reset.to_line(), r#"{"cmd":"clip_reset","bus":2}"#);
    let level = Request::SendSet {
        track: TrackRef::Index(1),
        bus: BusRef::Index(0),
        level: Some(SendLevel::Db(-6.0)),
        pre: Some(true),
    };
    assert_eq!(
        level.to_line(),
        r#"{"cmd":"send_set","track":1,"bus":0,"level":-6.0,"pre":true}"#
    );
}
