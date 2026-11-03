use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use xguitare_core::control::{
    EngineState, Reprepare, Request, Response, handle, handle_line, needs_reprepare, reprepare,
};
use xguitare_core::engine::{
    AudioQueues, Command, EngineShared, SWITCHER_MAX_FRAMES, engine_queues, raise_peak,
};
use xguitare_core::{Bank, BankEntry, BuildWarning, Config, Preset, PresetModule};

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
    config.bank_dir = dir.0.clone();
    let (queues, audio) = engine_queues(QUEUE_CAPACITY);
    let state = EngineState {
        bank,
        active,
        shared,
        queues,
        config,
        connections: vec![("system:capture_1".to_string(), "xguitare:in".to_string())],
        sample_rate: 48_000.0,
        max_frames: 128,
        revision: 1,
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

fn set(param: &str, value: f64) -> Request {
    Request::Set {
        param: param.to_string(),
        value,
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
            "input_peak_db": -120.0,
            "output_peak_db": -120.0,
            "xruns": 3,
            "callback_max_us": 250,
            "callback_last_us": 90,
            "connections": [{"from": "system:capture_1", "to": "xguitare:in"}],
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
        },
    );
    assert!(matches!(response, Response::Ok(_)));
    assert!(
        matches!(&warnings[..], [BuildWarning::Ir { id, .. }] if id == "cabinet"),
        "{warnings:?}"
    );
}

#[test]
fn load_by_name_and_number() {
    let (mut state, mut audio, _dir) = state("load");
    let payload = ok(handle(
        &mut state,
        Request::Load {
            preset: "Loud".to_string(),
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
    let expected = json!({"module": "gain", "param": "level", "value": -3.0, "unit": "dB", "min": -60.0, "max": 20.0});
    let all = ok(handle(&mut state, Request::Get { param: None }));
    assert_eq!(all, json!({"params": [expected]}));
    let module = ok(handle(
        &mut state,
        Request::Get {
            param: Some("gain".to_string()),
        },
    ));
    assert_eq!(module, all);
    let one = ok(handle(
        &mut state,
        Request::Get {
            param: Some("gain.level".to_string()),
        },
    ));
    assert_eq!(one, all);
    assert_eq!(
        error(handle(
            &mut state,
            Request::Get {
                param: Some("delay".to_string())
            }
        )),
        "no module 'delay'"
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
                preset: "1".to_string()
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
    let response: Value =
        serde_json::from_str(&handle_line(&mut state, r#"{"cmd":"bogus"}"#).0).unwrap();
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
    let response: Value = serde_json::from_str(&handle_line(&mut state, "{\"cmd\":").0).unwrap();
    assert_eq!(response["ok"], false);
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid request: "),
        "{response}"
    );
    let response: Value =
        serde_json::from_str(&handle_line(&mut state, r#"{"cmd":"status"}"#).0).unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(response["preset"], "Clean");
}

#[test]
fn save_writes_live_values_atomically() {
    let (mut state, _audio, _dir) = state("save");
    ok(handle(&mut state, set("gain.level", -6.0)));
    let payload = ok(handle(&mut state, Request::Save));
    let file = state.config.bank_dir.join("01-clean.toml");
    assert_eq!(payload, json!({"path": file.display().to_string()}));
    let saved = Preset::parse(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved.modules[0].params[0], ("level".to_string(), -6.0));
    let leftovers = tmp_files(&state.config.bank_dir);
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn save_without_preset_is_error() {
    let (mut state, _audio, _dir) = state("save-without-preset");
    state.active = None;
    assert_eq!(error(handle(&mut state, Request::Save)), "no preset loaded");
}

#[test]
fn save_after_active_removed_is_error() {
    let (mut state, _audio, _dir) = state("save-after-removal");
    remove_active_file_and_reindex(&mut state);
    let loud = fs::read_to_string(state.config.bank_dir.join("02-loud.toml")).unwrap();
    assert_eq!(
        error(handle(&mut state, Request::Save)),
        "active preset file was removed; load a preset first"
    );
    assert!(!state.config.bank_dir.join("01-clean.toml").exists());
    assert_eq!(
        fs::read_to_string(state.config.bank_dir.join("02-loud.toml")).unwrap(),
        loud
    );
    assert!(tmp_files(&state.config.bank_dir).is_empty());
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
    let message = error(handle(&mut state, Request::Save));
    assert!(message.starts_with("cannot save"), "{message}");
    assert!(tmp_files(&state.config.bank_dir).is_empty());
}

#[test]
fn needs_reprepare_when_sizes_differ() {
    assert!(needs_reprepare(128, 256));
    assert!(needs_reprepare(512, 128));
    assert!(!needs_reprepare(128, 128));
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
        },
    ));
    assert_eq!(revision(&mut state), 2);
    ok(handle(&mut state, set("gain.level", -6.0)));
    assert_eq!(revision(&mut state), 3);
    ok(handle(&mut state, Request::Save));
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
