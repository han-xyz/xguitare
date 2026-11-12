use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use xguitare_client::ClientError;
use xguitare_core::control::Request;
use xguitare_core::control::Response;
use xguitare_core::control::{EngineState, PeakMeters, SILENCE_DB, handle};
use xguitare_core::engine::{EngineShared, engine_queues};
use xguitare_core::metronome::Metronome;
use xguitare_core::project_control::Projects;
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BankKind, Config};
use xguitare_gui::model::{
    AddPlace, Band, BankRow, ClipLatch, Command, Direction, EAR_HOLE_RADIUS, EAR_WIDTH, Feedback,
    KeyEvent, KeyModifiers, KnobKey, LADDER_SEGMENTS, Link, ModuleType, NO_SIGNAL, ParamRow,
    PeakHold, PortChoices, PortSelection, SECTIONS, Section, SettingsDraft, Throttle, TunerReading,
    add_after, add_command, add_place, add_places, autoscroll_delta, bank_kind, bank_rows,
    bank_rows_for_rack, cabinet_ir, cents_fraction, cents_text, counts_toward_spawn, ear_holes,
    engine_readout, failed_set, filter_bank, forget_removed_units, format_value, in_tune, ir_names,
    key_step, key_to_command, knob_angle_deg, knob_fraction, knob_param_value, knob_points,
    knob_value_from_drag, ladder_segments, latency_ms, module_types, move_target,
    open_units_from_string, open_units_to_string, param_label, param_rows, parse_typed_value,
    peak_db, peak_text, port_choices, quit_commands, rack_channels, refresh_decision,
    refused_remove, remember_removes, requests_until_quit, response_text, set_ports_command,
    set_settings_command, settings_draft, shortcuts_active, spawn_policy, tuner_header_text,
    tuner_reading, tuner_toggle, typed_text, typed_value, unit_suffix,
};
use xguitare_gui::model::{
    CLOSED_KNOB_WIDTH, Dragged, KnobArcs, Outgoing, RemovedUnit, Restore, ScreenRect, UnitStyle,
    bypassed, closed_unit_rows, drag_outcome, drop_slot, focus_ring_rects, knob_arcs, knob_origin,
    module_title, mute_banner, palette_place, rack_position_labels, restore, shown_undo,
    unit_style,
};
use xguitare_gui::model::{
    ChipClick, ChoiceButton, ChoiceKey, OPEN_KNOB_PITCH, OPEN_UNIT_HEIGHT, chip_click,
    choice_buttons, choice_step, open_knob_rows, open_unit_height,
};
use xguitare_gui::theme::{NEUTRAL_ACCENT, accent_for, palette};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn registry_modules(test: &str) -> serde_json::Value {
    let dir = TempDir(PathBuf::from(format!(
        "/tmp/xguitare-gm-{}-{test}",
        std::process::id()
    )));
    fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("config.toml");
    fs::write(&path, "").unwrap();
    let config = Config::load(Some(&path)).unwrap();
    let (queues, _, tap) = engine_queues(1);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let mut state = EngineState {
        bank: Bank {
            dir: dir.0.clone(),
            entries: Vec::new(),
        },
        active: None,
        shared: Arc::new(EngineShared::default()),
        queues,
        tuner,
        config,
        connections: Vec::new(),
        sample_rate: 48_000.0,
        max_frames: 128,
        revision: 0,
        modified: false,
        peaks: PeakMeters::default(),
        saved: HashMap::new(),
        projects: Projects::default(),
        metronome: Metronome::default(),
    };
    match handle(&mut state, Request::Modules).0 {
        Response::Ok(payload) => payload,
        Response::Err { error } => panic!("modules: {error}"),
    }
}

#[test]
fn spawn_policy_three_failures_then_backoff() {
    let now = Instant::now();
    assert!(!spawn_policy(2, None, now));
    assert!(spawn_policy(3, None, now));
    assert!(!spawn_policy(
        3,
        Some(now.checked_sub(Duration::from_secs(1)).unwrap()),
        now
    ));
    assert!(!spawn_policy(
        9,
        Some(now.checked_sub(Duration::from_millis(4_999)).unwrap()),
        now
    ));
    assert!(spawn_policy(
        4,
        Some(now.checked_sub(Duration::from_secs(5)).unwrap()),
        now
    ));
}

#[test]
fn spawn_policy_ignores_live_daemon_errors() {
    let socket = PathBuf::from("/run/user/1000/xguitare/xguitare.sock");
    assert!(counts_toward_spawn(&ClientError::NotRunning(
        socket.clone()
    )));
    assert!(!counts_toward_spawn(&ClientError::NotResponding(socket)));
    assert!(!counts_toward_spawn(&ClientError::InsecureSocketDir(
        "socket directory /tmp/xguitare-1000 is owned by another user".to_string()
    )));
}

#[test]
fn commands_before_quit_are_sent_in_order() {
    let queued = vec![
        Command::Save,
        Command::Set {
            param: "preamp.gain".to_string(),
            value: 70.0,
        },
        Command::RackAdd {
            type_name: "delay".to_string(),
            after: Some("reverb".to_string()),
            at: None,
        },
        Command::RackAdd {
            type_name: "gate".to_string(),
            after: None,
            at: Some(0),
        },
        Command::RackMove {
            id: "delay".to_string(),
            to: 4,
        },
        Command::RackRemove {
            id: "delay".to_string(),
        },
        Command::Quit,
        Command::Load("2".to_string()),
    ];
    let expected = vec![
        Request::Save { name: None },
        Request::Set {
            param: "preamp.gain".to_string(),
            value: 70.0,
            track: None,
            bus: None,
            master: false,
        },
        Request::RackAdd {
            r#type: "delay".to_string(),
            after: Some("reverb".to_string()),
            at: None,
            track: None,
            bus: None,
            master: false,
        },
        Request::RackAdd {
            r#type: "gate".to_string(),
            after: None,
            at: Some(0),
            track: None,
            bus: None,
            master: false,
        },
        Request::RackMove {
            id: "delay".to_string(),
            to: 4,
            track: None,
            bus: None,
            master: false,
        },
        Request::RackRemove {
            id: "delay".to_string(),
            track: None,
            bus: None,
            master: false,
        },
    ];
    let expected = expected.into_iter().map(Outgoing::Request).collect();
    assert_eq!(requests_until_quit(queued, None, None), (expected, true));
    assert_eq!(
        requests_until_quit(vec![Command::Load("Lead".to_string())], None, None),
        (
            vec![Outgoing::Request(Request::Load {
                preset: "Lead".to_string(),
                track: None,
                bus: None,
                master: false,
            })],
            false
        )
    );
}

#[test]
fn null_or_missing_peak_reads_as_silence() {
    let status = json!({"input_peak_db": null, "output_peak_db": -30.0});
    assert_eq!(peak_db(&status, "input_peak_db"), SILENCE_DB);
    assert_eq!(peak_db(&status, "output_peak_db"), -30.0);
    assert_eq!(peak_db(&json!({}), "output_peak_db"), SILENCE_DB);
}

#[test]
fn refresh_decision_on_revision_change() {
    assert!(refresh_decision(None, 1));
    assert!(!refresh_decision(Some(1), 1));
    assert!(refresh_decision(Some(1), 2));
}

#[test]
fn link_text() {
    let now = Instant::now();
    let since = now.checked_sub(Duration::from_millis(12_400)).unwrap();
    assert_eq!(
        xguitare_gui::model::link_text(&Link::Connected, now),
        "connected"
    );
    assert_eq!(
        xguitare_gui::model::link_text(&Link::Disconnected { since }, now),
        "disconnected 12s"
    );
    assert_eq!(
        xguitare_gui::model::link_text(
            &Link::Starting {
                since: now.checked_sub(Duration::from_secs(3)).unwrap()
            },
            now
        ),
        "starting daemon 3s"
    );
}

fn starter_list() -> serde_json::Value {
    json!({
        "active": 1,
        "entries": [
            {"program": 0, "name": "Clean", "valid": true},
            {"program": 1, "name": "Crunch", "valid": true},
            {"program": 2, "name": "Lead", "valid": true},
            {"program": 3, "name": "06-broken", "valid": false, "error": "line 1:8: string values must be quoted"},
        ],
    })
}

#[test]
fn bank_rows_marks_active_and_invalid() {
    let rows = bank_rows(&starter_list()).0;
    let row = |program: usize, name: &str, active: bool, error: Option<&str>| BankRow {
        program,
        name: name.to_string(),
        active,
        error: error.map(str::to_string),
    };
    assert_eq!(
        rows,
        vec![
            row(0, "Clean", false, None),
            row(1, "Crunch", true, None),
            row(2, "Lead", false, None),
            row(
                3,
                "06-broken",
                false,
                Some("line 1:8: string values must be quoted")
            ),
        ]
    );
    let no_active =
        json!({"active": null, "entries": [{"program": 0, "name": "Clean", "valid": true}]});
    assert!(!bank_rows(&no_active).0[0].active);
}

#[test]
fn bank_rows_are_refused_while_a_bus_or_master_rack_is_shown() {
    let rows = bank_rows(&starter_list()).0;
    assert_eq!(bank_rows_for_rack(rows.clone(), false), rows);
    let refused = bank_rows_for_rack(rows, true);
    let errors: Vec<_> = refused.iter().map(|row| row.error.as_deref()).collect();
    assert_eq!(
        errors,
        [
            Some("a Preset loads only into a Track"),
            Some("a Preset loads only into a Track"),
            Some("a Preset loads only into a Track"),
            Some("line 1:8: string values must be quoted"),
        ]
    );
    assert_eq!(
        key_to_command(KeyEvent::Digit(1, KeyModifiers::default()), &refused),
        None
    );
}

#[test]
fn key_digit_loads_program_only_when_valid() {
    let rows = bank_rows(&starter_list()).0;
    assert_eq!(
        key_to_command(KeyEvent::Digit(3, KeyModifiers::default()), &rows),
        Some(Command::Load("2".to_string()))
    );
    assert_eq!(
        key_to_command(KeyEvent::Digit(1, KeyModifiers::default()), &rows),
        Some(Command::Load("0".to_string()))
    );
    assert_eq!(
        key_to_command(KeyEvent::Digit(4, KeyModifiers::default()), &rows),
        None
    );
    assert_eq!(
        key_to_command(KeyEvent::Digit(5, KeyModifiers::default()), &rows),
        None
    );
    assert_eq!(
        key_to_command(KeyEvent::Digit(0, KeyModifiers::default()), &rows),
        None
    );
}

