use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use xguitare_core::control::{EngineState, Request, Response, handle, handle_with_ports};
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues};
use xguitare_core::firstrun::ensure_bank;
use xguitare_core::ports::{AudioPorts, AvailablePort};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BankEntry, BankKind, BuildWarning, Config, Ports};

const OWN_INPUT: &str = "xguitare:in";
const OWN_LEFT: &str = "xguitare:out_l";
const OWN_RIGHT: &str = "xguitare:out_r";
const BANK_DIR_IN_CONFIG: &str = "bank_dir is set in config.toml; edit it there";

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct FakePorts {
    log: RefCell<Vec<String>>,
    refused: Option<&'static str>,
}

fn port(name: &str, is_physical: bool) -> AvailablePort {
    AvailablePort {
        name: name.to_string(),
        is_physical,
    }
}

impl AudioPorts for FakePorts {
    fn audio_ports(&self) -> (Vec<AvailablePort>, Vec<AvailablePort>) {
        (
            vec![
                port("card:capture_FR", true),
                port("card:capture_FL", true),
                port("Firefox:output_FL", false),
            ],
            vec![
                port("card:playback_FR", true),
                port("card:playback_FL", true),
            ],
        )
    }

    fn own_ports(&self) -> Ports {
        Ports {
            input: OWN_INPUT.to_string(),
            output_left: OWN_LEFT.to_string(),
            output_right: OWN_RIGHT.to_string(),
        }
    }

    fn connect(&self, from: &str, to: &str) -> Result<(), String> {
        if self.refused == Some(to) {
            return Err("refused".to_string());
        }
        self.log
            .borrow_mut()
            .push(format!("connect {from} -> {to}"));
        Ok(())
    }

    fn disconnect(&self, from: &str, to: &str) -> Result<(), String> {
        self.log
            .borrow_mut()
            .push(format!("disconnect {from} -> {to}"));
        Ok(())
    }
}

