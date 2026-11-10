use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use xguitare_client::ClientError;
use xguitare_core::control::Request;
use xguitare_core::control::Response;
use xguitare_core::control::{EngineState, PeakMeters, handle};
use xguitare_core::engine::{EngineShared, engine_queues};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, Config};
use xguitare_gui::model::{
    Band, BankRow, ClipLatch, Command, Direction, EAR_HOLE_RADIUS, EAR_WIDTH, Feedback, KeyEvent,
    KeyModifiers, LADDER_SEGMENTS, Link, ModuleType, NEUTRAL_ACCENT, ParamRow, Rgb, Throttle,
    TunerReading, accent_for, add_after, add_command, bank_rows, cabinet_ir, cents_fraction,
    cents_text, counts_toward_spawn, drop_index, ear_holes, failed_set, in_tune, ir_names,
    key_to_command, knob_angle_deg, knob_fraction, knob_param_value, knob_points,
    knob_value_from_drag, ladder_segments, meter_fraction, module_types, move_target,
    open_units_from_string, open_units_to_string, palette, param_rows, peak_fraction,
    quit_commands, rack_channels, refresh_decision, requests_until_quit, response_text,
    spawn_policy, status_line, tuner_reading, tuner_toggle, unit_suffix,
};

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
    assert!(!spawn_policy(3, Some(now - Duration::from_secs(1)), now));
    assert!(!spawn_policy(
        9,
        Some(now - Duration::from_millis(4_999)),
        now
    ));
    assert!(spawn_policy(4, Some(now - Duration::from_secs(5)), now));
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
        },
        Request::RackAdd {
            r#type: "delay".to_string(),
            after: Some("reverb".to_string()),
            at: None,
        },
        Request::RackAdd {
            r#type: "gate".to_string(),
            after: None,
            at: Some(0),
        },
        Request::RackMove {
            id: "delay".to_string(),
            to: 4,
        },
        Request::RackRemove {
            id: "delay".to_string(),
        },
    ];
    assert_eq!(requests_until_quit(queued), (expected, true));
    assert_eq!(
        requests_until_quit(vec![Command::Load("Lead".to_string())]),
        (
            vec![Request::Load {
                preset: "Lead".to_string()
            }],
            false
        )
    );
}

#[test]
fn meter_fraction_bounds() {
    assert_eq!(meter_fraction(-120.0), 0.0);
    assert_eq!(meter_fraction(-60.0), 0.0);
    assert_eq!(meter_fraction(-30.0), 0.5);
    assert_eq!(meter_fraction(0.0), 1.0);
    assert_eq!(meter_fraction(3.0), 1.0);
}

#[test]
fn null_or_missing_peak_reads_as_silence() {
    let status = json!({"input_peak_db": null, "output_peak_db": -30.0});
    assert_eq!(peak_fraction(&status, "input_peak_db"), 0.0);
    assert_eq!(peak_fraction(&status, "output_peak_db"), 0.5);
    assert_eq!(peak_fraction(&json!({}), "output_peak_db"), 0.0);
}

#[test]
fn refresh_decision_on_revision_change() {
    assert!(refresh_decision(None, 1));
    assert!(!refresh_decision(Some(1), 1));
    assert!(refresh_decision(Some(1), 2));
}

#[test]
fn status_line_format() {
    let status = json!({
        "preset": "clean",
        "sample_rate": 48000,
        "buffer_size": 128,
        "prepared_frames": 128,
        "revision": 4,
        "input_peak_db": -20.5,
        "output_peak_db": -18.0,
        "xruns": 0,
        "callback_max_us": 40,
        "callback_last_us": 12,
        "connections": [],
    });
    assert_eq!(
        status_line(&status),
        "48000 Hz · 128/128 frames · xruns 0 · cb 12/40 µs"
    );
}

#[test]
fn link_text() {
    let now = Instant::now();
    let since = now - Duration::from_millis(12_400);
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
                since: now - Duration::from_secs(3)
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
    assert!(param_rows(&json!({})).is_empty());
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
    assert!(throttle.tick(at(20)).is_empty());
    assert_eq!(
        throttle.tick(at(31)),
        [Command::Set {
            param: "preamp.gain".to_string(),
            value: 13.0
        }]
    );
    assert!(throttle.tick(at(70)).is_empty());
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
    assert_eq!(add_command(&[], &gate), front);
    assert_eq!(add_command(&clean_rack()[2..], &gate), front);
    assert_eq!(
        add_command(&clean_rack(), &gate),
        Command::RackAdd {
            type_name: "gate".to_string(),
            after: Some("amp".to_string()),
            at: None,
        }
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
    assert_eq!(types.len(), 12);
    let channels = |type_name: &str| {
        types
            .iter()
            .find(|module_type| module_type.type_name == type_name)
            .map(|module_type| module_type.channels.as_str())
    };
    assert_eq!(channels("gain"), Some("mono"));
    assert_eq!(channels("fuzz"), Some("mono"));
    assert_eq!(channels("cabinet"), Some("mono_to_stereo"));
    assert_eq!(channels("reverb"), Some("stereo"));
    let cabinet = types.iter().find(|t| t.type_name == "cabinet").unwrap();
    assert_eq!(cabinet.label(), "cabinet (mono_to_stereo)");
    assert!(module_types(&json!({})).is_empty());
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
        tuner_toggle(false, true, false),
        (true, Some(Command::Mute(true)), true)
    );
    assert_eq!(tuner_toggle(false, false, false), (true, None, false));
    assert_eq!(
        tuner_toggle(true, true, true),
        (false, Some(Command::Mute(false)), false)
    );
    assert_eq!(tuner_toggle(true, true, false), (false, None, false));
}