#[test]
fn key_ctrl_s_is_save() {
    assert_eq!(key_to_command(KeyEvent::CtrlS, &[]), Some(Command::Save));
}

fn two_module_get() -> serde_json::Value {
    json!({"params": [
        {"module": "preamp", "type": "preamp", "param": "gain", "value": 40.0, "unit": "%", "min": 0.0, "max": 100.0, "default": 50.0, "integer": false},
        {"module": "preamp", "type": "preamp", "param": "stages", "value": 3.0, "unit": "", "min": 1.0, "max": 4.0, "default": 2.0, "integer": true},
        {"module": "delay", "type": "delay", "param": "time", "value": 350.0, "unit": "ms", "min": 1.0, "max": 2000.0, "default": 400.0, "integer": false},
        {"module": "delay", "type": "delay", "param": "pingpong", "value": 0.0, "unit": "", "min": 0.0, "max": 1.0, "default": 0.0, "integer": true},
    ]})
}

#[test]
fn param_rows_in_rack_order_with_units() {
    let row = |module: &str,
               param: &str,
               value: f64,
               (min, max, default): (f64, f64, f64),
               unit: &str,
               integer: bool| {
        ParamRow {
            address: format!("{module}.{param}"),
            module: module.to_string(),
            module_type: module.to_string(),
            param: param.to_string(),
            value,
            min,
            max,
            default,
            unit: unit.to_string(),
            integer,
            labels: Vec::new(),
        }
    };
    assert_eq!(
        param_rows(&two_module_get()),
        vec![
            (
                "preamp".to_string(),
                vec![
                    row("preamp", "gain", 40.0, (0.0, 100.0, 50.0), "%", false),
                    row("preamp", "stages", 3.0, (1.0, 4.0, 2.0), "", true),
                ]
            ),
            (
                "delay".to_string(),
                vec![
                    row("delay", "time", 350.0, (1.0, 2000.0, 400.0), "ms", false),
                    row("delay", "pingpong", 0.0, (0.0, 1.0, 0.0), "", true),
                ]
            ),
        ]
    );
    assert_eq!(param_rows(&json!({})), []);
}

#[test]
fn integer_flag_from_payload() {
    let bias = json!({"params": [
        {"module": "preamp", "type": "preamp", "param": "bias", "value": 0.0, "unit": "", "min": -1.0, "max": 1.0, "integer": false},
        {"module": "preamp", "type": "preamp", "param": "stages", "value": 3.0, "unit": "", "min": 1.0, "max": 4.0, "integer": true},
    ]});
    let flags: Vec<bool> = param_rows(&bias)[0]
        .1
        .iter()
        .map(|row| row.integer)
        .collect();
    assert_eq!(flags, [false, true]);
}

#[test]
fn throttle_sends_at_most_every_30ms_and_final() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let set = |value: f64| Command::Set {
        param: "preamp.gain".to_string(),
        value,
    };
    let mut throttle = Throttle::default();
    assert_eq!(
        throttle.on_drag("preamp.gain", 10.0, at(0)),
        Some(set(10.0))
    );
    assert_eq!(throttle.on_drag("preamp.gain", 11.0, at(10)), None);
    assert_eq!(throttle.on_drag("preamp.gain", 12.0, at(20)), None);
    assert_eq!(
        throttle.on_drag("preamp.gain", 13.0, at(35)),
        Some(set(13.0))
    );
    assert_eq!(
        throttle.on_drag("delay.time", 5.0, at(36)),
        Some(Command::Set {
            param: "delay.time".to_string(),
            value: 5.0,
        })
    );
    assert_eq!(throttle.on_release("preamp.gain", 14.0, 7), set(14.0));
}

#[test]
fn hold_local_until_newer_revision() {
    let now = Instant::now();
    let mut throttle = Throttle::default();
    assert!(!throttle.hold_local("preamp.gain", 5));
    throttle.on_drag("preamp.gain", 10.0, now);
    assert!(throttle.hold_local("preamp.gain", 6));
    throttle.on_release("preamp.gain", 12.0, 7);
    assert!(throttle.hold_local("preamp.gain", 7));
    assert!(!throttle.hold_local("preamp.gain", 8));
    assert!(!throttle.hold_local("delay.time", 7));
}

#[test]
fn feedback_expires_after_3s() {
    let now = Instant::now();
    let mut feedback = Feedback::default();
    assert_eq!(feedback.text(now), None);
    feedback.show("saved /bank/01-clean.toml".to_string(), now);
    assert_eq!(
        feedback.text(now + Duration::from_millis(2_999)),
        Some("saved /bank/01-clean.toml")
    );
    assert_eq!(feedback.text(now + Duration::from_secs(3)), None);
}

#[test]
fn feedback_only_save_and_errors() {
    let saved = Ok(Response::Ok(json!({"path": "/bank/01-clean.toml"})));
    assert_eq!(
        response_text(&Request::Save { name: None }, saved).as_deref(),
        Some("saved /bank/01-clean.toml")
    );
    let busy = Ok(Response::Err {
        error: "engine busy".to_string(),
    });
    let set = Request::Set {
        param: "preamp.gain".to_string(),
        value: 42.0,
        track: None,
        bus: None,
        master: false,
    };
    assert_eq!(
        response_text(&set, busy).as_deref(),
        Some("error: engine busy")
    );
    let gone = Err(ClientError::NotRunning(PathBuf::from("/tmp/xgv/x.sock")));
    assert!(
        response_text(&Request::Save { name: None }, gone)
            .is_some_and(|text| text.starts_with("error: "))
    );
    let applied = Ok(Response::Ok(
        json!({"param": "preamp.gain", "value": 42.0, "clamped": false}),
    ));
    assert_eq!(response_text(&set, applied), None);
    let loaded = Ok(Response::Ok(json!({"program": 1, "name": "Crunch"})));
    let load = Request::Load {
        preset: "1".to_string(),
        track: None,
        bus: None,
        master: false,
    };
    assert_eq!(response_text(&load, loaded), None);
}

#[test]
fn bank_rows_rejects_malformed() {
    let list = json!({"active": 0, "entries": [
        {"program": 0, "name": "Clean", "valid": true},
        {"name": "No program", "valid": true},
        {"program": 1.5, "name": "Half", "valid": true},
        {"program": "2", "name": "Text", "valid": true},
        {"program": 3, "name": "05-bad", "valid": false},
    ]});
    let (rows, skipped) = bank_rows(&list);
    assert_eq!(skipped, 3);
    let programs: Vec<usize> = rows.iter().map(|row| row.program).collect();
    assert_eq!(programs, [0, 3]);
    assert_eq!(rows[1].error.as_deref(), Some("invalid preset"));
}

#[test]
fn digits_ignored_with_modifiers() {
    let rows = bank_rows(&starter_list()).0;
    let ctrl = KeyModifiers {
        ctrl: true,
        alt: false,
    };
    let alt = KeyModifiers {
        ctrl: false,
        alt: true,
    };
    assert_eq!(key_to_command(KeyEvent::Digit(1, ctrl), &rows), None);
    assert_eq!(key_to_command(KeyEvent::Digit(1, alt), &rows), None);
    assert_eq!(
        key_to_command(KeyEvent::Digit(1, KeyModifiers::default()), &rows),
        Some(Command::Load("0".to_string()))
    );
}

#[test]
fn unit_suffix_table() {
    let suffixes: Vec<&str> = ["dB", "ms", "Hz", "%", ""]
        .into_iter()
        .map(unit_suffix)
        .collect();
    assert_eq!(suffixes, [" dB", " ms", " Hz", " %", ""]);
}

#[test]
fn throttle_flushes_pending_without_new_drag() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut throttle = Throttle::default();
    throttle.on_drag("preamp.gain", 10.0, at(0));
    assert_eq!(throttle.on_drag("preamp.gain", 13.0, at(10)), None);
    assert_eq!(throttle.tick(at(20)), []);
    assert_eq!(
        throttle.tick(at(31)),
        [Command::Set {
            param: "preamp.gain".to_string(),
            value: 13.0
        }]
    );
    assert_eq!(throttle.tick(at(70)), []);
}

#[test]
fn hold_cleared_when_param_gone() {
    let now = Instant::now();
    let mut throttle = Throttle::default();
    throttle.on_drag("preamp.gain", 10.0, now);
    throttle.on_drag("delay.time", 300.0, now);
    throttle.retain(|address| address == "delay.time");
    assert!(!throttle.hold_local("preamp.gain", 9));
    assert!(throttle.hold_local("delay.time", 9));
}

#[test]
fn hold_cleared_on_set_error() {
    let mut throttle = Throttle::default();
    throttle.on_drag("preamp.gain", 10.0, Instant::now());
    throttle.on_release("preamp.gain", 12.0, 7);
    let set = Request::Set {
        param: "preamp.gain".to_string(),
        value: 12.0,
        track: None,
        bus: None,
        master: false,
    };
    let busy = Ok(Response::Err {
        error: "engine busy".to_string(),
    });
    let failed = failed_set(&set, &busy).unwrap();
    assert_eq!(failed, "preamp.gain");
    throttle.forget(&failed);
    assert!(!throttle.hold_local("preamp.gain", 7));
    let gone = Err(ClientError::NotRunning(PathBuf::from("/tmp/xgv/x.sock")));
    assert_eq!(failed_set(&set, &gone).as_deref(), Some("preamp.gain"));
    let applied = Ok(Response::Ok(
        json!({"param": "preamp.gain", "value": 12.0, "clamped": false}),
    ));
    assert_eq!(failed_set(&set, &applied), None);
    assert_eq!(failed_set(&Request::Save { name: None }, &busy), None);
}

#[test]
fn move_target_at_ends_is_none() {
    assert_eq!(move_target(0, Direction::Up, 5), None);
    assert_eq!(move_target(4, Direction::Down, 5), None);
    assert_eq!(move_target(0, Direction::Down, 1), None);
}

#[test]
fn move_target_middle() {
    assert_eq!(move_target(2, Direction::Up, 5), Some(1));
    assert_eq!(move_target(2, Direction::Down, 5), Some(3));
    assert_eq!(move_target(0, Direction::Down, 5), Some(1));
    assert_eq!(move_target(4, Direction::Up, 5), Some(3));
}

