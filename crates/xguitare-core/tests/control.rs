use std::collections::HashMap;
use std::f32::consts::TAU;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use xguitare_core::control::{
    EngineState, PeakMeters, Reprepare, Request, Response, change_sample_rate, follow_sample_rate,
    handle, handle_line, needs_reprepare, open_connection, passthrough_rack, prepared_max_frames,
    reprepare, write_new,
};
use xguitare_core::engine::{
    AudioQueues, Command, EngineShared, FadeLengths, RackSwitcher, SWITCHER_MAX_FRAMES,
    engine_queues, raise_peak,
};
use xguitare_core::project_control::Projects;
use xguitare_core::tuner::Tuner;
use xguitare_core::watch::apply;
use xguitare_core::{Bank, BankEntry, BuildWarning, Config, ModuleRegistry, Preset, PresetModule};

const QUEUE_CAPACITY: usize = 64;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn bank_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-control-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("01-clean.toml"),
        "name = \"Clean\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = -3\n",
    )
    .unwrap();
    fs::write(
        dir.join("02-loud.toml"),
        "name = \"Loud\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 6\n",
    )
    .unwrap();
    fs::write(dir.join("03-broken.toml"), "name = \n").unwrap();
    TempDir(dir)
}

fn state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let dir = bank_dir(test);
    let bank = Bank::load_dir(&dir.0).unwrap();
    let BankEntry::Valid { file, preset } = &bank.entries[0] else {
        panic!("01-clean.toml should be valid");
    };
    let active = Some((file.clone(), preset.clone()));
    let shared = Arc::new(EngineShared::default());
    shared.sample_rate.store(48_000, Ordering::Relaxed);
    shared.buffer_size.store(128, Ordering::Relaxed);
    shared.xruns.store(3, Ordering::Relaxed);
    shared.callback_ns_max.store(250_000, Ordering::Relaxed);
    shared.callback_ns_last.store(90_000, Ordering::Relaxed);
    let mut config = empty_config(&dir.0);
    config.bank_dir.clone_from(&dir.0);
    let (queues, audio, tap) = engine_queues(QUEUE_CAPACITY);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state = EngineState {
        bank,
        active,
        shared,
        queues,
        tuner,
        config,
        connections: vec![("system:capture_1".to_string(), "xguitare:in".to_string())],
        sample_rate: 48_000.0,
        max_frames: 128,
        revision: 1,
        modified: false,
        peaks: PeakMeters::default(),
        saved: HashMap::new(),
        projects: Projects::default(),
    };
    (state, audio, dir)
}

fn active_file(state: &EngineState) -> Option<PathBuf> {
    state.active.as_ref().map(|(file, _)| file.clone())
}

fn remove_active_file_and_reindex(state: &mut EngineState) {
    fs::remove_file(state.config.bank_dir.join("01-clean.toml")).unwrap();
    state.bank = Bank::load_dir(&state.config.bank_dir).unwrap();
}

fn tmp_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
        .collect()
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

fn bypass(id: &str, on: bool) -> Request {
    Request::Bypass {
        id: id.to_string(),
        on,
        track: None,
        bus: None,
        master: false,
    }
}

fn set(param: &str, value: f64) -> Request {
    Request::Set {
        param: param.to_string(),
        value,
        track: None,
        bus: None,
        master: false,
    }
}

#[test]
fn status_reports_fields() {
    let (mut state, _audio, _dir) = state("status");
    let payload = ok(handle(&mut state, Request::Status));
    assert_eq!(
        payload,
        json!({
            "preset": "Clean",
            "program": 0,
            "sample_rate": 48000,
            "buffer_size": 128,
            "prepared_frames": 128,
            "revision": 1,
            "muted": false,
            "modified": false,
            "input_peak_db": -120.0,
            "output_peak_db": -120.0,
            "xruns": 3,
            "callback_max_us": 250,
            "callback_last_us": 90,
            "connections": [{"from": "system:capture_1", "to": "xguitare:in"}],
            "project": null,
            "project_warning": null,
            "render": null,
            "transport": null,
            "position": null,
            "tempo": null,
            "loop": null,
            "click": null,
            "selected": null,
            "tracks": [],
            "buses": [],
            "master": null,
            "disk_overflows": 0,
            "count_in_remaining": 0,
            "loaded_audio_bytes": 0,
        })
    );
}

#[test]
fn list_marks_active_and_invalid() {
    let (mut state, _audio, _dir) = state("list");
    let payload = ok(handle(&mut state, Request::List));
    assert_eq!(payload["active"], 0);
    let entries = payload["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[1],
        json!({"program": 1, "name": "Loud", "valid": true})
    );
    assert_eq!(entries[2]["name"], "03-broken");
    assert_eq!(entries[2]["valid"], false);
    assert!(
        entries[2]["error"].as_str().unwrap().starts_with("line 1:"),
        "{}",
        entries[2]
    );
}

