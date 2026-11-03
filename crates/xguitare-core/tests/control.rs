use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use xguitare_core::control::{EngineState, Request, Response, handle, handle_line};
use xguitare_core::engine::{AudioQueues, Command, EngineShared, engine_queues};
use xguitare_core::{Bank, BankEntry, Config, Preset};

const QUEUE_CAPACITY: usize = 64;

fn bank_dir(test: &str) -> PathBuf {
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
    dir
}

fn state(test: &str) -> (EngineState, AudioQueues) {
    let dir = bank_dir(test);
    let bank = Bank::load_dir(&dir).unwrap();
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
    let mut config = Config::load(Some(&dir.join("absent.toml"))).unwrap();
    config.bank_dir = dir;
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
    };
    (state, audio)
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

fn ok(response: Response) -> Value {
    match response {
        Response::Ok(payload) => payload,
        Response::Err { error } => panic!("expected ok, got error {error}"),
    }
}

fn error(response: Response) -> String {
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
    let (mut state, _audio) = state("status");
    let payload = ok(handle(&mut state, Request::Status));
    assert_eq!(
        payload,
        json!({
            "preset": "Clean",
            "program": 0,
            "sample_rate": 48000,
            "buffer_size": 128,
            "xruns": 3,
            "callback_max_us": 250,
            "callback_last_us": 90,
            "connections": [{"from": "system:capture_1", "to": "xguitare:in"}],
        })
    );
}

#[test]
fn list_marks_active_and_invalid() {
    let (mut state, _audio) = state("list");
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
fn load_by_name_and_number() {
    let (mut state, mut audio) = state("load");
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
    let (mut state, audio) = state("load-invalid");
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
    let (mut state, mut audio) = state("set");
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
    let (mut state, audio) = state("set-non-finite");
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
    let (mut state, _audio) = state("set-unknown-module");
    assert_eq!(
        error(handle(&mut state, set("drive.gain", 1.0))),
        "no module 'drive'"
    );
}

#[test]
fn set_unknown_param() {
    let (mut state, _audio) = state("set-unknown-param");
    assert_eq!(
        error(handle(&mut state, set("gain.drive", 1.0))),
        "module 'gain' has no parameter 'drive'"
    );
}

#[test]
fn set_bad_address() {
    let (mut state, _audio) = state("set-bad-address");
    assert_eq!(
        error(handle(&mut state, set("gain", 1.0))),
        "expected <module>.<param>, got 'gain'"
    );
}

#[test]
fn set_without_preset() {
    let (mut state, _audio) = state("set-without-preset");
    state.active = None;
    assert_eq!(
        error(handle(&mut state, set("gain.level", 1.0))),
        "no preset loaded"
    );
}

#[test]
fn get_all_and_one() {
    let (mut state, _audio) = state("get");
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
    let (mut state, audio) = state("busy");
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
    let (mut state, _audio) = state("unknown-cmd");
    let response: Value =
        serde_json::from_str(&handle_line(&mut state, r#"{"cmd":"bogus"}"#)).unwrap();
    assert_eq!(response["ok"], false);
    let message = response["error"].as_str().unwrap();
    assert!(
        message.starts_with("invalid request: unknown variant `bogus`"),
        "{message}"
    );
}

#[test]
fn malformed_json_is_error() {
    let (mut state, _audio) = state("malformed");
    let response: Value = serde_json::from_str(&handle_line(&mut state, "{\"cmd\":")).unwrap();
    assert_eq!(response["ok"], false);
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid request: "),
        "{response}"
    );
    let response: Value =
        serde_json::from_str(&handle_line(&mut state, r#"{"cmd":"status"}"#)).unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(response["preset"], "Clean");
}

#[test]
fn save_writes_live_values_atomically() {
    let (mut state, _audio) = state("save");
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
    let (mut state, _audio) = state("save-without-preset");
    state.active = None;
    assert_eq!(error(handle(&mut state, Request::Save)), "no preset loaded");
}

#[test]
fn save_after_active_removed_is_error() {
    let (mut state, _audio) = state("save-after-removal");
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
    let (mut state, _audio) = state("status-after-removal");
    remove_active_file_and_reindex(&mut state);
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["program"], Value::Null);
    assert_eq!(status["preset"], "Clean");
    let list = ok(handle(&mut state, Request::List));
    assert_eq!(list["active"], Value::Null);
}

#[test]
fn failed_save_leaves_no_tmp() {
    let (mut state, _audio) = state("save-fails");
    let target = state.config.bank_dir.join("01-clean.toml");
    fs::remove_file(&target).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("occupied"), "").unwrap();
    let message = error(handle(&mut state, Request::Save));
    assert!(message.starts_with("cannot save"), "{message}");
    assert!(tmp_files(&state.config.bank_dir).is_empty());
}