fn clean_rack() -> Vec<(String, String)> {
    [
        ("gate", "mono"),
        ("amp", "mono"),
        ("cabinet", "mono_to_stereo"),
        ("reverb", "stereo"),
    ]
    .map(|(id, channels)| (id.to_string(), channels.to_string()))
    .to_vec()
}

#[test]
fn add_after_mono_goes_before_cabinet() {
    assert_eq!(add_after(&clean_rack(), "mono").as_deref(), Some("amp"));
    assert_eq!(
        add_after(&clean_rack(), "mono_to_stereo").as_deref(),
        Some("amp")
    );
    assert_eq!(add_after(&clean_rack()[2..], "mono"), None);
}

#[test]
fn add_after_stereo_appends() {
    assert_eq!(
        add_after(&clean_rack(), "stereo").as_deref(),
        Some("reverb")
    );
}

#[test]
fn add_after_empty_rack_is_front() {
    assert_eq!(add_after(&[], "mono"), None);
    assert_eq!(add_after(&[], "stereo"), None);
    let gate = ModuleType {
        type_name: "gate".to_string(),
        channels: "mono".to_string(),
    };
    let front = Command::RackAdd {
        type_name: "gate".to_string(),
        after: None,
        at: Some(0),
    };
    let place = |rack: &[(String, String)]| add_place(rack, &gate.channels, None);
    assert_eq!(add_command("gate", &place(&[])), front);
    assert_eq!(add_command("gate", &place(&clean_rack()[2..])), front);
    assert_eq!(
        add_command("gate", &place(&clean_rack())),
        Command::RackAdd {
            type_name: "gate".to_string(),
            after: Some("amp".to_string()),
            at: None,
        }
    );
}

#[test]
fn add_place_offers_the_front_and_after_each_unit() {
    let places = add_places(&clean_rack());
    let labels: Vec<String> = places.iter().map(AddPlace::label).collect();
    assert_eq!(
        labels,
        [
            "at the front",
            "after gate",
            "after amp",
            "after cabinet",
            "after reverb"
        ]
    );
}

#[test]
fn add_place_keeps_a_chosen_place_while_the_unit_is_there() {
    let rack = clean_rack();
    let chosen = AddPlace::After("gate".to_string());
    assert_eq!(add_place(&rack, "stereo", Some(&chosen)), chosen);
    assert_eq!(
        add_place(&rack, "stereo", Some(&AddPlace::Front)),
        AddPlace::Front
    );
    assert_eq!(
        add_place(&rack, "stereo", Some(&AddPlace::After("gone".to_string()))),
        AddPlace::After("reverb".to_string())
    );
    assert_eq!(
        add_place(&rack, "mono", None),
        AddPlace::After("amp".to_string())
    );
}

#[test]
fn rack_channels_by_module_type() {
    let get = json!({"params": [
        {"module": "amp", "type": "preamp", "param": "gain", "value": 40.0, "unit": "%", "min": 0.0, "max": 100.0, "integer": false},
        {"module": "delay-2", "type": "delay", "param": "time", "value": 350.0, "unit": "ms", "min": 1.0, "max": 2000.0, "integer": false},
    ]});
    let types = module_types(&json!({"modules": [
        {"type": "preamp", "channels": "mono", "params": []},
        {"type": "delay", "channels": "stereo", "params": []},
    ]}));
    assert_eq!(
        rack_channels(&param_rows(&get), &types),
        vec![
            ("amp".to_string(), "mono".to_string()),
            ("delay-2".to_string(), "stereo".to_string()),
        ]
    );
}

#[test]
fn module_types_from_payload() {
    let types = module_types(&registry_modules("module-types"));
    assert_eq!(types.len(), 17);
    let channels = |type_name: &str| {
        types
            .iter()
            .find(|module_type| module_type.type_name == type_name)
            .map(|module_type| module_type.channels.as_str())
    };
    assert_eq!(channels("gain"), Some("mono"));
    assert_eq!(channels("fuzz"), Some("mono"));
    assert_eq!(channels("autowah"), Some("mono"));
    assert_eq!(channels("eq"), Some("mono"));
    assert_eq!(channels("blend"), Some("mono"));
    assert_eq!(channels("limiter"), Some("mono"));
    assert_eq!(channels("shaper"), Some("mono"));
    assert_eq!(channels("cabinet"), Some("mono_to_stereo"));
    assert_eq!(channels("reverb"), Some("stereo"));
    let cabinet = types.iter().find(|t| t.type_name == "cabinet").unwrap();
    assert_eq!(cabinet.label(), "cabinet (mono_to_stereo)");
    assert_eq!(module_types(&json!({})), Vec::<ModuleType>::new());
}

#[test]
fn knob_value_from_drag_steps_and_clamps() {
    assert_eq!(
        knob_value_from_drag(0.0, 0.0, 100.0, -220.0, false, false),
        100.0
    );
    assert_eq!(
        knob_value_from_drag(50.0, 0.0, 100.0, -22.0, false, false),
        60.0
    );
    assert_eq!(
        knob_value_from_drag(50.0, 0.0, 100.0, 22.0, false, false),
        40.0
    );
    assert_eq!(
        knob_value_from_drag(-20.0, -60.0, 20.0, -110.0, false, false),
        20.0
    );
    assert_eq!(
        knob_value_from_drag(-20.0, -60.0, 20.0, 400.0, false, false),
        -60.0
    );
}

#[test]
fn knob_fine_is_ten_times_slower() {
    assert_eq!(
        knob_value_from_drag(50.0, 0.0, 100.0, -22.0, true, false),
        51.0
    );
    assert_eq!(
        knob_value_from_drag(0.0, 0.0, 100.0, -2200.0, true, false),
        100.0
    );
    assert_eq!(
        knob_value_from_drag(0.0, 0.0, 100.0, -220.0, true, false),
        10.0
    );
}

#[test]
fn knob_integer_rounds() {
    assert_eq!(knob_value_from_drag(2.0, 1.0, 4.0, -40.0, false, true), 3.0);
    assert_eq!(knob_value_from_drag(2.0, 1.0, 4.0, -20.0, false, true), 2.0);
    assert_eq!(
        knob_value_from_drag(0.0, 0.0, 1.0, -110.0, false, true),
        1.0
    );
    assert_eq!(knob_value_from_drag(1.0, 0.0, 1.0, 90.0, false, true), 1.0);
}

#[test]
fn knob_fraction_ends_mid_and_clamping() {
    assert_eq!(knob_fraction(-60.0, -60.0, 20.0), 0.0);
    assert_eq!(knob_fraction(20.0, -60.0, 20.0), 1.0);
    assert_eq!(knob_fraction(-20.0, -60.0, 20.0), 0.5);
    assert_eq!(knob_fraction(-90.0, -60.0, 20.0), 0.0);
    assert_eq!(knob_fraction(35.0, -60.0, 20.0), 1.0);
    assert_eq!(knob_fraction(3.0, 3.0, 3.0), 0.0);
}

#[test]
fn knob_angle_sweeps_270_degrees_from_lower_left() {
    assert_eq!(knob_angle_deg(0.0), 135.0);
    assert_eq!(knob_angle_deg(0.5), 270.0);
    assert_eq!(knob_angle_deg(1.0), 405.0);
}

#[test]
fn accent_table_covers_all_types() {
    let types = module_types(&registry_modules("accents"));
    let accents: Vec<_> = types
        .iter()
        .map(|module_type| accent_for(&module_type.type_name))
        .collect();
    assert!(accents.iter().all(|accent| *accent != NEUTRAL_ACCENT));
    for (index, accent) in accents.iter().enumerate() {
        assert!(!accents[index + 1..].contains(accent), "{:?}", types[index]);
    }
    assert_eq!(accent_for("theremin"), NEUTRAL_ACCENT);
}

#[test]
fn accent_greys_are_distinct() {
    let greys = [accent_for("gate"), accent_for("gain"), NEUTRAL_ACCENT];
    let far_apart = |(r1, g1, b1): (u8, u8, u8), (r2, g2, b2): (u8, u8, u8)| {
        [(r1, r2), (g1, g2), (b1, b2)]
            .iter()
            .any(|(a, b)| a.abs_diff(*b) > 40)
    };
    for (index, grey) in greys.iter().enumerate() {
        for other in &greys[index + 1..] {
            assert!(far_apart(*grey, *other), "{grey:?} vs {other:?}");
        }
    }
}

#[test]
fn tuner_reading_requires_confident() {
    assert_eq!(
        tuner_reading(&json!({"ok":true,"hz":41.2,"note":"E1","cents":-3,"confident":true})),
        Some(TunerReading {
            note: "E1".to_string(),
            hz: 41.2,
            cents: -3,
        })
    );
    assert_eq!(
        tuner_reading(&json!({"ok":true,"hz":41.2,"note":"E1","cents":-3,"confident":false})),
        None
    );
    assert_eq!(
        tuner_reading(&json!({"ok":true,"hz":null,"note":null,"cents":null,"confident":false})),
        None
    );
}

#[test]
fn cents_fraction_clamps() {
    assert_eq!(cents_fraction(0), 0.0);
    assert_eq!(cents_fraction(25), 0.5);
    assert_eq!(cents_fraction(-50), -1.0);
    assert_eq!(cents_fraction(80), 1.0);
    assert_eq!(cents_fraction(-80), -1.0);
}

#[test]
fn in_tune_within_three() {
    assert!(in_tune(0));
    assert!(in_tune(3));
    assert!(in_tune(-3));
    assert!(!in_tune(4));
    assert!(!in_tune(-4));
}

#[test]
fn tuner_toggle_mutes_and_unmutes() {
    assert_eq!(
        tuner_toggle(false, true, false, false),
        (true, Some(Command::Mute(true)), true)
    );
    assert_eq!(
        tuner_toggle(false, false, false, false),
        (true, None, false)
    );
    assert_eq!(
        tuner_toggle(true, true, true, true),
        (false, Some(Command::Mute(false)), false)
    );
    assert_eq!(tuner_toggle(true, true, false, true), (false, None, false));
}

#[test]
fn tuner_leaves_a_mute_turned_on_by_hand_alone() {
    let (open, command, muted_by_us) = tuner_toggle(false, true, false, true);
    assert_eq!((open, command, muted_by_us), (true, None, false));
    assert_eq!(
        tuner_toggle(open, true, muted_by_us, true),
        (false, None, false)
    );
}

