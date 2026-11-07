use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use xguitare_client::ClientError;
use xguitare_core::control::Request;
use xguitare_core::control::Response;
use xguitare_gui::model::{
    BankRow, Command, Direction, Feedback, KeyEvent, KeyModifiers, Link, ModuleType, ParamRow,
    Throttle, add_after, bank_rows, counts_toward_spawn, failed_set, key_to_command,
    meter_fraction, module_types, move_target, param_rows, peak_fraction, rack_channels,
    refresh_decision, requests_until_quit, response_text, spawn_policy, status_line, unit_suffix,
};

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
        Request::Save,
        Request::Set {
            param: "preamp.gain".to_string(),
            value: 70.0,
        },
        Request::RackAdd {
            r#type: "delay".to_string(),
            after: Some("reverb".to_string()),
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
        {"module": "preamp", "type": "preamp", "param": "gain", "value": 40.0, "unit": "%", "min": 0.0, "max": 100.0, "integer": false},
        {"module": "preamp", "type": "preamp", "param": "stages", "value": 3.0, "unit": "", "min": 1.0, "max": 4.0, "integer": true},
        {"module": "delay", "type": "delay", "param": "time", "value": 350.0, "unit": "ms", "min": 1.0, "max": 2000.0, "integer": false},
        {"module": "delay", "type": "delay", "param": "pingpong", "value": 0.0, "unit": "", "min": 0.0, "max": 1.0, "integer": true},
    ]})
}

#[test]
fn param_rows_in_rack_order_with_units() {
    let row =
        |module: &str, param: &str, value: f64, min: f64, max: f64, unit: &str, integer: bool| {
            ParamRow {
                address: format!("{module}.{param}"),
                module: module.to_string(),
                module_type: module.to_string(),
                param: param.to_string(),
                value,
                min,
                max,
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
                    row("preamp", "gain", 40.0, 0.0, 100.0, "%", false),
                    row("preamp", "stages", 3.0, 1.0, 4.0, "", true),
                ]
            ),
            (
                "delay".to_string(),
                vec![
                    row("delay", "time", 350.0, 1.0, 2000.0, "ms", false),
                    row("delay", "pingpong", 0.0, 0.0, 1.0, "", true),
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
        response_text(&Request::Save, saved).as_deref(),
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
    assert!(response_text(&Request::Save, gone).is_some_and(|text| text.starts_with("error: ")));
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
    assert_eq!(failed_set(&Request::Save, &busy), None);
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
    let payload = json!({"modules": [
        {"type": "gain", "channels": "mono", "params": []},
        {"type": "gate", "channels": "mono", "params": []},
        {"type": "compressor", "channels": "mono", "params": []},
        {"type": "drive", "channels": "mono", "params": []},
        {"type": "preamp", "channels": "mono", "params": []},
        {"type": "tonestack", "channels": "mono", "params": []},
        {"type": "cabinet", "channels": "mono_to_stereo", "params": []},
        {"type": "delay", "channels": "stereo", "params": []},
        {"type": "chorus", "channels": "stereo", "params": []},
        {"type": "flanger", "channels": "stereo", "params": []},
        {"type": "reverb", "channels": "stereo", "params": []},
    ]});
    let types = module_types(&payload);
    assert_eq!(types.len(), 11);
    let module_type = |type_name: &str, channels: &str| ModuleType {
        type_name: type_name.to_string(),
        channels: channels.to_string(),
    };
    assert_eq!(types[0], module_type("gain", "mono"));
    assert_eq!(types[6], module_type("cabinet", "mono_to_stereo"));
    assert_eq!(types[10], module_type("reverb", "stereo"));
    assert_eq!(types[6].label(), "cabinet (mono_to_stereo)");
    assert!(module_types(&json!({})).is_empty());
}