#[test]
fn quit_with_tuner_open_unmutes_first() {
    assert_eq!(
        quit_commands(true, true),
        vec![Command::Mute(false), Command::Quit]
    );
    assert_eq!(quit_commands(true, false), vec![Command::Quit]);
    assert_eq!(quit_commands(false, false), vec![Command::Quit]);
    let (requests, quit) = requests_until_quit(quit_commands(true, true));
    assert_eq!((requests, quit), (vec![Request::Mute { on: false }], true));
}

#[test]
fn mute_command_maps_to_request() {
    assert_eq!(
        requests_until_quit(vec![Command::Mute(true), Command::Mute(false)]),
        (
            vec![Request::Mute { on: true }, Request::Mute { on: false }],
            false
        )
    );
}

#[test]
fn cents_text_is_signed() {
    assert_eq!(cents_text(3), "+3 ct");
    assert_eq!(cents_text(-3), "-3 ct");
    assert_eq!(cents_text(0), "+0 ct");
}

#[test]
fn ir_names_from_payload() {
    let payload = json!({"ok": true, "ir_dir": "/srv/irs", "irs": ["Loud.WAV", "bass-cab.wav", "celestion/v30.wav"]});
    assert_eq!(
        ir_names(&payload),
        vec!["Loud.WAV", "bass-cab.wav", "celestion/v30.wav"]
    );
    assert!(ir_names(&json!({"ok": true, "ir_dir": "/srv/irs", "irs": []})).is_empty());
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
        requests_until_quit(vec![Command::RackIr {
            id: "cabinet".to_string(),
            ir: "cabs/x.wav".to_string(),
        }]),
        (
            vec![Request::RackIr {
                ir: "cabs/x.wav".to_string(),
                id: Some("cabinet".to_string()),
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
fn knob_points_follow_the_sweep() {
    let diagonal = 10.0 * std::f32::consts::FRAC_1_SQRT_2;
    let start = knob_points(0.0, 10.0);
    assert_near(start.pointer, (-diagonal, diagonal));
    assert!(start.arc.iter().all(|point| *point == start.arc[0]));
    let middle = knob_points(0.5, 10.0);
    assert_near(middle.pointer, (0.0, -10.0));
    assert_near(middle.arc[0], (-diagonal, diagonal));
    assert_near(*middle.arc.last().unwrap(), (0.0, -10.0));
    let end = knob_points(1.0, 10.0);
    assert_near(end.pointer, (diagonal, diagonal));
    assert_near(*end.arc.last().unwrap(), (diagonal, diagonal));
    assert!(
        end.arc
            .iter()
            .any(|point| point.0.abs() < 1e-4 && point.1 < 0.0)
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
    assert_eq!(ladder_counts(-12.0, false), (19, 0, 0));
    assert_eq!(ladder_counts(-3.0, false), (19, 3, 0));
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
    assert_eq!(ear_holes(66.0), [(15.0, 22.0), (15.0, 44.0)]);
    assert_eq!(ear_holes(190.0), [(15.0, 22.0), (15.0, 168.0)]);
}

fn relative_luminance((r, g, b): Rgb) -> f64 {
    let channel = |value: u8| {
        let srgb = f64::from(value) / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (light, dark) = {
        let (la, lb) = (relative_luminance(a), relative_luminance(b));
        (la.max(lb), la.min(lb))
    };
    (light + 0.05) / (dark + 0.05)
}

#[test]
fn palette_text_meets_wcag_aa_on_units() {
    let colours = palette();
    for background in [colours.ground, colours.unit, colours.unit_open] {
        assert!(contrast(colours.text, background) >= 4.5);
        assert!(contrast(colours.muted, background) >= 4.5);
    }
    assert_eq!(contrast((255, 255, 255), (0, 0, 0)), 21.0);
}

const UNIT_TOPS: [f32; 4] = [100.0, 170.0, 240.0, 430.0];

#[test]
fn drop_index_above_the_top_unit_is_the_front() {
    assert_eq!(drop_index(2, 40.0, &UNIT_TOPS), 0);
    assert_eq!(drop_index(2, 120.0, &UNIT_TOPS), 0);
}

#[test]
fn drop_index_in_the_middle_is_the_unit_under_the_pointer() {
    assert_eq!(drop_index(0, 300.0, &UNIT_TOPS), 2);
    assert_eq!(drop_index(3, 170.0, &UNIT_TOPS), 1);
}

#[test]
fn drop_index_below_the_bottom_unit_is_the_end() {
    assert_eq!(drop_index(0, 900.0, &UNIT_TOPS), 3);
}

#[test]
fn drop_index_over_its_own_unit_is_the_same_place() {
    assert_eq!(drop_index(1, 200.0, &UNIT_TOPS), 1);
    assert_eq!(drop_index(0, 0.0, &[]), 0);
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