#[test]
fn active_unit_keeps_its_panel_and_accent() {
    let colours = palette();
    let closed = unit_style(false, false);
    assert_eq!(
        closed,
        UnitStyle {
            panel: colours.panel,
            text: colours.text,
            arc: None,
            lamp_lit: true,
            tag: None,
        }
    );
    assert_eq!(unit_style(false, true).panel, colours.panel_open);
}

#[test]
fn bypassed_unit_greys_its_panel_text_and_arcs_and_wears_a_tag() {
    let colours = palette();
    for open in [false, true] {
        assert_eq!(
            unit_style(true, open),
            UnitStyle {
                panel: colours.bypass_panel,
                text: colours.bypass_text,
                arc: Some(colours.bypass_arc),
                lamp_lit: false,
                tag: Some("Bypassed"),
            }
        );
    }
}

#[test]
fn quit_with_tuner_open_unmutes_first() {
    assert_eq!(
        quit_commands(true, true),
        vec![Command::Mute(false), Command::Quit]
    );
    assert_eq!(quit_commands(true, false), vec![Command::Quit]);
    assert_eq!(quit_commands(false, false), vec![Command::Quit]);
    let (requests, quit) = single_requests_until_quit(quit_commands(true, true));
    assert_eq!((requests, quit), (vec![Request::Mute { on: false }], true));
}

#[test]
fn mute_command_maps_to_request() {
    assert_eq!(
        single_requests_until_quit(vec![Command::Mute(true), Command::Mute(false)]),
        (
            vec![Request::Mute { on: true }, Request::Mute { on: false }],
            false
        )
    );
}

#[test]
fn bypass_command_maps_to_request() {
    let command = Command::Bypass {
        id: "chorus".to_string(),
        on: true,
    };
    assert_eq!(
        single_requests_until_quit(vec![command]),
        (
            vec![Request::Bypass {
                id: "chorus".to_string(),
                on: true,
                track: None,
                bus: None,
                master: false,
            }],
            false
        )
    );
}

#[test]
fn bypassed_from_get() {
    let payload = json!({"params": [
        {"module": "preamp", "type": "preamp", "param": "gain", "value": 40.0, "bypass": false},
        {"module": "chorus", "type": "chorus", "param": "rate", "value": 1.0, "bypass": true},
        {"module": "chorus", "type": "chorus", "param": "mix", "value": 0.4, "bypass": true},
    ]});
    assert!(bypassed(&payload, "chorus"));
    assert!(!bypassed(&payload, "preamp"));
    assert!(!bypassed(&payload, "delay"));
}

#[test]
fn mute_banner_only_while_muted() {
    let muted = json!({"muted": true});
    assert_eq!(
        mute_banner(Some(&muted), false).as_deref(),
        Some("Output muted \u{2014} press Mute to unmute")
    );
    assert_eq!(
        mute_banner(Some(&muted), true).as_deref(),
        Some("Output muted \u{2014} close the Tuner to unmute")
    );
    assert_eq!(mute_banner(Some(&json!({"muted": false})), true), None);
    assert_eq!(mute_banner(None, false), None);
}

#[test]
fn cents_text_is_signed() {
    assert_eq!(cents_text(3), "+3 ct");
    assert_eq!(cents_text(-3), "\u{2212}3 ct");
    assert_eq!(cents_text(0), "+0 ct");
}

#[test]
fn ir_names_from_payload() {
    let payload = json!({"ok": true, "ir_dir": "/srv/irs", "irs": ["Loud.WAV", "bass-cab.wav", "celestion/v30.wav"]});
    assert_eq!(
        ir_names(&payload),
        vec!["Loud.WAV", "bass-cab.wav", "celestion/v30.wav"]
    );
    assert_eq!(
        ir_names(&json!({"ok": true, "ir_dir": "/srv/irs", "irs": []})),
        Vec::<String>::new()
    );
}

#[test]
fn cabinet_ir_from_get() {
    let payload = json!({"params": [
        {"module": "preamp", "type": "preamp", "param": "gain", "value": 40.0},
        {"module": "cabinet", "type": "cabinet", "param": "mix", "value": 1.0, "ir": "cabs/x.wav"},
        {"module": "cabinet", "type": "cabinet", "param": "low_cut", "value": 80.0, "ir": "cabs/x.wav"},
        {"module": "cab2", "type": "cabinet", "param": "mix", "value": 1.0, "ir": null},
    ]});
    assert_eq!(
        cabinet_ir(&payload, "cabinet"),
        Some("cabs/x.wav".to_string())
    );
    assert_eq!(cabinet_ir(&payload, "cab2"), None);
    assert_eq!(cabinet_ir(&payload, "preamp"), None);
    assert_eq!(cabinet_ir(&payload, "delay"), None);
}

#[test]
fn rack_ir_command_maps_to_request() {
    assert_eq!(
        single_requests_until_quit(vec![Command::RackIr {
            id: "cabinet".to_string(),
            ir: "cabs/x.wav".to_string(),
        }]),
        (
            vec![Request::RackIr {
                ir: "cabs/x.wav".to_string(),
                id: Some("cabinet".to_string()),
                track: None,
                bus: None,
                master: false,
            }],
            false
        )
    );
}

fn assert_near(actual: (f32, f32), expected: (f32, f32)) {
    let close = (actual.0 - expected.0).abs() < 1e-4 && (actual.1 - expected.1).abs() < 1e-4;
    assert!(close, "{actual:?} is not {expected:?}");
}

#[test]
fn knob_pointer_follows_the_sweep() {
    let diagonal = 10.0 * std::f32::consts::FRAC_1_SQRT_2;
    assert_near(knob_points(0.0, 10.0).pointer, (-diagonal, diagonal));
    assert_near(knob_points(0.5, 10.0).pointer, (0.0, -10.0));
    assert_near(knob_points(1.0, 10.0).pointer, (diagonal, diagonal));
}

#[test]
fn knob_track_spans_the_sweep_and_the_value_arc_ends_at_the_value() {
    assert_eq!(
        knob_arcs(0.5, 0.0),
        KnobArcs {
            track: (135.0, 405.0),
            value: Some((135.0, 270.0)),
        }
    );
    assert_eq!(knob_arcs(1.0, 0.0).value, Some((135.0, 405.0)));
    assert_eq!(knob_arcs(0.0, 0.0).value, None);
    assert_eq!(knob_arcs(0.0, 0.0).track, (135.0, 405.0));
}

#[test]
fn a_bipolar_knob_lights_its_arc_from_the_top() {
    assert_eq!(knob_arcs(0.5, 0.5).value, None);
    assert_eq!(knob_arcs(0.25, 0.5).value, Some((270.0, 202.5)));
    assert_eq!(knob_arcs(1.0, 0.5).value, Some((270.0, 405.0)));
}

#[test]
fn a_range_symmetric_around_a_zero_default_starts_at_the_middle() {
    assert_eq!(knob_origin(-1.0, 1.0, 0.0), 0.5);
    assert_eq!(knob_origin(-12.0, 12.0, 0.0), 0.5);
    assert_eq!(knob_origin(-12.0, 12.0, 3.0), 0.0);
    assert_eq!(knob_origin(-60.0, 6.0, 0.0), 0.0);
    assert_eq!(knob_origin(0.0, 100.0, 50.0), 0.0);
}

#[test]
fn focus_ring_sits_a_gap_outside_the_widget() {
    let widget = ScreenRect {
        left: 10.0,
        top: 20.0,
        right: 50.0,
        bottom: 40.0,
    };
    let rect = |left, top, right, bottom| ScreenRect {
        left,
        top,
        right,
        bottom,
    };
    assert_eq!(
        focus_ring_rects(widget),
        (rect(9.0, 19.0, 51.0, 41.0), rect(7.0, 17.0, 53.0, 43.0))
    );
}

fn ladder_counts(db: f64, clipped: bool) -> (usize, usize, usize) {
    let lit: Vec<Band> = ladder_segments(db, clipped)
        .iter()
        .filter(|segment| segment.lit)
        .map(|segment| segment.band)
        .collect();
    let count = |band: Band| lit.iter().filter(|lit_band| **lit_band == band).count();
    (count(Band::Green), count(Band::Yellow), count(Band::Red))
}

#[test]
fn ladder_lights_bands_from_the_bottom() {
    assert_eq!(ladder_segments(0.0, false).len(), LADDER_SEGMENTS);
    assert_eq!(ladder_counts(-120.0, false), (0, 0, 0));
    assert_eq!(ladder_counts(-60.0, false), (0, 0, 0));
    assert_eq!(ladder_counts(-12.0, false), (19, 1, 0));
    assert_eq!(ladder_counts(-3.0, false), (19, 3, 1));
    assert_eq!(ladder_counts(0.0, false), (19, 3, 2));
    assert_eq!(ladder_counts(6.0, false), (19, 3, 2));
    let segments = ladder_segments(-12.0, false);
    assert!(segments.windows(2).all(|pair| pair[0].lit || !pair[1].lit));
}

#[test]
fn ladder_clip_lights_the_top_segment() {
    assert_eq!(ladder_counts(-60.0, true), (0, 0, 1));
    assert!(ladder_segments(-60.0, true)[LADDER_SEGMENTS - 1].lit);
}

#[test]
fn ear_holes_sit_inside_the_ear() {
    for height in [24.0, 34.0, 66.0, 190.0] {
        let holes = ear_holes(height);
        for (x, y) in holes {
            assert!(
                x - EAR_HOLE_RADIUS >= 0.0 && x + EAR_HOLE_RADIUS <= EAR_WIDTH,
                "{height}"
            );
            assert!(
                y - EAR_HOLE_RADIUS >= 0.0 && y + EAR_HOLE_RADIUS <= height,
                "{height}"
            );
        }
        assert!(holes[0].1 < holes[1].1, "{height}");
    }
    assert_eq!(ear_holes(66.0), [(15.0, 10.0), (15.0, 56.0)]);
    assert_eq!(ear_holes(190.0), [(15.0, 10.0), (15.0, 180.0)]);
}

const UNIT_EDGES: [f32; 5] = [100.0, 170.0, 240.0, 430.0, 494.0];

fn rack_ids() -> Vec<String> {
    ["compressor", "preamp", "cabinet", "chorus"]
        .map(String::from)
        .to_vec()
}