fn link(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

fn state(test: &str, config_toml: &str) -> (EngineState, AudioQueues, TempDir) {
    let dir = std::env::temp_dir().join(format!("xguitare-settings-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    fs::write(&config_path, config_toml).unwrap();
    let config = Config::load(Some(&config_path)).unwrap();
    ensure_bank(&config.bank_dir, config.bank_kind, 48_000).unwrap();
    let bank = Bank::load_dir(&config.bank_dir).unwrap();
    let BankEntry::Valid { file, preset } = &bank.entries[0] else {
        panic!("the starter Bank's first Preset should be valid");
    };
    let active = Some((file.clone(), preset.clone()));
    let (queues, audio, tap) = engine_queues(64);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state = EngineState::new(
        bank,
        Arc::new(EngineShared::default()),
        queues,
        tuner,
        config,
        48_000.0,
        128,
    )
    .with_active(active)
    .with_connections(vec![
        link("old:capture_1", OWN_INPUT),
        link(OWN_LEFT, "card:playback_FL"),
    ]);
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

fn set_ports(input: &str, output_left: &str, output_right: &str) -> Request {
    Request::SetPorts {
        input: input.to_string(),
        output_left: output_left.to_string(),
        output_right: output_right.to_string(),
    }
}

fn set_settings(bank_kind: Option<BankKind>, ir_dir: Option<&Path>) -> Request {
    Request::SetSettings {
        bank_kind,
        ir_dir: ir_dir.map(Path::to_path_buf),
        projects_dir: None,
    }
}

fn revision(state: &mut EngineState) -> Value {
    ok(handle(state, Request::Status))["revision"].clone()
}

fn reloaded(dir: &TempDir) -> Config {
    Config::load(Some(&dir.0.join("config.toml"))).unwrap()
}

#[test]
fn ports_lists_ports_and_connected() {
    let (mut state, _audio, _dir) = state("ports", "");
    let payload = ok(handle_with_ports(
        &mut state,
        &FakePorts::default(),
        Request::Ports,
    ));
    assert_eq!(
        payload,
        json!({
            "capture": ["Firefox:output_FL", "card:capture_FL", "card:capture_FR"],
            "playback": ["card:playback_FL", "card:playback_FR"],
            "connected": {
                "input": "old:capture_1",
                "output_left": "card:playback_FL",
                "output_right": null,
            },
        })
    );
}

#[test]
fn set_ports_refuses_unknown_names() {
    let (mut state, _audio, dir) = state("set-ports-unknown", "");
    let ports = FakePorts::default();
    assert_eq!(
        error(handle_with_ports(
            &mut state,
            &ports,
            set_ports("nope", "card:playback_FL", "card:playback_FR"),
        )),
        "no capture port 'nope'"
    );
    assert_eq!(
        error(handle_with_ports(
            &mut state,
            &ports,
            set_ports("card:capture_FL", "card:capture_FR", "card:playback_FR"),
        )),
        "no playback port 'card:capture_FR'"
    );
    assert_eq!(
        error(handle_with_ports(
            &mut state,
            &ports,
            set_ports("card:capture_FL", "card:playback_FL", ""),
        )),
        "no playback port ''"
    );
    assert!(ports.log.borrow().is_empty());
    assert!(!dir.0.join("state.toml").exists());
    assert_eq!(revision(&mut state), 1);
}

#[test]
fn set_ports_reconnects_and_persists() {
    let config_toml = "# hand-written\n[ports]\ninput = \"card*\"\n";
    let (mut state, _audio, dir) = state("set-ports", config_toml);
    let ports = FakePorts::default();
    let payload = ok(handle_with_ports(
        &mut state,
        &ports,
        set_ports("card:capture_FR", "card:playback_FL", "card:playback_FR"),
    ));
    assert_eq!(
        payload,
        json!({"connected": {
            "input": "card:capture_FR",
            "output_left": "card:playback_FL",
            "output_right": "card:playback_FR",
        }})
    );
    assert_eq!(
        *ports.log.borrow(),
        [
            "disconnect old:capture_1 -> xguitare:in",
            "connect card:capture_FR -> xguitare:in",
            "connect xguitare:out_r -> card:playback_FR",
        ]
    );
    assert_eq!(
        state.connections,
        [
            link(OWN_LEFT, "card:playback_FL"),
            link("card:capture_FR", OWN_INPUT),
            link(OWN_RIGHT, "card:playback_FR"),
        ]
    );
    assert_eq!(revision(&mut state), 2);
    let state_file = dir.0.join("state.toml");
    assert_eq!(
        fs::metadata(&state_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::read_to_string(dir.0.join("config.toml")).unwrap(),
        config_toml
    );
    let config = reloaded(&dir);
    assert_eq!(config.ports.input, "card:capture_FR");
    assert_eq!(config.ports.output_left, "card:playback_FL");
    assert_eq!(config.ports.output_right, "card:playback_FR");
}

#[test]
fn settings_reports_bank_and_ir_dir() {
    let (mut state, _audio, dir) = state("settings", "");
    let bank_dir = dir.0.join("bank").display().to_string();
    let projects_dir = state.config.projects_dir.display().to_string();
    assert_eq!(
        ok(handle(&mut state, Request::Settings)),
        json!({"bank_kind": "guitar", "bank_dir": bank_dir, "ir_dir": bank_dir, "projects_dir": projects_dir})
    );
}

#[test]
fn set_settings_bank_kind_seeds_and_loads_program_0() {
    let (mut state, _audio, dir) = state("bank-kind", "");
    ok(handle(
        &mut state,
        Request::Load {
            preset: "2".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ));
    let bass_dir = dir.0.join("bank-bass");
    let bass = bass_dir.display().to_string();
    assert_eq!(
        ok(handle(&mut state, set_settings(Some(BankKind::Bass), None))),
        json!({"bank_kind": "bass", "bank_dir": bass, "ir_dir": bass, "projects_dir": state.config.projects_dir.display().to_string()})
    );
    assert!(bass_dir.join("01-blues.toml").is_file());
    assert!(bass_dir.join("bass-cab.wav").is_file());
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["preset"], "Blues");
    assert_eq!(status["program"], 0);
    assert_eq!(status["revision"], 3);
    assert_eq!(state.bank.dir, bass_dir);
    assert_eq!(state.bank.entries.len(), 15);
    let config = reloaded(&dir);
    assert_eq!(config.bank_kind, BankKind::Bass);
    assert_eq!(config.bank_dir, bass_dir);
    assert_eq!(config.state.banks.guitar, Some(dir.0.join("bank")));
    assert_eq!(config.state.banks.bass, Some(bass_dir));

    ok(handle(
        &mut state,
        set_settings(Some(BankKind::Guitar), None),
    ));
    assert_eq!(state.bank.dir, dir.0.join("bank"));
    assert_eq!(ok(handle(&mut state, Request::Status))["preset"], "Clean");
}

#[test]
fn set_settings_bass_user_keeps_bank_folder() {
    let (mut state, _audio, dir) = state("bass-user", "bank_kind = \"bass\"\n");
    let old_dir = dir.0.join("bank");
    assert_eq!(state.bank.dir, old_dir);
    assert_eq!(ok(handle(&mut state, Request::Status))["preset"], "Blues");

    ok(handle(
        &mut state,
        set_settings(Some(BankKind::Guitar), None),
    ));
    let guitar_dir = dir.0.join("bank-guitar");
    assert!(guitar_dir.join("01-clean.toml").is_file());
    assert_eq!(state.bank.dir, guitar_dir);
    assert_eq!(ok(handle(&mut state, Request::Status))["preset"], "Clean");
    let saved = reloaded(&dir).state.banks;
    assert_eq!(saved.bass, Some(old_dir.clone()));
    assert_eq!(saved.guitar, Some(guitar_dir));

    ok(handle(&mut state, set_settings(Some(BankKind::Bass), None)));
    assert_eq!(state.bank.dir, old_dir);
    assert_eq!(ok(handle(&mut state, Request::Status))["preset"], "Blues");
    assert!(!dir.0.join("bank-bass").exists());
    assert_eq!(reloaded(&dir).bank_dir, old_dir);
}

#[test]
fn set_settings_bank_kind_refused_when_config_sets_bank_dir() {
    let (mut state, _audio, dir) = state("bank-kind-refused", "");
    let config_toml = format!("bank_dir = \"{}\"\n", dir.0.join("bank").display());
    fs::write(dir.0.join("config.toml"), &config_toml).unwrap();
    state.config = reloaded(&dir);
    assert_eq!(
        error(handle(&mut state, set_settings(Some(BankKind::Bass), None))),
        BANK_DIR_IN_CONFIG
    );
    assert!(!dir.0.join("bank-bass").exists());
    assert!(!dir.0.join("state.toml").exists());
    assert_eq!(revision(&mut state), 1);
}

#[test]
fn set_settings_ir_dir_must_be_a_directory() {
    let (mut state, _audio, dir) = state("ir-dir", "");
    let missing = dir.0.join("nope");
    assert_eq!(
        error(handle(&mut state, set_settings(None, Some(&missing)))),
        format!("ir_dir '{}' is not a directory", missing.display())
    );
    let file = dir.0.join("config.toml");
    assert_eq!(
        error(handle(&mut state, set_settings(None, Some(&file)))),
        format!("ir_dir '{}' is not a directory", file.display())
    );
    assert_eq!(revision(&mut state), 1);

    let irs = dir.0.join("irs");
    fs::create_dir_all(&irs).unwrap();
    fs::write(irs.join("room.wav"), "").unwrap();
    let payload = ok(handle(&mut state, set_settings(None, Some(&irs))));
    assert_eq!(payload["ir_dir"], irs.display().to_string());
    assert_eq!(
        ok(handle(&mut state, Request::IrList)),
        json!({"ir_dir": irs.display().to_string(), "irs": ["room.wav"]})
    );
    assert_eq!(revision(&mut state), 2);

    ok(handle_with_ports(
        &mut state,
        &FakePorts::default(),
        set_ports("card:capture_FL", "card:playback_FL", "card:playback_FR"),
    ));
    let config = reloaded(&dir);
    assert_eq!(config.ir_dir, irs);
    assert_eq!(config.ports.input, "card:capture_FL");
}

fn fill_queue(state: &mut EngineState) {
    let first = ok(handle(
        state,
        Request::Get {
            param: None,
            track: None,
            bus: None,
            master: false,
        },
    ))["params"][0]
        .clone();
    let param = format!(
        "{}.{}",
        first["module"].as_str().unwrap(),
        first["param"].as_str().unwrap()
    );
    while let Response::Ok(_) = handle(
        state,
        Request::Set {
            param: param.clone(),
            value: 0.0,
            track: None,
            bus: None,
            master: false,
        },
    )
    .0
    {}
}

#[test]
fn set_settings_engine_busy_changes_nothing() {
    let (mut state, _audio, dir) = state("bank-kind-busy", "");
    let state_file = dir.0.join("state.toml");
    fs::write(&state_file, "ir_dir = \"/srv/xg/irs\"\n").unwrap();
    state.config = reloaded(&dir);
    let before_settings = ok(handle(&mut state, Request::Settings));
    fill_queue(&mut state);
    let before_status = ok(handle(&mut state, Request::Status));
    assert_eq!(
        error(handle(&mut state, set_settings(Some(BankKind::Bass), None))),
        "engine busy"
    );
    assert_eq!(ok(handle(&mut state, Request::Settings)), before_settings);
    assert_eq!(ok(handle(&mut state, Request::Status)), before_status);
    assert_eq!(state.bank.dir, dir.0.join("bank"));
    assert_eq!(
        fs::read_to_string(&state_file).unwrap(),
        "ir_dir = \"/srv/xg/irs\"\n"
    );
}

#[test]
fn set_settings_engine_busy_without_state_file_leaves_none() {
    let (mut state, _audio, dir) = state("bank-kind-busy-new", "");
    fill_queue(&mut state);
    assert_eq!(
        error(handle(&mut state, set_settings(Some(BankKind::Bass), None))),
        "engine busy"
    );
    assert!(!dir.0.join("state.toml").exists());
}

#[test]
fn set_ports_reports_failed_link_after_the_others() {
    let (mut state, _audio, _dir) = state("set-ports-refused", "");
    let ports = FakePorts {
        refused: Some("card:playback_FR"),
        ..FakePorts::default()
    };
    assert_eq!(
        error(handle_with_ports(
            &mut state,
            &ports,
            set_ports("card:capture_FL", "card:playback_FL", "card:playback_FR"),
        )),
        "cannot connect xguitare:out_r -> card:playback_FR: refused"
    );
    assert_eq!(
        ok(handle_with_ports(&mut state, &ports, Request::Ports))["connected"],
        json!({"input": "card:capture_FL", "output_left": "card:playback_FL", "output_right": null})
    );
}

#[test]
fn unwritable_state_file_changes_nothing() {
    let (mut state, _audio, dir) = state("state-unwritable", "");
    state.config.state_file = dir.0.join("config.toml/state.toml");
    let ports = FakePorts::default();
    let message = error(handle_with_ports(
        &mut state,
        &ports,
        set_ports("card:capture_FL", "card:playback_FL", "card:playback_FR"),
    ));
    let prefix = format!("cannot save {}: ", state.config.state_file.display());
    assert!(message.starts_with(&prefix), "{message}");
    assert!(ports.log.borrow().is_empty());
    assert_eq!(state.connections[0], link("old:capture_1", OWN_INPUT));
    assert_eq!(state.config.ports.input, "auto");
    let message = error(handle(&mut state, set_settings(None, Some(&dir.0))));
    assert!(message.starts_with(&prefix), "{message}");
    assert_eq!(state.config.ir_dir, dir.0.join("bank"));
    assert_eq!(revision(&mut state), 1);
}

#[test]
fn set_settings_refuses_relative_ir_dir_and_stores_no_same_kind() {
    let (mut state, _audio, dir) = state("ir-dir-relative", "");
    assert_eq!(
        error(handle(&mut state, set_settings(None, Some(Path::new("."))))),
        "ir_dir '.' is not a directory"
    );
    ok(handle(
        &mut state,
        set_settings(Some(BankKind::Guitar), None),
    ));
    ok(handle(&mut state, set_settings(None, None)));
    assert_eq!(reloaded(&dir).state.bank_kind, None);
    assert!(!dir.0.join("state.toml").exists());
    assert_eq!(revision(&mut state), 1);
}