#[test]
fn load_returns_build_warnings() {
    let (mut state, _audio, _dir) = state("load-warnings");
    fs::write(
        state.config.bank_dir.join("04-cab.toml"),
        "name = \"Cab\"\n[[module]]\ntype = \"cabinet\"\nir = \"missing.wav\"\n",
    )
    .unwrap();
    state.bank = Bank::load_dir(&state.config.bank_dir).unwrap();
    let (response, warnings) = handle(
        &mut state,
        Request::Load {
            preset: "Cab".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    );
    assert!(matches!(response, Response::Ok(_)));
    assert!(
        matches!(&warnings[..], [BuildWarning::Ir { id, .. }] if id == "cabinet"),
        "{warnings:?}"
    );
}

#[test]
#[allow(clippy::float_cmp, reason = "the test sets this exact value")]
fn load_by_name_and_number() {
    let (mut state, mut audio, _dir) = state("load");
    let payload = ok(handle(
        &mut state,
        Request::Load {
            preset: "Loud".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(payload, json!({"program": 1, "name": "Loud"}));
    assert!(matches!(
        audio.commands.pop(),
        Ok(Command::SwapRack(rack)) if rack.modules()[0].1.param(0) == 6.0
    ));
    let payload = ok(handle(
        &mut state,
        Request::Load {
            preset: "0".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(payload, json!({"program": 0, "name": "Clean"}));
    assert!(matches!(audio.commands.pop(), Ok(Command::SwapRack(_))));
    assert_eq!(
        active_file(&state),
        Some(state.config.bank_dir.join("01-clean.toml"))
    );
}

#[test]
fn load_invalid_entry_is_error() {
    let (mut state, audio, _dir) = state("load-invalid");
    let message = error(handle(
        &mut state,
        Request::Load {
            preset: "2".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert!(message.starts_with("line 1:"), "{message}");
    assert!(audio.commands.is_empty());
    assert_eq!(
        active_file(&state),
        Some(state.config.bank_dir.join("01-clean.toml"))
    );
}

#[test]
#[allow(clippy::float_cmp, reason = "the test sets this exact value")]
fn set_clamps_and_reports() {
    let (mut state, mut audio, _dir) = state("set");
    let payload = ok(handle(&mut state, set("gain.level", 50.0)));
    assert_eq!(
        payload,
        json!({"param": "gain.level", "value": 20.0, "clamped": true})
    );
    assert!(matches!(
        audio.commands.pop(),
        Ok(Command::SetParam { module: 0, param: 0, value }) if value == 20.0
    ));
    let (_, preset) = state.active.as_ref().unwrap();
    assert_eq!(preset.modules[0].params[0].1, 20.0);
    let payload = ok(handle(&mut state, set("gain.level", -6.0)));
    assert_eq!(payload["clamped"], false);
}

#[test]
#[allow(clippy::float_cmp, reason = "the test sets this exact value")]
fn set_rounds_integer_param() {
    let (mut state, mut audio, _dir) = amp_state("set-rounds-integer");
    let (_, preset) = state.active.as_mut().unwrap();
    preset.modules[1].params = vec![
        ("level".to_string(), 0.0),
        ("low_cut".to_string(), 20.0),
        ("high_cut".to_string(), 20000.0),
    ];
    let payload = ok(handle(&mut state, set("cabinet.low_cut", 150.4)));
    assert_eq!(
        payload,
        json!({"param": "cabinet.low_cut", "value": 150.0, "clamped": false})
    );
    assert!(matches!(
        audio.commands.pop(),
        Ok(Command::SetParam { module: 1, param: 1, value }) if value == 150.0
    ));
    ok(handle(&mut state, Request::Save { name: None }));
    let file = state.config.bank_dir.join("01-clean.toml");
    let saved = Preset::parse(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved.modules[1].params[1], ("low_cut".to_string(), 150.0));
}

#[test]
fn set_non_finite_is_error() {
    let (mut state, audio, _dir) = state("set-non-finite");
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            error(handle(&mut state, set("gain.level", value))),
            "value must be a finite number"
        );
    }
    assert!(audio.commands.is_empty());
}

#[test]
fn set_unknown_module() {
    let (mut state, _audio, _dir) = state("set-unknown-module");
    assert_eq!(
        error(handle(&mut state, set("drive.gain", 1.0))),
        "no module 'drive'"
    );
}

#[test]
fn set_unknown_param() {
    let (mut state, _audio, _dir) = state("set-unknown-param");
    assert_eq!(
        error(handle(&mut state, set("gain.drive", 1.0))),
        "module 'gain' has no parameter 'drive'"
    );
}

#[test]
fn set_bad_address() {
    let (mut state, _audio, _dir) = state("set-bad-address");
    assert_eq!(
        error(handle(&mut state, set("gain", 1.0))),
        "expected <module>.<param>, got 'gain'"
    );
}

#[test]
fn set_without_preset() {
    let (mut state, _audio, _dir) = state("set-without-preset");
    state.active = None;
    assert_eq!(
        error(handle(&mut state, set("gain.level", 1.0))),
        "no preset loaded"
    );
}

#[test]
fn get_all_and_one() {
    let (mut state, _audio, _dir) = state("get");
    let expected = json!({"module": "gain", "type": "gain", "param": "level", "value": -3.0, "unit": "dB", "min": -60.0, "max": 20.0, "default": 0.0, "integer": false, "bypass": false});
    let all = ok(handle(
        &mut state,
        Request::Get {
            param: None,
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(all, json!({"params": [expected]}));
    let module = ok(handle(
        &mut state,
        Request::Get {
            param: Some("gain".to_string()),
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(module, all);
    let one = ok(handle(
        &mut state,
        Request::Get {
            param: Some("gain.level".to_string()),
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(one, all);
    assert_eq!(
        error(handle(
            &mut state,
            Request::Get {
                param: Some("delay".to_string()),
                track: None,
                bus: None,
                master: false,
            }
        )),
        "no module 'delay'"
    );
}

#[test]
fn get_carries_integer() {
    let (mut state, _audio, dir) = state("integer");
    fs::write(
        dir.0.join("04-hot.toml"),
        "name = \"Hot\"\n[[module]]\ntype = \"preamp\"\n",
    )
    .unwrap();
    state.bank = Bank::load_dir(&dir.0).unwrap();
    ok(handle(
        &mut state,
        Request::Load {
            preset: "Hot".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    let all = ok(handle(
        &mut state,
        Request::Get {
            param: None,
            track: None,
            bus: None,
            master: false,
        },
    ));
    let integer: Vec<(&str, bool)> = all["params"]
        .as_array()
        .unwrap()
        .iter()
        .map(|param| {
            (
                param["param"].as_str().unwrap(),
                param["integer"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        integer,
        [
            ("gain", false),
            ("bias", false),
            ("master", false),
            ("stages", true)
        ]
    );
}

#[test]
fn queue_full_is_engine_busy() {
    let (mut state, audio, _dir) = state("busy");
    for _ in 0..QUEUE_CAPACITY {
        ok(handle(&mut state, set("gain.level", 1.0)));
    }
    assert_eq!(
        error(handle(&mut state, set("gain.level", 2.0))),
        "engine busy"
    );
    assert_eq!(
        error(handle(
            &mut state,
            Request::Load {
                preset: "1".to_string(),
                track: None,
                bus: None,
                master: false,
            }
        )),
        "engine busy"
    );
    assert_eq!(
        active_file(&state),
        Some(state.config.bank_dir.join("01-clean.toml"))
    );
    assert_eq!(audio.commands.slots(), QUEUE_CAPACITY);
}

#[test]
fn unknown_cmd_is_error() {
    let (mut state, _audio, _dir) = state("unknown-cmd");
    let connection = open_connection(&mut state);
    let response: Value =
        serde_json::from_str(&handle_line(&mut state, None, connection, r#"{"cmd":"bogus"}"#).0)
            .unwrap();
    assert_eq!(response["ok"], false);
    let message = response["error"].as_str().unwrap();
    assert!(
        message.starts_with("invalid request: unknown variant `bogus`"),
        "{message}"
    );
}

#[test]
fn malformed_json_is_error() {
    let (mut state, _audio, _dir) = state("malformed");
    let connection = open_connection(&mut state);
    let response: Value =
        serde_json::from_str(&handle_line(&mut state, None, connection, "{\"cmd\":").0).unwrap();
    assert_eq!(response["ok"], false);
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid request: "),
        "{response}"
    );
    let response: Value =
        serde_json::from_str(&handle_line(&mut state, None, connection, r#"{"cmd":"status"}"#).0)
            .unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(response["preset"], "Clean");
}

#[test]
fn save_writes_live_values_atomically() {
    let (mut state, _audio, _dir) = state("save");
    ok(handle(&mut state, set("gain.level", -6.0)));
    let payload = ok(handle(&mut state, Request::Save { name: None }));
    let file = state.config.bank_dir.join("01-clean.toml");
    assert_eq!(payload, json!({"path": file.display().to_string()}));
    let saved = Preset::parse(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved.modules[0].params[0], ("level".to_string(), -6.0));
    let leftovers = tmp_files(&state.config.bank_dir);
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn save_keeps_mode() {
    let (mut state, _audio, _dir) = state("save-keeps-mode");
    let file = state.config.bank_dir.join("01-clean.toml");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    ok(handle(&mut state, Request::Save { name: None }));
    assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn save_through_symlink() {
    let (mut state, _audio, dir) = state("save-through-symlink");
    let link = state.config.bank_dir.join("01-clean.toml");
    let elsewhere = dir.0.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let target = elsewhere.join("clean.toml");
    fs::rename(&link, &target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    ok(handle(&mut state, set("gain.level", -6.0)));
    ok(handle(&mut state, Request::Save { name: None }));
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let saved = Preset::parse(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(saved.modules[0].params[0], ("level".to_string(), -6.0));
    assert_eq!(tmp_files(&elsewhere), Vec::<PathBuf>::new());
}

const SAVES_IN_THIS_BINARY_BOUND: u64 = 1024;

#[test]
fn save_does_not_follow_planted_tmp_link() {
    let (mut state, _audio, dir) = state("save-planted-link");
    let victim = dir.0.join("victim.txt");
    fs::write(&victim, "keep").unwrap();
    let victim_mode = fs::metadata(&victim).unwrap().mode();
    // White-box on purpose: plants the private temp name `<file>.<pid>.<n>.tmp`. `n` counts
    // saves across this whole test binary, so every value it can reach is planted.
    for n in 0..SAVES_IN_THIS_BINARY_BOUND {
        let name = format!("01-clean.toml.{}.{n}.tmp", std::process::id());
        std::os::unix::fs::symlink(&victim, state.config.bank_dir.join(name)).unwrap();
    }
    let message = error(handle(&mut state, Request::Save { name: None }));
    assert!(message.starts_with("cannot save "), "{message}");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    assert_eq!(fs::metadata(&victim).unwrap().mode(), victim_mode);
}

#[test]
fn save_without_preset_is_error() {
    let (mut state, _audio, _dir) = state("save-without-preset");
    state.active = None;
    assert_eq!(
        error(handle(&mut state, Request::Save { name: None })),
        "no preset loaded"
    );
}

#[test]
fn save_after_active_removed_is_error() {
    let (mut state, _audio, _dir) = state("save-after-removal");
    remove_active_file_and_reindex(&mut state);
    let loud = fs::read_to_string(state.config.bank_dir.join("02-loud.toml")).unwrap();
    assert_eq!(
        error(handle(&mut state, Request::Save { name: None })),
        "active preset file was removed; load a preset first"
    );
    assert!(!state.config.bank_dir.join("01-clean.toml").exists());
    assert_eq!(
        fs::read_to_string(state.config.bank_dir.join("02-loud.toml")).unwrap(),
        loud
    );
    assert_eq!(tmp_files(&state.config.bank_dir), Vec::<PathBuf>::new());
}

#[test]
fn status_program_null_after_removal() {
    let (mut state, _audio, _dir) = state("status-after-removal");
    remove_active_file_and_reindex(&mut state);
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["program"], Value::Null);
    assert_eq!(status["preset"], "Clean");
    let list = ok(handle(&mut state, Request::List));
    assert_eq!(list["active"], Value::Null);
}

#[test]
fn failed_save_leaves_no_tmp() {
    let (mut state, _audio, _dir) = state("save-fails");
    let target = state.config.bank_dir.join("01-clean.toml");
    fs::remove_file(&target).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("occupied"), "").unwrap();
    let message = error(handle(&mut state, Request::Save { name: None }));
    assert!(message.starts_with("cannot save"), "{message}");
    assert_eq!(tmp_files(&state.config.bank_dir), Vec::<PathBuf>::new());
}

#[test]
fn needs_reprepare_when_sizes_differ() {
    assert!(needs_reprepare(128, 256));
    assert!(needs_reprepare(512, 128));
    assert!(!needs_reprepare(128, 128));
}

#[test]
fn prepared_max_frames_is_larger_of_requested_and_effective() {
    assert_eq!(prepared_max_frames(128, 256), 256);
    assert_eq!(prepared_max_frames(512, 128), 512);
    assert_eq!(prepared_max_frames(128, 128), 128);
}

#[test]
fn reprepare_swaps_in_active_preset_at_new_size() {
    let (mut state, mut audio, _dir) = state("reprepare");
    assert_eq!(
        reprepare(&mut state, 256).unwrap(),
        Reprepare::Built(Vec::new())
    );
    let Ok(Command::SwapRack(rack)) = audio.commands.pop() else {
        panic!("expected SwapRack");
    };
    assert_eq!(rack.max_frames(), 256);
    assert_eq!(rack.modules()[0].0, "gain");
}

#[test]
fn notified_rate_equal_to_built_rate_rebuilds_nothing() {
    let (mut state, mut audio, _dir) = state("sample-rate-equal");
    assert_eq!(follow_sample_rate(&mut state), None);
    assert!(audio.commands.pop().is_err());
}

#[test]
fn notified_rate_change_rebuilds() {
    let (mut state, mut audio, _dir) = state("sample-rate-follow");
    state.shared.sample_rate.store(44_100, Ordering::Relaxed);
    assert_eq!(
        follow_sample_rate(&mut state),
        Some(Ok(Reprepare::Built(Vec::new())))
    );
    assert_eq!(state.sample_rate, 44_100.0);
    assert!(matches!(
        audio.commands.pop(),
        Ok(Command::SetFadeLengths(_))
    ));
    assert!(matches!(audio.commands.pop(), Ok(Command::SwapRack(_))));
}

#[test]
fn sample_rate_change_rebuilds() {
    let (mut state, mut audio, _dir) = state("sample-rate-change");
    state.shared.sample_rate.store(96_000, Ordering::Relaxed);
    assert_eq!(
        change_sample_rate(&mut state, 96_000).unwrap(),
        Reprepare::Built(Vec::new())
    );
    assert!(matches!(
        audio.commands.pop(),
        Ok(Command::SetFadeLengths(_))
    ));
    let Ok(Command::SwapRack(rack)) = audio.commands.pop() else {
        panic!("expected SwapRack");
    };
    assert_eq!(rack.modules()[0].0, "gain");
    assert_eq!(rack.max_frames(), 128);
    assert_eq!(
        ok(handle(&mut state, Request::Status))["sample_rate"],
        96_000
    );
    for n in 0..16384 {
        let sample = 0.5 * (TAU * 110.0 * n as f32 / 96_000.0).sin();
        assert!(audio.tap.push(sample).is_ok());
    }
    state.tuner.tick();
    let payload = ok(handle(&mut state, Request::Tuner));
    assert_eq!(payload["note"], "A2");
    assert_eq!(payload["hz"], 110.0);
}

fn run_blocks(switcher: &mut RackSwitcher, frames: usize) -> Vec<f32> {
    const BLOCK: usize = 128;
    let input = [0.5; BLOCK];
    let mut out = Vec::new();
    for _ in 0..frames / BLOCK {
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        switcher.apply_commands();
        switcher.process(&input, &mut left, &mut right);
        out.extend_from_slice(&left);
    }
    out
}

fn frames_to_settle(out: &[f32]) -> usize {
    let last = out[out.len() - 1];
    out.iter()
        .rposition(|&sample| (sample - last).abs() >= 1e-6)
        .map_or(0, |n| n + 1)
}

#[test]
fn rate_change_doubles_fade_frames() {
    let (mut state, audio, _dir) = state("rate-change-fades");
    let shared = Arc::clone(&state.shared);
    let rack = Box::new(passthrough_rack(48_000.0, 128));
    let fades = FadeLengths::at(48_000.0, state.config.crossfade_ms);
    let mut switcher = RackSwitcher::new(rack, 128, fades, audio, Arc::clone(&shared));
    let mute_ramp = |switcher: &mut RackSwitcher| {
        shared.muted.store(true, Ordering::Relaxed);
        let frames = frames_to_settle(&run_blocks(switcher, 4096));
        shared.muted.store(false, Ordering::Relaxed);
        run_blocks(switcher, 4096);
        frames
    };

    ok(handle(
        &mut state,
        Request::Load {
            preset: "Loud".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    let crossfade_48k = frames_to_settle(&run_blocks(&mut switcher, 8192));
    let mute_48k = mute_ramp(&mut switcher);
    state.shared.sample_rate.store(96_000, Ordering::Relaxed);
    change_sample_rate(&mut state, 96_000).unwrap();
    run_blocks(&mut switcher, 4096);
    ok(handle(
        &mut state,
        Request::Load {
            preset: "Clean".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    let crossfade_96k = frames_to_settle(&run_blocks(&mut switcher, 8192));
    let mute_96k = mute_ramp(&mut switcher);

    assert_eq!(crossfade_48k, 960);
    assert_eq!(crossfade_96k, 2 * crossfade_48k);
    assert!((479..=480).contains(&mute_48k), "{mute_48k}");
    assert!((959..=960).contains(&mute_96k), "{mute_96k}");
}

#[test]
fn reprepare_without_preset_swaps_in_passthrough() {
    let (mut state, mut audio, _dir) = state("reprepare-passthrough");
    state.active = None;
    reprepare(&mut state, 512).unwrap();
    let Ok(Command::SwapRack(rack)) = audio.commands.pop() else {
        panic!("expected SwapRack");
    };
    assert_eq!(rack.max_frames(), 512);
    assert!(rack.modules().is_empty());
}

#[test]
fn reprepare_with_full_queue_is_engine_busy() {
    let (mut state, _audio, _dir) = state("reprepare-busy");
    for _ in 0..QUEUE_CAPACITY {
        ok(handle(&mut state, set("gain.level", 1.0)));
    }
    assert_eq!(reprepare(&mut state, 256).unwrap_err(), "engine busy");
    assert_eq!(
        ok(handle(&mut state, Request::Status))["prepared_frames"],
        128
    );
}

#[test]
fn status_reports_prepared_frames() {
    let (mut state, _audio, _dir) = state("status-prepared");
    reprepare(&mut state, 256).unwrap();
    assert_eq!(
        ok(handle(&mut state, Request::Status))["prepared_frames"],
        256
    );
}

fn cabinet(id: &str) -> PresetModule {
    PresetModule {
        id: id.to_string(),
        type_name: "cabinet".to_string(),
        ir: None,
        bypass: false,
        params: Vec::new(),
    }
}

#[test]
fn build_failure_falls_back_to_passthrough() {
    let (mut state, mut audio, _dir) = state("reprepare-fallback");
    let file = active_file(&state).unwrap();
    let preset = Preset {
        name: "Two Cabs".to_string(),
        modules: vec![cabinet("cab1"), cabinet("cab2")],
    };
    state.active = Some((file, preset));
    assert_eq!(
        reprepare(&mut state, 256).unwrap(),
        Reprepare::Passthrough {
            preset: "Two Cabs".to_string(),
            error: "more than one cabinet".to_string(),
        }
    );
    let Ok(Command::SwapRack(rack)) = audio.commands.pop() else {
        panic!("expected SwapRack");
    };
    assert_eq!(rack.max_frames(), 256);
    assert!(rack.modules().is_empty());
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["preset"], Value::Null);
    assert_eq!(status["prepared_frames"], 256);
}

#[test]
fn quantum_above_limit_is_not_reprepared() {
    let (mut state, _audio, _dir) = state("reprepare-above-limit");
    let above = SWITCHER_MAX_FRAMES as u32 + 1;
    assert!(!needs_reprepare(128, above));
    assert!(needs_reprepare(128, SWITCHER_MAX_FRAMES as u32));
    state.shared.buffer_size.store(above, Ordering::Relaxed);
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["buffer_size"], above);
    assert_eq!(status["prepared_frames"], 128);
}

fn revision(state: &mut EngineState) -> Value {
    ok(handle(state, Request::Status))["revision"].clone()
}

#[test]
fn status_has_revision_and_peaks() {
    let (mut state, _audio, _dir) = state("status-revision-peaks");
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["revision"], 1);
    assert_eq!(status["input_peak_db"], -120.0);
    assert_eq!(status["output_peak_db"], -120.0);
}

#[test]
fn status_reports_peaks_since_last_read() {
    let (mut state, _audio, _dir) = state("status-peaks");
    raise_peak(&state.shared.input_peak_bits, 0.5);
    raise_peak(&state.shared.output_peak_bits, 1.0);
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["input_peak_db"], -6.0);
    assert_eq!(status["output_peak_db"], 0.0);
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["input_peak_db"], -120.0);
    assert_eq!(status["output_peak_db"], -120.0);
}

#[test]
fn revision_increments_on_load_set_save() {
    let (mut state, _audio, _dir) = state("revision-increments");
    assert_eq!(revision(&mut state), 1);
    ok(handle(
        &mut state,
        Request::Load {
            preset: "1".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(revision(&mut state), 2);
    ok(handle(&mut state, set("gain.level", -6.0)));
    assert_eq!(revision(&mut state), 3);
    ok(handle(&mut state, Request::Save { name: None }));
    assert_eq!(revision(&mut state), 4);
}

#[test]
fn revision_unchanged_on_failed_set() {
    let (mut state, _audio, _dir) = state("revision-failed-set");
    error(handle(&mut state, set("gain.nope", 1.0)));
    assert_eq!(revision(&mut state), 1);
}

#[test]
fn reprepare_bumps_revision() {
    let (mut state, _audio, _dir) = state("reprepare-revision");
    reprepare(&mut state, 256).unwrap();
    assert_eq!(revision(&mut state), 2);
    let file = active_file(&state).unwrap();
    state.active = Some((
        file,
        Preset {
            name: "Two Cabs".to_string(),
            modules: vec![cabinet("cab1"), cabinet("cab2")],
        },
    ));
    reprepare(&mut state, 512).unwrap();
    assert_eq!(revision(&mut state), 3);
}

fn empty_config(dir: &Path) -> Config {
    let path = dir.join("config");
    fs::write(&path, "").unwrap();
    Config::load(Some(&path)).unwrap()
}

fn amp_state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let (mut state, audio, dir) = state(test);
    let (file, clean) = state.active.clone().unwrap();
    let cab = PresetModule {
        ir: Some("default-cab.wav".to_string()),
        params: vec![("level".to_string(), 0.0)],
        ..cabinet("cabinet")
    };
    let preset = Preset {
        name: "Amp".to_string(),
        modules: vec![clean.modules[0].clone(), cab],
    };
    state.active = Some((file, preset));
    (state, audio, dir)
}

fn ids(state: &EngineState) -> Vec<String> {
    let (_, preset) = state.active.as_ref().unwrap();
    preset.modules.iter().map(|m| m.id.clone()).collect()
}

fn rack_add(type_name: &str, after: Option<&str>) -> Request {
    Request::RackAdd {
        r#type: type_name.to_string(),
        after: after.map(str::to_string),
        at: None,
        track: None,
        bus: None,
        master: false,
    }
}

fn rack_add_at(type_name: &str, at: usize) -> Request {
    Request::RackAdd {
        r#type: type_name.to_string(),
        after: None,
        at: Some(at),
        track: None,
        bus: None,
        master: false,
    }
}

fn rack_move(id: &str, to: usize) -> Request {
    Request::RackMove {
        id: id.to_string(),
        to,
        track: None,
        bus: None,
        master: false,
    }
}

fn rack_remove(id: &str) -> Request {
    Request::RackRemove {
        id: id.to_string(),
        track: None,
        bus: None,
        master: false,
    }
}

#[test]
fn modules_lists_registry() {
    let (mut state, _audio, _dir) = state("modules");
    let payload = ok(handle(&mut state, Request::Modules));
    let modules = payload["modules"].as_array().unwrap();
    let types: Vec<&str> = modules
        .iter()
        .map(|m| m["type"].as_str().unwrap())
        .collect();
    let registry: Vec<&str> = ModuleRegistry::default()
        .specs()
        .map(|spec| spec.type_name)
        .collect();
    assert_eq!(types, registry);
    assert!(types.contains(&"fuzz"), "fuzz missing from {types:?}");
    let by_type = |t: &str| modules.iter().find(|m| m["type"] == t).unwrap().clone();
    assert_eq!(by_type("gain")["channels"], "mono");
    assert_eq!(by_type("delay")["channels"], "stereo");
    assert_eq!(by_type("cabinet")["channels"], "mono_to_stereo");
    let stages = by_type("preamp")["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "stages")
        .unwrap()
        .clone();
    assert_eq!(stages["integer"], true);
    assert_eq!(
        by_type("gain")["params"],
        json!([{"name": "level", "min": -60.0, "max": 20.0, "default": 0.0, "unit": "dB", "integer": false}])
    );
}

#[test]
fn rack_add_appends_with_default_params() {
    let (mut state, mut audio, _dir) = amp_state("rack-add-append");
    let payload = ok(handle(&mut state, rack_add("delay", Some("cabinet"))));
    assert_eq!(payload, json!({"id": "delay"}));
    assert_eq!(ids(&state), ["gain", "cabinet", "delay"]);
    let spec = ModuleRegistry::default().spec("delay").unwrap();
    let defaults: Vec<(String, f32)> = spec
        .params
        .iter()
        .map(|p| (p.name.to_string(), p.default))
        .collect();
    let (_, preset) = state.active.as_ref().unwrap();
    assert_eq!(preset.modules[2].params, defaults);
    let Ok(Command::SwapRack(rack)) = audio.commands.pop() else {
        panic!("expected SwapRack");
    };
    assert_eq!(rack.modules().len(), 3);
}

#[test]
fn rack_add_after_inserts_at_position() {
    let (mut state, _audio, _dir) = amp_state("rack-add-after");
    ok(handle(&mut state, rack_add("gate", Some("gain"))));
    assert_eq!(ids(&state), ["gain", "gate", "cabinet"]);
}

#[test]
fn rack_add_at_inserts_at_index() {
    let (mut state, _audio, _dir) = amp_state("rack-add-at");
    ok(handle(&mut state, rack_add_at("gate", 0)));
    assert_eq!(ids(&state), ["gate", "gain", "cabinet"]);
    ok(handle(&mut state, rack_add_at("delay", 99)));
    assert_eq!(ids(&state), ["gate", "gain", "cabinet", "delay"]);
    let both = Request::RackAdd {
        r#type: "gate".to_string(),
        after: Some("gain".to_string()),
        at: Some(0),
        track: None,
        bus: None,
        master: false,
    };
    assert_eq!(error(handle(&mut state, both)), "give either after or at");
    assert_eq!(ids(&state), ["gate", "gain", "cabinet", "delay"]);
}

#[test]
fn rack_add_cabinet_picks_ir() {
    let (mut state, mut audio, dir) = state("rack-add-cabinet-ir");
    assert_eq!(
        error(handle(&mut state, rack_add("cabinet", None))),
        "cabinet needs an ir; add one in the preset file"
    );
    assert_eq!(ids(&state), ["gain"]);
    assert!(audio.commands.pop().is_err(), "SwapRack pushed");
    assert_eq!(revision(&mut state), 1);
    let cabinet_ir = |state: &EngineState| {
        let (_, preset) = state.active.as_ref().unwrap();
        preset.modules[1].ir.clone()
    };
    fs::write(dir.0.join("bass-cab.wav"), "").unwrap();
    ok(handle(&mut state, rack_add("cabinet", None)));
    assert_eq!(cabinet_ir(&state).as_deref(), Some("bass-cab.wav"));
    ok(handle(&mut state, rack_remove("cabinet")));
    fs::write(dir.0.join("default-cab.wav"), "").unwrap();
    ok(handle(&mut state, rack_add("cabinet", None)));
    assert_eq!(cabinet_ir(&state).as_deref(), Some("default-cab.wav"));
}

#[test]
fn rack_add_cabinet_default_ir_from_ir_dir() {
    let (mut state, _audio, dir) = state("rack-add-cabinet-ir-dir");
    state.config.ir_dir = dir.0.join("irs");
    write_impulse(&state.config.ir_dir.join("default-cab.wav"));
    ok(handle(&mut state, rack_add("cabinet", None)));
    let (_, preset) = state.active.as_ref().unwrap();
    assert_eq!(preset.modules[1].ir.as_deref(), Some("default-cab.wav"));
}

#[test]
fn rack_add_second_cabinet_is_layout_error() {
    let (mut state, _audio, _dir) = amp_state("rack-add-second-cabinet");
    assert_eq!(
        error(handle(&mut state, rack_add("cabinet", None))),
        "more than one cabinet"
    );
    assert_eq!(ids(&state), ["gain", "cabinet"]);
}

#[test]
fn rack_add_without_after_appends() {
    let (mut state, _audio, _dir) = amp_state("rack-add-without-after");
    ok(handle(&mut state, rack_add("delay", None)));
    assert_eq!(ids(&state), ["gain", "cabinet", "delay"]);
}

#[test]
fn rack_add_suffixes_duplicate_id() {
    let (mut state, _audio, _dir) = amp_state("rack-add-suffix");
    ok(handle(&mut state, rack_add("delay", Some("cabinet"))));
    let payload = ok(handle(&mut state, rack_add("delay", Some("delay"))));
    assert_eq!(payload, json!({"id": "delay-2"}));
    let payload = ok(handle(&mut state, rack_add("delay", Some("cabinet"))));
    assert_eq!(payload, json!({"id": "delay-3"}));
    assert_eq!(
        ids(&state),
        ["gain", "cabinet", "delay-3", "delay", "delay-2"]
    );
}

#[test]
fn rack_add_layout_error_changes_nothing() {
    let (mut state, mut audio, _dir) = amp_state("rack-add-layout");
    assert_eq!(
        error(handle(&mut state, rack_add("delay", Some("gain")))),
        "stereo module 'delay' before the cabinet"
    );
    assert_eq!(ids(&state), ["gain", "cabinet"]);
    assert!(audio.commands.pop().is_err(), "SwapRack pushed");
    assert_eq!(revision(&mut state), 1);
}

#[test]
fn rack_add_unknown_type_is_error() {
    let (mut state, _audio, _dir) = amp_state("rack-add-unknown");
    assert_eq!(
        error(handle(&mut state, rack_add("wah", None))),
        "unknown module type 'wah'"
    );
    assert_eq!(
        error(handle(&mut state, rack_add("gate", Some("nope")))),
        "no module 'nope'"
    );
    assert_eq!(ids(&state), ["gain", "cabinet"]);
}

#[test]
fn rack_remove_unknown_is_error() {
    let (mut state, mut audio, _dir) = amp_state("rack-remove-unknown");
    assert_eq!(
        error(handle(&mut state, rack_remove("nope"))),
        "no module 'nope'"
    );
    let payload = ok(handle(&mut state, rack_remove("gain")));
    assert_eq!(payload, json!({"id": "gain"}));
    assert_eq!(ids(&state), ["cabinet"]);
    assert!(matches!(audio.commands.pop(), Ok(Command::SwapRack(_))));
}

#[test]
fn rack_remove_layout_error_changes_nothing() {
    let (mut state, mut audio, _dir) = amp_state("rack-remove-layout");
    ok(handle(&mut state, rack_add("delay", Some("cabinet"))));
    while audio.commands.pop().is_ok() {}
    assert_eq!(
        error(handle(&mut state, rack_remove("cabinet"))),
        "stereo module 'delay' before the cabinet"
    );
    assert_eq!(ids(&state), ["gain", "cabinet", "delay"]);
    assert!(audio.commands.pop().is_err(), "SwapRack pushed");
    assert_eq!(revision(&mut state), 2);
}

#[test]
fn rack_move_clamps_and_reorders() {
    let (mut state, mut audio, _dir) = amp_state("rack-move");
    ok(handle(&mut state, rack_add("delay", Some("cabinet"))));
    ok(handle(&mut state, rack_add("reverb", Some("delay"))));
    while audio.commands.pop().is_ok() {}
    let payload = ok(handle(&mut state, rack_move("delay", 6)));
    assert_eq!(payload, json!({"id": "delay", "to": 3}));
    assert!(matches!(audio.commands.pop(), Ok(Command::SwapRack(_))));
    assert_eq!(ids(&state), ["gain", "cabinet", "reverb", "delay"]);
    assert_eq!(
        error(handle(&mut state, rack_move("delay", 1))),
        "stereo module 'delay' before the cabinet"
    );
    assert_eq!(ids(&state), ["gain", "cabinet", "reverb", "delay"]);
    assert_eq!(
        error(handle(&mut state, rack_move("nope", 0))),
        "no module 'nope'"
    );
}

#[test]
fn rack_edits_bump_revision() {
    let (mut state, _audio, _dir) = amp_state("rack-revision");
    ok(handle(&mut state, rack_add("delay", Some("cabinet"))));
    assert_eq!(revision(&mut state), 2);
    ok(handle(&mut state, rack_move("delay", 2)));
    assert_eq!(revision(&mut state), 3);
    ok(handle(&mut state, rack_remove("delay")));
    assert_eq!(revision(&mut state), 4);
    error(handle(&mut state, rack_remove("delay")));
    assert_eq!(revision(&mut state), 4);
}

#[test]
fn save_after_rack_edit_round_trips() {
    let (mut state, _audio, _dir) = amp_state("rack-save");
    ok(handle(&mut state, rack_add("delay", Some("cabinet"))));
    ok(handle(&mut state, rack_add("delay", Some("delay"))));
    ok(handle(&mut state, rack_add("gate", Some("gain"))));
    ok(handle(&mut state, Request::Save { name: None }));
    let file = state.config.bank_dir.join("01-clean.toml");
    let saved = Preset::parse(&fs::read_to_string(&file).unwrap()).unwrap();
    let (_, live) = state.active.as_ref().unwrap();
    assert_eq!(saved, *live);
    let types: Vec<&str> = saved.modules.iter().map(|m| m.type_name.as_str()).collect();
    assert_eq!(types, ["gain", "gate", "cabinet", "delay", "delay"]);
}

#[test]
fn rack_edit_without_preset_is_error() {
    let (mut state, _audio, _dir) = state("rack-without-preset");
    state.active = None;
    for req in [
        rack_add("gate", None),
        rack_remove("gain"),
        rack_move("gain", 0),
    ] {
        assert_eq!(error(handle(&mut state, req)), "no preset loaded");
    }
}

#[test]
fn tuner_null_before_tick() {
    let (mut state, _audio, _dir) = state("tuner-null");
    let connection = open_connection(&mut state);
    let (line, _) = handle_line(&mut state, None, connection, r#"{"cmd":"tuner"}"#);
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"ok": true, "hz": null, "note": null, "cents": null, "confident": false})
    );
}

#[test]
fn tuner_reports_a2_after_tick() {
    let (mut state, mut audio, _dir) = state("tuner-a2");
    for n in 0..8192 {
        let sample = 0.5 * (TAU * 110.0 * n as f32 / 48_000.0).sin();
        assert!(audio.tap.push(sample).is_ok());
    }
    state.tuner.tick();
    let payload = ok(handle(&mut state, Request::Tuner));
    assert_eq!(payload["note"], "A2");
    assert_eq!(payload["hz"], 110.0);
    assert!(payload["cents"].as_i64().unwrap().abs() <= 1, "{payload}");
    assert_eq!(payload["confident"], true);
}

#[test]
fn tuner_after_rate_change_ignores_old_rate_samples() {
    let (mut state, mut audio, _dir) = state("tuner-rate-change");
    for n in 0..8192 {
        let sample = 0.9 * (TAU * 110.0 * n as f32 / 48_000.0).sin();
        assert!(audio.tap.push(sample).is_ok());
    }
    state.shared.sample_rate.store(96_000, Ordering::Relaxed);
    change_sample_rate(&mut state, 96_000).unwrap();
    for n in 0..16384 {
        let sample = 0.5 * (TAU * 110.0 * n as f32 / 96_000.0).sin();
        assert!(audio.tap.push(sample).is_ok());
    }
    state.tuner.tick();
    assert_eq!(ok(handle(&mut state, Request::Tuner))["note"], "A2");
}

#[test]
fn mute_on_off_and_status() {
    let (mut state, _audio, _dir) = state("mute");
    let connection = open_connection(&mut state);
    let (line, _) = handle_line(&mut state, None, connection, r#"{"cmd":"mute","on":true}"#);
    assert_eq!(line, r#"{"ok":true,"muted":true}"#);
    assert!(state.shared.muted.load(Ordering::Relaxed));
    assert_eq!(ok(handle(&mut state, Request::Status))["muted"], true);
    let payload = ok(handle(&mut state, Request::Mute { on: false }));
    assert_eq!(payload, json!({"muted": false}));
    assert!(!state.shared.muted.load(Ordering::Relaxed));
    assert_eq!(ok(handle(&mut state, Request::Status))["muted"], false);
}

#[test]
fn mute_without_on_is_invalid_request() {
    let (mut state, _audio, _dir) = state("mute-no-on");
    let connection = open_connection(&mut state);
    for line in [r#"{"cmd":"mute"}"#, r#"{"cmd":"mute","on":"yes"}"#] {
        let response: Value =
            serde_json::from_str(&handle_line(&mut state, None, connection, line).0).unwrap();
        assert_eq!(response["ok"], false);
        let message = response["error"].as_str().unwrap();
        assert!(message.starts_with("invalid request: "), "{message}");
    }
    assert!(!state.shared.muted.load(Ordering::Relaxed));
}

#[test]
fn revision_unchanged_by_mute() {
    let (mut state, _audio, _dir) = state("mute-revision");
    let before = revision(&mut state);
    ok(handle(&mut state, Request::Mute { on: true }));
    ok(handle(&mut state, Request::Mute { on: false }));
    assert_eq!(revision(&mut state), before);
}

fn rack_ir(ir: &str, id: Option<&str>) -> Request {
    Request::RackIr {
        ir: ir.to_string(),
        id: id.map(str::to_string),
        track: None,
        bus: None,
        master: false,
    }
}

fn write_impulse(path: &Path) {
    write_wav(path, &[1.0, 0.5, 0.25]);
}

fn write_wav(path: &Path, samples: &[f32]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for sample in samples {
        writer.write_sample(*sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn ir_state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let (mut state, audio, dir) = amp_state(test);
    state.config.ir_dir = dir.0.join("irs");
    write_impulse(&state.config.ir_dir.join("cabs/x.wav"));
    (state, audio, dir)
}

fn active_ir(state: &EngineState) -> Option<String> {
    let (_, preset) = state.active.as_ref().unwrap();
    preset.modules[1].ir.clone()
}

#[test]
fn non_finite_ir_refused_by_rack_ir_warned_on_load() {
    let (mut state, _audio, _dir) = ir_state("rack-ir-non-finite");
    let nan = state.config.ir_dir.join("nan.wav");
    write_wav(&nan, &[0.5, f32::NAN]);
    let expected = "ir nan.wav: non-finite sample";
    assert_eq!(
        error(handle(&mut state, rack_ir("nan.wav", None))),
        expected
    );
    assert_eq!(active_ir(&state).as_deref(), Some("default-cab.wav"));
    fs::write(
        state.config.bank_dir.join("04-nan.toml"),
        "name = \"Nan\"\n[[module]]\ntype = \"cabinet\"\nir = \"nan.wav\"\n",
    )
    .unwrap();
    state.bank = Bank::load_dir(&state.config.bank_dir).unwrap();
    let (response, warnings) = handle(
        &mut state,
        Request::Load {
            preset: "Nan".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    );
    assert!(matches!(response, Response::Ok(_)));
    assert!(
        matches!(&warnings[..], [BuildWarning::Ir { error, .. }] if *error == expected),
        "{warnings:?}"
    );
}

#[test]
fn ir_list_recursive_sorted() {
    let (mut state, _audio, dir) = state("ir-list");
    let irs = dir.0.join("irs");
    for file in [
        "bass-cab.wav",
        "Loud.WAV",
        "celestion/v30.wav",
        "celestion/old/g12.wav",
        ".hidden/x.wav",
        ".dot.wav",
        "notes.txt",
    ] {
        let path = irs.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
    std::os::unix::fs::symlink(irs.join("bass-cab.wav"), irs.join("link.wav")).unwrap();
    state.config.ir_dir = irs.clone();
    assert_eq!(
        ok(handle(&mut state, Request::IrList)),
        json!({
            "ir_dir": irs.display().to_string(),
            "irs": [
                "Loud.WAV",
                "bass-cab.wav",
                "celestion/old/g12.wav",
                "celestion/v30.wav",
                "link.wav"
            ],
        })
    );
}

#[test]
fn ir_list_missing_dir_error() {
    let (mut state, _audio, dir) = state("ir-list-missing");
    state.config.ir_dir = dir.0.join("absent");
    assert_eq!(
        error(handle(&mut state, Request::IrList)),
        format!(
            "ir_dir {}: No such file or directory (os error 2)",
            state.config.ir_dir.display()
        )
    );
}

#[test]
fn rack_ir_swaps_and_bumps_revision() {
    let (mut state, mut audio, _dir) = ir_state("rack-ir");
    let payload = ok(handle(&mut state, rack_ir("cabs/x.wav", Some("cabinet"))));
    assert_eq!(payload, json!({"id": "cabinet", "ir": "cabs/x.wav"}));
    assert!(matches!(audio.commands.pop(), Ok(Command::SwapRack(_))));
    assert_eq!(active_ir(&state).as_deref(), Some("cabs/x.wav"));
    assert_eq!(revision(&mut state), 2);
}

#[test]
fn rack_ir_first_cabinet_when_id_omitted() {
    let (mut state, _audio, _dir) = ir_state("rack-ir-first");
    let connection = open_connection(&mut state);
    let (line, warnings) = handle_line(
        &mut state,
        None,
        connection,
        r#"{"cmd":"rack_ir","ir":"cabs/x.wav"}"#,
    );
    assert_eq!(warnings, []);
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"ok": true, "id": "cabinet", "ir": "cabs/x.wav"})
    );
    assert_eq!(active_ir(&state).as_deref(), Some("cabs/x.wav"));
}

#[test]
fn rack_ir_errors_verbatim() {
    let (mut state, _audio, _dir) = state("rack-ir-no-cabinet");
    assert_eq!(
        error(handle(&mut state, rack_ir("cabs/x.wav", None))),
        "no cabinet module"
    );
    let (mut state, mut audio, _dir) = ir_state("rack-ir-errors");
    ok(handle(&mut state, rack_add_at("gate", 0)));
    while audio.commands.pop().is_ok() {}
    let before = ok(handle(
        &mut state,
        Request::Get {
            param: None,
            track: None,
            bus: None,
            master: false,
        },
    ));
    let bad = state.config.ir_dir.join("bad.wav");
    fs::write(&bad, "").unwrap();
    let loader_error = xguitare_core::load_ir(&bad, "bad.wav", 48_000, 100.0)
        .unwrap_err()
        .to_string();
    for (req, expected) in [
        (
            rack_ir("cabs/x.wav", Some("x")),
            "no module 'x'".to_string(),
        ),
        (
            rack_ir("cabs/x.wav", Some("gate")),
            "module 'gate' is not a cabinet".to_string(),
        ),
        (
            rack_ir("../x.wav", None),
            "invalid ir name '../x.wav'".to_string(),
        ),
        (rack_ir("bad.wav", None), loader_error),
    ] {
        assert_eq!(error(handle(&mut state, req)), expected);
    }
    assert_eq!(
        ok(handle(
            &mut state,
            Request::Get {
                param: None,
                track: None,
                bus: None,
                master: false,
            }
        )),
        before
    );
    assert!(audio.commands.pop().is_err(), "SwapRack pushed");
    assert_eq!(revision(&mut state), 2);
}

#[test]
fn save_after_rack_ir_round_trips() {
    let (mut state, _audio, _dir) = ir_state("rack-ir-save");
    ok(handle(&mut state, rack_ir("cabs/x.wav", None)));
    ok(handle(&mut state, Request::Save { name: None }));
    let file = state.config.bank_dir.join("01-clean.toml");
    let saved = Preset::parse(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved.modules[1].ir.as_deref(), Some("cabs/x.wav"));
    let (_, live) = state.active.as_ref().unwrap();
    assert_eq!(saved, *live);
}

#[test]
fn get_cabinet_carries_ir() {
    let (mut state, _audio, _dir) = amp_state("get-cabinet-ir");
    let all = ok(handle(
        &mut state,
        Request::Get {
            param: None,
            track: None,
            bus: None,
            master: false,
        },
    ));
    let params = all["params"].as_array().unwrap();
    assert!(params.iter().any(|p| p["module"] == "cabinet"));
    for param in params {
        if param["module"] == "cabinet" {
            assert_eq!(param["ir"], "default-cab.wav", "{param}");
        } else {
            assert!(param.get("ir").is_none(), "{param}");
        }
    }
}

struct RestoreMode(PathBuf);

impl Drop for RestoreMode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn ir_list_skips_unreadable_subdir() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let (mut state, _audio, dir) = state("ir-list-unreadable");
    let irs = dir.0.join("irs");
    let locked = irs.join("locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(irs.join("a.wav"), "").unwrap();
    fs::write(locked.join("b.wav"), "").unwrap();
    let _restore = RestoreMode(locked.clone());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    state.config.ir_dir = irs;
    let is_root = fs::metadata("/proc/self").unwrap().uid() == 0;
    if is_root {
        return;
    }
    assert_eq!(
        ok(handle(&mut state, Request::IrList))["irs"],
        json!(["a.wav"])
    );
}

fn save_as(name: &str) -> Request {
    Request::Save {
        name: Some(name.to_string()),
    }
}

fn is_modified(state: &mut EngineState) -> bool {
    ok(handle(state, Request::Status))["modified"] == true
}

fn bank_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "toml"))
        .collect();
    names.sort();
    names
}

#[test]
fn modified_tracks_changes() {
    let (mut state, _audio, _dir) = ir_state("modified");
    assert!(!is_modified(&mut state));
    for req in [
        set("gain.level", -6.0),
        rack_add_at("gate", 0),
        rack_move("gate", 1),
        rack_remove("gate"),
        rack_ir("cabs/x.wav", None),
        bypass("gain", true),
    ] {
        ok(handle(&mut state, Request::Save { name: None }));
        assert!(!is_modified(&mut state));
        ok(handle(&mut state, req.clone()));
        assert!(is_modified(&mut state), "{req:?}");
    }
    ok(handle(
        &mut state,
        Request::Load {
            preset: "Loud".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert!(!is_modified(&mut state));
    ok(handle(&mut state, set("gain.level", -9.0)));
    let file = active_file(&state).unwrap();
    fs::write(
        &file,
        "name = \"Loud\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 1\n",
    )
    .unwrap();
    apply(&mut state, &[file]);
    assert_eq!(ok(handle(&mut state, Request::Status))["preset"], "Loud");
    assert!(!is_modified(&mut state));
}

#[test]
fn save_as_creates_and_loads() {
    let (mut state, _audio, _dir) = state("save-as");
    ok(handle(&mut state, set("gain.level", -6.0)));
    let before = revision(&mut state).as_u64().unwrap();
    let connection = open_connection(&mut state);
    let (line, _) = handle_line(
        &mut state,
        None,
        connection,
        r#"{"cmd":"save","as":"My Lead Tone!"}"#,
    );
    let file = state.config.bank_dir.join("04-my-lead-tone.toml");
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"ok": true, "path": file.display().to_string(), "program": 3})
    );
    let saved = Preset::parse(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved.name, "My Lead Tone!");
    assert_eq!(saved.modules[0].params[0], ("level".to_string(), -6.0));
    let original =
        Preset::parse(&fs::read_to_string(state.config.bank_dir.join("01-clean.toml")).unwrap())
            .unwrap();
    assert_eq!(original.modules[0].params[0], ("level".to_string(), -3.0));
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["preset"], "My Lead Tone!");
    assert_eq!(status["program"], 3);
    assert_eq!(status["modified"], false);
    assert!(status["revision"].as_u64().unwrap() > before);
    assert_eq!(
        ok(handle(&mut state, Request::List))["entries"][3]["name"],
        "My Lead Tone!"
    );
    assert_eq!(tmp_files(&state.config.bank_dir), Vec::<PathBuf>::new());
}

#[test]
fn save_as_rejects_empty_and_duplicate() {
    let (mut state, _audio, _dir) = state("save-as-rejects");
    let before = bank_files(&state.config.bank_dir);
    for name in ["", "   ", "!!!", "--"] {
        assert_eq!(
            error(handle(&mut state, save_as(name))),
            "preset name must not be empty",
            "{name:?}"
        );
    }
    assert_eq!(
        error(handle(&mut state, save_as("loud"))),
        "a preset named 'loud' already exists"
    );
    assert_eq!(
        error(handle(&mut state, save_as("CLEAN"))),
        "a preset named 'CLEAN' already exists"
    );
    assert_eq!(
        error(handle(&mut state, save_as(" Loud "))),
        "a preset named 'Loud' already exists"
    );
    assert_eq!(bank_files(&state.config.bank_dir), before);
    assert!(!is_modified(&mut state));
}

#[test]
fn save_as_strips_control_characters() {
    let (mut state, _audio, _dir) = state("save-as-control");
    assert_eq!(
        error(handle(&mut state, save_as("\u{7}"))),
        "preset name must not be empty"
    );
    assert_eq!(
        error(handle(&mut state, save_as(" \u{1b}\u{7f}\t "))),
        "preset name must not be empty"
    );
    let payload = ok(handle(&mut state, save_as("\u{1b}[31m Lead\u{7}\n\u{7f}")));
    let saved =
        Preset::parse(&fs::read_to_string(payload["path"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(saved.name, "[31m Lead");
    assert_eq!(
        ok(handle(&mut state, Request::Status))["preset"],
        "[31m Lead"
    );
}

#[test]
fn write_new_falls_back_when_hard_links_fail() {
    let dir = bank_dir("write-new-fallback");
    for (n, errno) in [1, 18, 95].into_iter().enumerate() {
        let file = dir.0.join(format!("0{n}-new.toml"));
        let refuse = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(errno));
        write_new(&file, "name = \"New\"\n", refuse).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "name = \"New\"\n",
            "{errno}"
        );
    }
    let denied = dir.0.join("09-denied.toml");
    let refuse = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(13));
    let err = write_new(&denied, "name = \"New\"\n", refuse).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(13));
    assert!(!denied.exists());
    let taken = dir.0.join("01-clean.toml");
    let before = fs::read_to_string(&taken).unwrap();
    let refuse = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(95));
    let err = write_new(&taken, "replaced", refuse).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&taken).unwrap(), before);
    assert_eq!(tmp_files(&dir.0), Vec::<PathBuf>::new());
}

#[test]
fn save_as_name_cannot_escape_bank() {
    let (mut state, _audio, _dir) = state("save-as-escape");
    let bank = state.config.bank_dir.clone();
    assert_eq!(
        error(handle(&mut state, save_as(".."))),
        "preset name must not be empty"
    );
    for (name, file) in [("../x", "04-x.toml"), ("a/b", "05-a-b.toml")] {
        let payload = ok(handle(&mut state, save_as(name)));
        assert_eq!(payload["path"], bank.join(file).display().to_string());
    }
    assert_eq!(
        bank_files(&bank),
        [
            "01-clean.toml",
            "02-loud.toml",
            "03-broken.toml",
            "04-x.toml",
            "05-a-b.toml"
        ]
    );
    assert!(!bank.join("a").exists());
    let outside = bank.parent().unwrap();
    assert!(!outside.join("x").exists());
    assert!(!outside.join("x.toml").exists());
}

#[test]
fn save_as_numbers_past_unindexed_files() {
    let (mut state, _audio, dir) = state("save-as-no-replace");
    let bank = state.config.bank_dir.clone();
    let victim = dir.0.join("victim.txt");
    fs::write(&victim, "keep").unwrap();
    fs::write(bank.join("04-x.toml"), "unindexed").unwrap();
    std::os::unix::fs::symlink(&victim, bank.join("05-y.toml")).unwrap();
    for (name, file) in [("x", "06-x.toml"), ("y", "07-y.toml")] {
        let payload = ok(handle(&mut state, save_as(name)));
        assert_eq!(payload["path"], bank.join(file).display().to_string());
    }
    assert_eq!(
        fs::read_to_string(bank.join("04-x.toml")).unwrap(),
        "unindexed"
    );
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    assert_eq!(tmp_files(&bank), Vec::<PathBuf>::new());
}

#[test]
fn save_as_at_the_largest_number_does_not_overflow() {
    let (mut state, _audio, _dir) = state("save-as-max-prefix");
    let bank = state.config.bank_dir.clone();
    fs::write(
        bank.join(format!("{}-x.toml", u64::MAX)),
        "name = \"X\"\n[[module]]\ntype = \"gain\"\n",
    )
    .unwrap();
    state.bank = Bank::load_dir(&bank).unwrap();
    let payload = ok(handle(&mut state, save_as("Y")));
    assert_eq!(
        payload["path"],
        bank.join(format!("{}-y.toml", u64::MAX))
            .display()
            .to_string()
    );
}

#[test]
fn save_as_numbers_past_99() {
    let (mut state, _audio, _dir) = state("save-as-past-99");
    let bank = state.config.bank_dir.clone();
    fs::write(
        bank.join("99-x.toml"),
        "name = \"X\"\n[[module]]\ntype = \"gain\"\n",
    )
    .unwrap();
    state.bank = Bank::load_dir(&bank).unwrap();
    for (name, file) in [("A", "100-a.toml"), ("B", "101-b.toml")] {
        let payload = ok(handle(&mut state, save_as(name)));
        assert_eq!(payload["path"], bank.join(file).display().to_string());
    }
}

#[test]
fn bypass_switches_the_module_and_shows_in_get() {
    let (mut state, mut audio, _dir) = state("bypass");
    assert_eq!(
        ok(handle(&mut state, bypass("gain", true))),
        json!({"bypass": true})
    );
    assert!(matches!(
        audio.commands.pop(),
        Ok(Command::SetBypass {
            module: 0,
            on: true
        })
    ));
    let all = ok(handle(
        &mut state,
        Request::Get {
            param: None,
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(all["params"][0]["bypass"], true);
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["modified"], true);
    assert_eq!(status["revision"], 2);
    assert_eq!(
        ok(handle(&mut state, bypass("gain", false))),
        json!({"bypass": false})
    );
    let all = ok(handle(
        &mut state,
        Request::Get {
            param: None,
            track: None,
            bus: None,
            master: false,
        },
    ));
    assert_eq!(all["params"][0]["bypass"], false);
}

#[test]
fn bypass_errors_verbatim() {
    let (mut state, _audio, _dir) = state("bypass-errors");
    assert_eq!(
        error(handle(&mut state, bypass("delay", true))),
        "no module 'delay'"
    );
    assert_eq!(ok(handle(&mut state, Request::Status))["revision"], 1);
    state.active = None;
    assert_eq!(
        error(handle(&mut state, bypass("gain", true))),
        "no preset loaded"
    );
}

#[test]
fn bypass_request_line() {
    let (mut state, _audio, _dir) = state("bypass-line");
    let connection = open_connection(&mut state);
    let line = r#"{"cmd":"bypass","id":"gain","on":true}"#;
    let (response, _) = handle_line(&mut state, None, connection, line);
    assert_eq!(response, r#"{"ok":true,"bypass":true}"#);
}

#[test]
fn save_writes_bypass() {
    let (mut state, _audio, _dir) = state("bypass-save");
    ok(handle(&mut state, bypass("gain", true)));
    ok(handle(&mut state, Request::Save { name: None }));
    let file = state.config.bank_dir.join("01-clean.toml");
    let saved = Preset::parse(&fs::read_to_string(&file).unwrap()).unwrap();
    assert!(saved.modules[0].bypass);
}