const WINDOW: ScreenRect = ScreenRect {
    left: 0.0,
    top: 0.0,
    right: 1440.0,
    bottom: 900.0,
};

const RACK_AREA: ScreenRect = ScreenRect {
    left: 300.0,
    top: 80.0,
    right: 1300.0,
    bottom: 800.0,
};

#[test]
fn drop_slot_above_the_middle_of_the_first_unit_is_the_front() {
    assert_eq!(drop_slot(40.0, &UNIT_EDGES), 0);
    assert_eq!(drop_slot(130.0, &UNIT_EDGES), 0);
}

#[test]
fn drop_slot_is_the_gap_nearest_the_pointer() {
    assert_eq!(drop_slot(140.0, &UNIT_EDGES), 1);
    assert_eq!(drop_slot(230.0, &UNIT_EDGES), 2);
    assert_eq!(drop_slot(300.0, &UNIT_EDGES), 2);
    assert_eq!(drop_slot(340.0, &UNIT_EDGES), 3);
}

#[test]
fn drop_slot_below_the_middle_of_the_last_unit_is_the_end() {
    assert_eq!(drop_slot(470.0, &UNIT_EDGES), 4);
    assert_eq!(drop_slot(900.0, &UNIT_EDGES), 4);
    assert_eq!(drop_slot(900.0, &[]), 0);
}

#[test]
fn palette_drop_at_a_slot_is_after_the_unit_above_it() {
    assert_eq!(palette_place(0, &rack_ids()), AddPlace::Front);
    assert_eq!(
        palette_place(2, &rack_ids()),
        AddPlace::After("preamp".to_string())
    );
    assert_eq!(
        palette_place(4, &rack_ids()),
        AddPlace::After("chorus".to_string())
    );
}

#[test]
fn unit_dropped_on_another_slot_moves_there() {
    let outcome = |from, y| {
        drag_outcome(
            &Dragged::Unit(from),
            Some((600.0, y)),
            &WINDOW,
            &RACK_AREA,
            &UNIT_EDGES,
            &rack_ids(),
        )
    };
    assert_eq!(
        outcome(0, 470.0),
        Some(Command::RackMove {
            id: "compressor".to_string(),
            to: 3,
        })
    );
    assert_eq!(
        outcome(3, 120.0),
        Some(Command::RackMove {
            id: "chorus".to_string(),
            to: 0,
        })
    );
    assert_eq!(
        outcome(0, 230.0),
        Some(Command::RackMove {
            id: "compressor".to_string(),
            to: 1,
        })
    );
}

#[test]
fn unit_dropped_beside_itself_stays() {
    for y in [150.0, 230.0] {
        let outcome = drag_outcome(
            &Dragged::Unit(1),
            Some((600.0, y)),
            &WINDOW,
            &RACK_AREA,
            &UNIT_EDGES,
            &rack_ids(),
        );
        assert_eq!(outcome, None);
    }
}

#[test]
fn unit_dropped_outside_the_rack_is_removed() {
    for pointer in [(100.0, 300.0), (600.0, 850.0)] {
        assert_eq!(
            drag_outcome(
                &Dragged::Unit(2),
                Some(pointer),
                &WINDOW,
                &RACK_AREA,
                &UNIT_EDGES,
                &rack_ids(),
            ),
            Some(Command::RackRemove {
                id: "cabinet".to_string(),
            })
        );
    }
}

#[test]
fn drag_released_outside_the_window_is_cancelled() {
    for dragged in [Dragged::Unit(2), Dragged::Tile("delay".to_string())] {
        assert_eq!(
            drag_outcome(
                &dragged,
                None,
                &WINDOW,
                &RACK_AREA,
                &UNIT_EDGES,
                &rack_ids()
            ),
            None
        );
    }
}

#[test]
fn drag_released_at_a_point_outside_the_window_is_cancelled() {
    for pointer in [(-20.0, 300.0), (600.0, 950.0), (1500.0, 300.0)] {
        assert_eq!(
            drag_outcome(
                &Dragged::Unit(2),
                Some(pointer),
                &WINDOW,
                &RACK_AREA,
                &UNIT_EDGES,
                &rack_ids(),
            ),
            None
        );
    }
}

#[test]
fn tile_dropped_in_the_rack_adds_at_the_slot() {
    let tile = Dragged::Tile("gate".to_string());
    assert_eq!(
        drag_outcome(
            &tile,
            Some((600.0, 90.0)),
            &WINDOW,
            &RACK_AREA,
            &UNIT_EDGES,
            &rack_ids()
        ),
        Some(add_command("gate", &AddPlace::Front))
    );
    assert_eq!(
        drag_outcome(
            &tile,
            Some((600.0, 300.0)),
            &WINDOW,
            &RACK_AREA,
            &UNIT_EDGES,
            &rack_ids()
        ),
        Some(Command::RackAdd {
            type_name: "gate".to_string(),
            after: Some("preamp".to_string()),
            at: None,
        })
    );
}

#[test]
fn tile_dropped_outside_the_rack_adds_nothing() {
    let tile = Dragged::Tile("gate".to_string());
    assert_eq!(
        drag_outcome(
            &tile,
            Some((100.0, 300.0)),
            &WINDOW,
            &RACK_AREA,
            &UNIT_EDGES,
            &rack_ids()
        ),
        None
    );
}

fn undo_payload() -> serde_json::Value {
    json!({"params": [
        {"module": "drive", "type": "drive", "param": "gain", "value": 30.0},
        {"module": "lead", "type": "drive", "param": "gain", "value": 80.0, "bypass": true},
        {"module": "lead", "type": "drive", "param": "tone", "value": 60.0, "bypass": true},
        {"module": "cabinet", "type": "cabinet", "param": "level", "value": 3.0,
            "ir": "bass-cab.wav"},
    ]})
}

/// [`requests_until_quit`] for commands that each send one request.
fn single_requests_until_quit(queued: Vec<Command>) -> (Vec<Request>, bool) {
    let (outgoing, quit) = requests_until_quit(queued, None, None);
    let requests = outgoing
        .into_iter()
        .map(|outgoing| match outgoing {
            Outgoing::Request(req) => req,
            other @ (Outgoing::Restore(_) | Outgoing::RestoreAutosave(_)) => {
                panic!("unexpected {other:?}")
            }
        })
        .collect();
    (requests, quit)
}

fn drive_restore() -> Restore {
    Restore {
        type_name: "drive".to_string(),
        after: Some("drive".to_string()),
        at: 1,
        values: vec![("gain".to_string(), 80.0), ("tone".to_string(), 60.0)],
        bypass: true,
        ir: None,
        track: None,
        bus: None,
        master: false,
    }
}

#[test]
fn restore_keeps_type_the_module_above_values_and_bypass() {
    assert_eq!(restore(&undo_payload(), "lead"), Some(drive_restore()));
    assert_eq!(
        restore(&undo_payload(), "drive").map(|restore| restore.after),
        Some(None)
    );
    assert_eq!(restore(&undo_payload(), "reverb"), None);
}

#[test]
fn restore_of_a_cabinet_keeps_its_ir() {
    let cabinet = restore(&undo_payload(), "cabinet").unwrap();
    assert_eq!(cabinet.ir.as_deref(), Some("bass-cab.wav"));
    assert_eq!(
        cabinet.follow_up_requests("cabinet"),
        [
            Request::Set {
                param: "cabinet.level".to_string(),
                value: 3.0,
                track: None,
                bus: None,
                master: false,
            },
            Request::RackIr {
                ir: "bass-cab.wav".to_string(),
                id: Some("cabinet".to_string()),
                track: None,
                bus: None,
                master: false,
            },
        ]
    );
}

#[test]
fn restore_adds_after_the_module_above_or_at_its_old_index() {
    let present = ["drive", "drive-2", "cabinet"].map(String::from);
    assert_eq!(
        drive_restore().add_request(&present),
        Request::RackAdd {
            r#type: "drive".to_string(),
            after: Some("drive".to_string()),
            at: None,
            track: None,
            bus: None,
            master: false,
        }
    );
    assert_eq!(
        drive_restore().add_request(&["cabinet".to_string()]),
        Request::RackAdd {
            r#type: "drive".to_string(),
            after: None,
            at: Some(1),
            track: None,
            bus: None,
            master: false,
        }
    );
}

#[test]
fn restore_follow_ups_address_the_id_rack_add_answered() {
    assert_eq!(
        drive_restore().follow_up_requests("drive-3"),
        [
            Request::Set {
                param: "drive-3.gain".to_string(),
                value: 80.0,
                track: None,
                bus: None,
                master: false,
            },
            Request::Set {
                param: "drive-3.tone".to_string(),
                value: 60.0,
                track: None,
                bus: None,
                master: false,
            },
            Request::Bypass {
                id: "drive-3".to_string(),
                on: true,
                track: None,
                bus: None,
                master: false,
            },
        ]
    );
}

#[test]
fn restore_goes_out_in_queue_order() {
    let queued = vec![
        Command::Load("Funk".to_string()),
        Command::Restore(drive_restore()),
        Command::Quit,
    ];
    assert_eq!(
        requests_until_quit(queued, None, None),
        (
            vec![
                Outgoing::Request(Request::Load {
                    preset: "Funk".to_string(),
                    track: None,
                    bus: None,
                    master: false,
                }),
                Outgoing::Restore(drive_restore()),
            ],
            true
        )
    );
}

fn funk_status() -> serde_json::Value {
    json!({"preset": "Funk", "program": 2, "revision": 4})
}

#[test]
fn removed_unit_offers_undo_for_ten_seconds() {
    let now = Instant::now();
    let status = funk_status();
    let removed = RemovedUnit::new(&undo_payload(), Some(&status), "lead", now).unwrap();
    assert_eq!(removed.id, "lead");
    assert_eq!(removed.restore, drive_restore());
    let later = json!({"preset": "Funk", "program": 2, "revision": 9});
    assert!(removed.is_shown(Some(&later), now + Duration::from_millis(9_999)));
    assert!(!removed.is_shown(Some(&later), now + Duration::from_secs(10)));
}

#[test]
fn removed_unit_offers_no_undo_after_the_preset_changes() {
    let now = Instant::now();
    let removed = RemovedUnit::new(&undo_payload(), Some(&funk_status()), "lead", now).unwrap();
    for other in [
        json!({"preset": "Blues", "program": 1, "revision": 5}),
        json!({"preset": "Funk", "program": 0, "revision": 5}),
    ] {
        assert!(!removed.is_shown(Some(&other), now));
    }
    assert!(!removed.is_shown(None, now));
}

#[test]
fn undo_dropped_by_a_preset_change_never_comes_back() {
    let now = Instant::now();
    let mut removed = RemovedUnit::new(&undo_payload(), Some(&funk_status()), "lead", now);
    assert!(shown_undo(&mut removed, Some(&funk_status()), now).is_some());
    let blues = json!({"preset": "Blues", "program": 1, "revision": 5});
    assert!(shown_undo(&mut removed, Some(&blues), now).is_none());
    assert!(shown_undo(&mut removed, Some(&funk_status()), now).is_none());
}

#[test]
fn removed_unit_absent_from_the_payload_offers_no_undo() {
    let status = funk_status();
    assert_eq!(
        RemovedUnit::new(&undo_payload(), Some(&status), "reverb", Instant::now()),
        None
    );
}

#[test]
fn peak_hold_holds_a_spike_for_two_seconds() {
    let start = Instant::now();
    let mut hold = PeakHold::default();
    hold.observe(-3.0, start);
    hold.observe(-30.0, start + Duration::from_millis(500));
    hold.observe(-40.0, start + Duration::from_millis(1990));
    assert_eq!(hold.held_db(), -3.0);
    hold.observe(-1.0, start + Duration::from_millis(1995));
    assert_eq!(hold.held_db(), -1.0);
}

#[test]
fn peak_hold_releases_to_the_current_value_after_two_seconds() {
    let start = Instant::now();
    let mut hold = PeakHold::default();
    hold.observe(-3.0, start);
    hold.observe(-40.0, start + Duration::from_secs(2));
    assert_eq!(hold.held_db(), -40.0);
    hold.observe(-50.0, start + Duration::from_millis(2100));
    assert_eq!(hold.held_db(), -40.0);
}

#[test]
fn clip_latch_latches_at_zero_db() {
    let mut latch = ClipLatch::default();
    latch.observe(-0.1);
    assert!(!latch.is_clipped());
    latch.observe(0.0);
    assert!(latch.is_clipped());
}

#[test]
fn clip_latch_stays_when_the_level_drops() {
    let mut latch = ClipLatch::default();
    latch.observe(2.5);
    latch.observe(-40.0);
    latch.observe(-120.0);
    assert!(latch.is_clipped());
}

#[test]
fn clip_latch_clears_on_reset() {
    let mut latch = ClipLatch::default();
    latch.observe(1.0);
    latch.reset();
    assert!(!latch.is_clipped());
    latch.observe(-3.0);
    assert!(!latch.is_clipped());
}

fn knob_row(min: f64, max: f64, default: f64, integer: bool) -> ParamRow {
    ParamRow {
        address: "preamp.gain".to_string(),
        module: "preamp".to_string(),
        module_type: "preamp".to_string(),
        param: "gain".to_string(),
        value: min,
        min,
        max,
        default,
        unit: String::new(),
        integer,
        labels: Vec::new(),
    }
}

#[test]
fn knob_drag_fine_moves_a_tenth() {
    let row = knob_row(-60.0, 20.0, 0.0, false);
    let coarse = knob_value_from_drag(0.5, 0.0, 1.0, -22.0, false, false);
    let fine = knob_value_from_drag(0.5, 0.0, 1.0, -22.0, true, false);
    assert!((knob_param_value(&row, coarse, false) - -12.0).abs() < 1e-4);
    assert!((knob_param_value(&row, fine, false) - -19.2).abs() < 1e-4);
}

#[test]
fn knob_drag_integer_parameter_steps() {
    let row = knob_row(1.0, 4.0, 2.0, true);
    assert_eq!(knob_param_value(&row, 0.4, false), 2.0);
    assert_eq!(knob_param_value(&row, 0.55, false), 3.0);
    assert_eq!(knob_param_value(&row, 1.0, false), 4.0);
}

#[test]
fn knob_drag_reset_gives_the_default() {
    let row = knob_row(-60.0, 20.0, -6.0, false);
    assert_eq!(knob_param_value(&row, 0.9, true), -6.0);
}

#[test]
fn open_units_round_trip_through_their_string() {
    let open: BTreeSet<String> = ["cabinet", "preamp"].map(str::to_string).into();
    assert_eq!(open_units_from_string(&open_units_to_string(&open)), open);
    assert_eq!(open_units_from_string("not json"), BTreeSet::new());
}

fn bank_row(program: usize, name: &str) -> BankRow {
    BankRow {
        program,
        name: name.to_string(),
        active: false,
        error: None,
    }
}

fn filter_rows() -> Vec<BankRow> {
    vec![
        bank_row(0, "Clean"),
        bank_row(1, "Crunch"),
        bank_row(2, "High Gain"),
        bank_row(12, "Lead"),
    ]
}

#[test]
fn filter_bank_matches_a_name_substring_in_any_case() {
    let names: Vec<String> = filter_bank(&filter_rows(), "cR")
        .into_iter()
        .map(|row| row.name)
        .collect();
    assert_eq!(names, ["Crunch"]);
    assert_eq!(
        filter_bank(&filter_rows(), "GAIN"),
        [bank_row(2, "High Gain")]
    );
}

#[test]
fn filter_bank_matches_the_program_number() {
    assert_eq!(filter_bank(&filter_rows(), "12"), [bank_row(12, "Lead")]);
    assert_eq!(filter_bank(&filter_rows(), " 1 "), [bank_row(1, "Crunch")]);
}

#[test]
fn filter_bank_empty_query_keeps_every_row() {
    assert_eq!(filter_bank(&filter_rows(), ""), filter_rows());
    assert_eq!(filter_bank(&filter_rows(), "  "), filter_rows());
}

#[test]
fn latency_ms_is_one_buffer() {
    assert!((latency_ms(128, 48000) - 2.667).abs() < 0.001);
    assert!((latency_ms(256, 44100) - 5.805).abs() < 0.001);
}

#[test]
fn engine_readout_shows_the_engine_numbers() {
    let status = json!({"sample_rate": 48000, "buffer_size": 128, "xruns": 3});
    let rows: Vec<(String, String)> = [
        ("Rate", "48 kHz"),
        ("Buffer", "128 frames"),
        ("Latency", "2.7 ms"),
        ("Xruns", "3"),
        ("Daemon", "connected"),
    ]
    .map(|(label, value)| (label.to_string(), value.to_string()))
    .into();
    assert_eq!(engine_readout(Some(&status)), rows);
    let status = json!({"sample_rate": 44100, "buffer_size": 64, "xruns": 0});
    assert_eq!(engine_readout(Some(&status))[0].1, "44.1 kHz");
}

#[test]
fn engine_readout_disconnected() {
    assert_eq!(
        engine_readout(None),
        [("Daemon".to_string(), "disconnected".to_string())]
    );
}

#[test]
fn save_as_command_maps_to_request() {
    assert_eq!(
        single_requests_until_quit(vec![Command::SaveAs("Funk".to_string())]),
        (
            vec![Request::Save {
                name: Some("Funk".to_string())
            }],
            false
        )
    );
}

fn get_with_modules(ids: &[&str]) -> serde_json::Value {
    let params: Vec<_> = ids
        .iter()
        .map(|id| json!({"module": id, "type": id, "param": "level", "value": 0.0}))
        .collect();
    json!({ "params": params })
}

#[test]
fn open_units_keep_a_refused_remove_open() {
    let mut open: BTreeSet<String> = ["cabinet", "preamp"].map(str::to_string).into();
    let mut removing = BTreeSet::new();
    remember_removes(
        &mut removing,
        &[Command::RackRemove {
            id: "preamp".into(),
        }],
    );
    assert_eq!(open, ["cabinet", "preamp"].map(str::to_string).into());
    forget_removed_units(
        &mut open,
        &mut removing,
        &get_with_modules(&["preamp", "cabinet"]),
    );
    assert_eq!(open, ["cabinet", "preamp"].map(str::to_string).into());
}

#[test]
fn open_units_stay_open_when_a_later_preset_lacks_a_refused_remove() {
    let mut open: BTreeSet<String> = ["cabinet", "preamp"].map(str::to_string).into();
    let mut removing = BTreeSet::new();
    remember_removes(
        &mut removing,
        &[Command::RackRemove {
            id: "preamp".into(),
        }],
    );
    forget_removed_units(
        &mut open,
        &mut removing,
        &get_with_modules(&["preamp", "cabinet"]),
    );
    forget_removed_units(&mut open, &mut removing, &get_with_modules(&["gate"]));
    assert_eq!(open, ["cabinet", "preamp"].map(str::to_string).into());
}

#[test]
fn refused_remove_names_its_id_only_on_an_error() {
    let remove = Request::RackRemove {
        id: "preamp".into(),
        track: None,
        bus: None,
        master: false,
    };
    let refused = Ok(Response::Err {
        error: "busy".into(),
    });
    assert_eq!(refused_remove(&remove, &refused).as_deref(), Some("preamp"));
    assert_eq!(refused_remove(&remove, &Ok(Response::Ok(json!({})))), None);
    assert_eq!(
        refused_remove(&Request::Save { name: None }, &refused),
        None
    );
}

#[test]
fn open_units_drop_a_confirmed_remove() {
    let mut open: BTreeSet<String> = ["cabinet", "preamp"].map(str::to_string).into();
    let mut removing = BTreeSet::new();
    remember_removes(
        &mut removing,
        &[
            Command::Load("lead".into()),
            Command::RackRemove {
                id: "preamp".into(),
            },
        ],
    );
    forget_removed_units(&mut open, &mut removing, &get_with_modules(&["cabinet"]));
    assert_eq!(open, ["cabinet"].map(str::to_string).into());
    assert!(removing.is_empty());
}

#[test]
fn open_units_keep_a_module_another_preset_lacks() {
    let mut open: BTreeSet<String> = ["cabinet", "preamp"].map(str::to_string).into();
    let mut removing = BTreeSet::new();
    remember_removes(&mut removing, &[Command::Load("lead".into())]);
    forget_removed_units(&mut open, &mut removing, &get_with_modules(&["gate"]));
    assert_eq!(open, ["cabinet", "preamp"].map(str::to_string).into());
}

#[test]
fn shortcuts_work_unless_a_text_field_is_being_typed_in() {
    assert!(shortcuts_active(false));
    assert!(!shortcuts_active(true));
}

#[test]
fn key_step_moves_one_percent_ten_percent_or_to_an_end() {
    let row = knob_row(-60.0, 20.0, 0.0, false);
    assert!((key_step(&row, 0.0, KnobKey::Up) - 0.8).abs() < 1e-9);
    assert!((key_step(&row, 0.0, KnobKey::Down) - -0.8).abs() < 1e-9);
    assert!((key_step(&row, 0.0, KnobKey::PageUp) - 8.0).abs() < 1e-9);
    assert!((key_step(&row, 0.0, KnobKey::PageDown) - -8.0).abs() < 1e-9);
    assert_eq!(key_step(&row, 0.0, KnobKey::Home), -60.0);
    assert_eq!(key_step(&row, 0.0, KnobKey::End), 20.0);
    assert_eq!(key_step(&row, 19.5, KnobKey::PageUp), 20.0);
    assert_eq!(key_step(&row, -59.5, KnobKey::Down), -60.0);
}

#[test]
fn key_step_of_an_integer_parameter_moves_whole_numbers() {
    let stages = knob_row(1.0, 4.0, 2.0, true);
    assert_eq!(key_step(&stages, 2.0, KnobKey::Up), 3.0);
    assert_eq!(key_step(&stages, 2.0, KnobKey::Down), 1.0);
    assert_eq!(key_step(&stages, 2.0, KnobKey::PageUp), 3.0);
    assert_eq!(key_step(&stages, 4.0, KnobKey::Up), 4.0);
    let wide = knob_row(0.0, 100.0, 0.0, true);
    assert_eq!(key_step(&wide, 50.0, KnobKey::PageDown), 40.0);
}

#[test]
fn typed_values_parse_with_a_unicode_minus_or_a_unit() {
    assert_eq!(parse_typed_value("-8"), Some(-8.0));
    assert_eq!(parse_typed_value("\u{2212}8"), Some(-8.0));
    assert_eq!(parse_typed_value(" 12.5 "), Some(12.5));
    assert_eq!(parse_typed_value("12 dB"), Some(12.0));
    assert_eq!(parse_typed_value("350ms"), Some(350.0));
    assert_eq!(parse_typed_value("2 kHz"), Some(2000.0));
    assert_eq!(parse_typed_value("2k"), Some(2000.0));
    assert_eq!(parse_typed_value("2.5 KHZ"), Some(2500.0));
    assert_eq!(parse_typed_value(""), None);
    assert_eq!(parse_typed_value("dB"), None);
    assert_eq!(parse_typed_value("loud"), None);
    assert_eq!(parse_typed_value("nan"), None);
    assert_eq!(parse_typed_value("inf"), None);
}

#[test]
fn autoscroll_only_near_an_edge_and_faster_closer_to_it() {
    let (top, bottom) = (100.0, 600.0);
    assert_eq!(autoscroll_delta(350.0, top, bottom), 0.0);
    assert_eq!(autoscroll_delta(141.0, top, bottom), 0.0);
    assert_eq!(autoscroll_delta(559.0, top, bottom), 0.0);
    let near_top = autoscroll_delta(130.0, top, bottom);
    let at_top = autoscroll_delta(101.0, top, bottom);
    assert!(near_top < 0.0 && at_top < near_top);
    let near_bottom = autoscroll_delta(570.0, top, bottom);
    let at_bottom = autoscroll_delta(599.0, top, bottom);
    assert!(near_bottom > 0.0 && at_bottom > near_bottom);
    assert_eq!(
        autoscroll_delta(20.0, top, bottom),
        autoscroll_delta(100.0, top, bottom)
    );
    assert_eq!(
        autoscroll_delta(700.0, top, bottom),
        autoscroll_delta(600.0, top, bottom)
    );
}

#[test]
fn tuner_header_shows_note_and_cents_or_no_signal() {
    let reading = TunerReading {
        note: "E1".into(),
        hz: 41.2,
        cents: -3,
    };
    assert_eq!(tuner_header_text(Some(&reading)), "E1  \u{2212}3 ct");
    assert_eq!(tuner_header_text(None), NO_SIGNAL);
}

#[test]
fn peak_text_without_a_status_shows_no_value() {
    assert_eq!(peak_text(Some(-4.2), false), "\u{2212}4.2 dB");
    assert_eq!(peak_text(Some(0.3), true), "+0.3 dB, clipped");
    assert_eq!(peak_text(None, false), "—");
}

fn spec(param: &str, unit: &str, integer: bool) -> ParamRow {
    ParamRow {
        param: param.to_string(),
        unit: unit.to_string(),
        ..knob_row(0.0, 1.0, 0.0, integer)
    }
}

#[test]
fn format_value_like_the_mockup() {
    let cases = [
        (spec("threshold", "dB", false), -24.0, "\u{2212}24 dB"),
        (spec("attack", "ms", false), 5.0, "5 ms"),
        (spec("ratio", "", false), 6.0, "6 : 1"),
        (spec("ratio", "", false), 2.5, "2.5 : 1"),
        (spec("high_cut", "Hz", true), 20000.0, "20 kHz"),
        (spec("high_cut", "Hz", true), 2500.0, "2.5 kHz"),
        (spec("low_cut", "Hz", true), 80.0, "80 Hz"),
        (spec("rate", "Hz", false), 0.8, "0.8 Hz"),
        (spec("gain", "%", false), 15.0, "15 %"),
        (spec("bias", "", false), -0.25, "\u{2212}0.3"),
        (spec("bias", "", false), -0.04, "0"),
        (spec("stages", "", true), 2.0, "2"),
        (spec("q", "", false), 3.0, "3"),
        (spec("mid1_q", "", false), 0.707, "0.71"),
        (spec("mid2_q", "", false), 2.5, "2.5"),
    ];
    for (row, value, expected) in cases {
        assert_eq!(format_value(&row, value), expected, "{} {value}", row.param);
    }
}

#[test]
fn typed_text_is_the_bare_number_in_the_parameter_unit() {
    assert_eq!(typed_text(&spec("high_cut", "Hz", true), 20000.0), "20000");
    assert_eq!(typed_text(&spec("ratio", "", false), 6.0), "6");
    assert_eq!(
        typed_text(&spec("threshold", "dB", false), -24.0),
        "\u{2212}24"
    );
    let text = typed_text(&spec("rate", "Hz", false), 0.8);
    assert_eq!(parse_typed_value(&text), Some(0.8));
}

#[test]
fn typed_value_refuses_text_that_is_no_number() {
    assert_eq!(typed_value("\u{2212}30 dB"), Ok(-30.0));
    assert_eq!(
        typed_value("loud"),
        Err("error: 'loud' is not a number".to_string())
    );
}

#[test]
fn param_label_in_sentence_case() {
    assert_eq!(param_label("low_cut"), "Low cut");
    assert_eq!(param_label("threshold"), "Threshold");
    assert_eq!(param_label(""), "");
}

#[test]
fn sections_in_panel_order() {
    assert_eq!(
        SECTIONS,
        [
            Section::Project,
            Section::Preset,
            Section::Bank,
            Section::Rack,
            Section::Tuner,
            Section::Metronome,
            Section::Audio,
            Section::Settings,
            Section::Engine,
        ]
    );
    let titles: Vec<&str> = SECTIONS
        .iter()
        .map(|section| section.title(BankKind::Bass))
        .collect();
    assert_eq!(
        titles,
        [
            "Project",
            "Preset",
            "Bass Bank",
            "Rack",
            "Tuner and Mute",
            "Metronome",
            "Audio",
            "Settings",
            "Engine"
        ]
    );
    assert_eq!(Section::Bank.title(BankKind::Guitar), "Guitar Bank");
    let closed: Vec<Section> = SECTIONS
        .into_iter()
        .filter(|section| !section.open_by_default())
        .collect();
    assert_eq!(closed, [Section::Audio, Section::Settings]);
}

#[test]
fn bank_kind_follows_settings_else_config() {
    let bass = json!({"bank_kind": "bass", "bank_dir": "/b", "ir_dir": "/b"});
    assert_eq!(bank_kind(Some(&bass), BankKind::Guitar), BankKind::Bass);
    assert_eq!(bank_kind(None, BankKind::Bass), BankKind::Bass);
    assert_eq!(
        bank_kind(Some(&json!({})), BankKind::Guitar),
        BankKind::Guitar
    );
}

const CAPTURE: &str = "Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO";
const LEFT: &str = "Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL";
const RIGHT: &str = "Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR";

#[test]
fn port_choices_from_a_ports_payload() {
    let payload = json!({"ok": true, "capture": [CAPTURE, "other:capture_FL"],
        "playback": [LEFT, RIGHT],
        "connected": {"input": CAPTURE, "output_left": LEFT, "output_right": null}});
    assert_eq!(
        port_choices(&payload),
        PortChoices {
            capture: vec![CAPTURE.to_string(), "other:capture_FL".to_string()],
            playback: vec![LEFT.to_string(), RIGHT.to_string()],
            connected: PortSelection {
                input: Some(CAPTURE.to_string()),
                output_left: Some(LEFT.to_string()),
                output_right: None,
            },
        }
    );
    assert_eq!(port_choices(&json!({})), PortChoices::default());
}

#[test]
fn set_ports_needs_all_three_ports() {
    let mut selection = PortSelection {
        input: Some(CAPTURE.to_string()),
        output_left: Some(LEFT.to_string()),
        output_right: None,
    };
    assert_eq!(set_ports_command(&selection), None);
    selection.output_right = Some(RIGHT.to_string());
    assert_eq!(
        set_ports_command(&selection),
        Some(Command::SetPorts {
            input: CAPTURE.to_string(),
            output_left: LEFT.to_string(),
            output_right: RIGHT.to_string(),
        })
    );
}

#[test]
fn set_settings_sends_only_what_changed() {
    let payload = json!({"ok": true, "bank_kind": "guitar", "bank_dir": "/c/bank", "ir_dir": "/c/bank",
            "projects_dir": "/c/music"});
    let current = settings_draft(&payload).unwrap();
    assert_eq!(
        current,
        SettingsDraft {
            bank_kind: BankKind::Guitar,
            ir_dir: "/c/bank".to_string(),
            projects_dir: "/c/music".to_string(),
        }
    );
    assert_eq!(settings_draft(&json!({})), None);
    assert_eq!(set_settings_command(&current, &current), None);
    let bass = SettingsDraft {
        bank_kind: BankKind::Bass,
        ..current.clone()
    };
    assert_eq!(
        set_settings_command(&current, &bass),
        Some(Command::SetSettings {
            bank_kind: Some(BankKind::Bass),
            ir_dir: None,
            projects_dir: None,
        })
    );
    let irs = SettingsDraft {
        ir_dir: " /c/irs ".to_string(),
        ..current.clone()
    };
    assert_eq!(
        set_settings_command(&current, &irs),
        Some(Command::SetSettings {
            bank_kind: None,
            ir_dir: Some(PathBuf::from("/c/irs")),
            projects_dir: None,
        })
    );
    let music = SettingsDraft {
        projects_dir: "/c/songs".to_string(),
        ..current.clone()
    };
    assert_eq!(
        set_settings_command(&current, &music),
        Some(Command::SetSettings {
            bank_kind: None,
            ir_dir: None,
            projects_dir: Some(PathBuf::from("/c/songs")),
        })
    );
}

#[test]
fn ports_and_settings_commands_become_requests() {
    let queued = vec![
        Command::RefreshPorts,
        Command::SetPorts {
            input: CAPTURE.to_string(),
            output_left: LEFT.to_string(),
            output_right: RIGHT.to_string(),
        },
        Command::SetSettings {
            bank_kind: Some(BankKind::Bass),
            ir_dir: Some(PathBuf::from("/c/irs")),
            projects_dir: None,
        },
    ];
    assert_eq!(
        single_requests_until_quit(queued),
        (
            vec![
                Request::SetPorts {
                    input: CAPTURE.to_string(),
                    output_left: LEFT.to_string(),
                    output_right: RIGHT.to_string(),
                },
                Request::SetSettings {
                    bank_kind: Some(BankKind::Bass),
                    ir_dir: Some(PathBuf::from("/c/irs")),
                    projects_dir: None,
                },
            ],
            false
        )
    );
}

#[test]
fn closed_unit_wraps_knobs_that_do_not_fit() {
    assert_eq!(closed_unit_rows(5, 6.0 * CLOSED_KNOB_WIDTH), 1);
    assert_eq!(closed_unit_rows(5, 5.0 * CLOSED_KNOB_WIDTH), 1);
    assert_eq!(closed_unit_rows(5, 3.5 * CLOSED_KNOB_WIDTH), 2);
    assert_eq!(closed_unit_rows(7, 3.0 * CLOSED_KNOB_WIDTH), 3);
    assert_eq!(closed_unit_rows(3, 10.0), 3);
    assert_eq!(closed_unit_rows(0, 500.0), 1);
}

#[test]
fn module_title_is_sentence_case_with_a_differing_type() {
    assert_eq!(
        module_title("compressor", "compressor"),
        ("Compressor".to_string(), None)
    );
    assert_eq!(
        module_title("low_drive", "drive"),
        ("Low drive".to_string(), Some("drive"))
    );
    assert_eq!(module_title("amp", ""), ("Amp".to_string(), None));
    assert_eq!(module_title("eq", "eq"), ("EQ".to_string(), None));
}

fn choice_row(param: &str, labels: &[&str], value: f64) -> ParamRow {
    ParamRow {
        address: format!("autowah.{param}"),
        module: "autowah".to_string(),
        module_type: "autowah".to_string(),
        param: param.to_string(),
        value,
        min: 0.0,
        max: (labels.len() - 1) as f64,
        default: 0.0,
        unit: String::new(),
        integer: true,
        labels: labels.iter().map(ToString::to_string).collect(),
    }
}

#[test]
fn param_rows_carry_labels() {
    let get = json!({"params": [
        {"module": "autowah", "type": "autowah", "param": "mode", "value": 1.0, "unit": "", "min": 0.0, "max": 2.0, "default": 0.0, "integer": true, "labels": ["lp", "bp", "hp"]},
        {"module": "autowah", "type": "autowah", "param": "q", "value": 3.0, "unit": "", "min": 0.5, "max": 12.0, "default": 3.0, "integer": false, "labels": []},
    ]});
    let rows = &param_rows(&get)[0].1;
    assert_eq!(rows[0].labels, ["lp", "bp", "hp"]);
    assert_eq!(rows[1].labels, Vec::<String>::new());
}

#[test]
fn three_choices_sit_side_by_side_in_the_column() {
    let buttons = choice_buttons(&choice_row("mode", &["lp", "bp", "hp"], 1.0), 84.0);
    assert_eq!(
        buttons,
        [
            ChoiceButton {
                text: "LP".to_string(),
                accessible_name: "Mode, LP".to_string(),
                lit: false,
                value: 0.0,
                cell: (0.0, 0.0, 28.0, 32.0),
            },
            ChoiceButton {
                text: "BP".to_string(),
                accessible_name: "Mode, BP".to_string(),
                lit: true,
                value: 1.0,
                cell: (28.0, 0.0, 28.0, 32.0),
            },
            ChoiceButton {
                text: "HP".to_string(),
                accessible_name: "Mode, HP".to_string(),
                lit: false,
                value: 2.0,
                cell: (56.0, 0.0, 28.0, 32.0),
            },
        ]
    );
}

#[test]
fn two_choices_stack_full_width() {
    let buttons = choice_buttons(&choice_row("direction", &["up", "down"], 0.0), 84.0);
    let summary: Vec<_> = buttons
        .iter()
        .map(|b| (b.text.as_str(), b.accessible_name.as_str(), b.lit, b.cell))
        .collect();
    assert_eq!(
        summary,
        [
            ("Up", "Direction, Up", true, (0.0, 0.0, 84.0, 32.0)),
            ("Down", "Direction, Down", false, (0.0, 36.0, 84.0, 32.0)),
        ]
    );
    let hertz = choice_buttons(&choice_row("bass_freq", &["40 Hz", "80 Hz"], 1.0), 84.0);
    assert_eq!(hertz[1].text, "80 Hz");
    assert_eq!(hertz[1].accessible_name, "Bass freq, 80 Hz");
}

#[test]
fn arrow_keys_move_within_the_choices_and_stop_at_the_ends() {
    let row = choice_row("mode", &["lp", "bp", "hp"], 1.0);
    assert_eq!(choice_step(&row, ChoiceKey::Next), 2.0);
    assert_eq!(choice_step(&row, ChoiceKey::Previous), 0.0);
    let last = choice_row("mode", &["lp", "bp", "hp"], 2.0);
    assert_eq!(choice_step(&last, ChoiceKey::Next), 2.0);
    let first = choice_row("mode", &["lp", "bp", "hp"], 0.0);
    assert_eq!(choice_step(&first, ChoiceKey::Previous), 0.0);
}

#[test]
fn closed_chip_toggles_two_choices_and_opens_for_three() {
    assert_eq!(
        chip_click(&choice_row("direction", &["up", "down"], 0.0)),
        ChipClick::Set(1.0)
    );
    assert_eq!(
        chip_click(&choice_row("direction", &["up", "down"], 1.0)),
        ChipClick::Set(0.0)
    );
    assert_eq!(
        chip_click(&choice_row("mode", &["lp", "bp", "hp"], 1.0)),
        ChipClick::Open
    );
}

#[test]
fn open_knobs_wrap_between_autowah_groups() {
    let rows =
        |width_in_knobs: f32| open_knob_rows("autowah", 10, width_in_knobs * OPEN_KNOB_PITCH);
    assert_eq!(rows(10.0), [10]);
    assert_eq!(rows(9.5), [8, 2]);
    assert_eq!(rows(7.0), [5, 5]);
    assert_eq!(rows(5.0), [5, 5]);
    assert_eq!(rows(4.0), [2, 3, 3, 2]);
    assert_eq!(rows(2.5), [2, 2, 1, 2, 1, 2]);
}

#[test]
fn open_knobs_wrap_between_blend_groups() {
    let rows = |width_in_knobs: f32| open_knob_rows("blend", 10, width_in_knobs * OPEN_KNOB_PITCH);
    assert_eq!(rows(10.0), [10]);
    assert_eq!(rows(9.5), [7, 3]);
    assert_eq!(rows(7.0), [7, 3]);
    assert_eq!(rows(5.0), [3, 4, 3]);
    assert_eq!(rows(3.5), [3, 3, 1, 3]);
}

#[test]
fn other_types_are_one_group_that_wraps_where_full() {
    assert_eq!(open_knob_rows("compressor", 5, 10.0 * OPEN_KNOB_PITCH), [5]);
    assert_eq!(
        open_knob_rows("compressor", 5, 3.0 * OPEN_KNOB_PITCH),
        [3, 2]
    );
    assert_eq!(open_knob_rows("cabinet", 3, 0.0), [1, 1, 1]);
    assert_eq!(open_knob_rows("gain", 0, 500.0), Vec::<usize>::new());
}

#[test]
fn open_unit_grows_by_a_row_per_extra_row() {
    assert_eq!(open_unit_height(0), OPEN_UNIT_HEIGHT);
    assert_eq!(open_unit_height(1), OPEN_UNIT_HEIGHT);
    let one_more = open_unit_height(2) - OPEN_UNIT_HEIGHT;
    assert!(one_more > 100.0, "{one_more}");
    assert_eq!(open_unit_height(3) - open_unit_height(2), one_more);
}

#[test]
fn ear_numbers_follow_rack_order_through_a_move_an_add_and_a_remove() {
    let labels = |ids: &[&str]| {
        let ids: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
        rack_position_labels(&ids)
    };
    let expected = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, label)| ((*id).to_string(), (*label).to_string()))
            .collect()
    };
    assert_eq!(
        labels(&["gate", "drive", "delay"]),
        expected(&[("gate", "1"), ("drive", "2"), ("delay", "3")])
    );
    assert_eq!(
        labels(&["delay", "gate", "drive"]),
        expected(&[("delay", "1"), ("gate", "2"), ("drive", "3")]),
        "after delay moves to the top"
    );
    assert_eq!(
        labels(&["delay", "gate", "drive", "reverb"]),
        expected(&[
            ("delay", "1"),
            ("gate", "2"),
            ("drive", "3"),
            ("reverb", "4")
        ]),
        "after reverb is added"
    );
    assert_eq!(
        labels(&["delay", "drive", "reverb"]),
        expected(&[("delay", "1"), ("drive", "2"), ("reverb", "3")]),
        "after gate is removed"
    );
    assert_eq!(labels(&[]), expected(&[]));
}
