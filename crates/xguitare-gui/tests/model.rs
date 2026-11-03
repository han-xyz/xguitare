use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use xguitare_client::ClientError;
use xguitare_core::control::Request;
use xguitare_core::control::Response;
use xguitare_gui::model::{
    BankRow, Command, Feedback, KeyEvent, Link, ParamRow, Throttle, bank_rows, counts_toward_spawn,
    key_to_command, meter_fraction, param_rows, peak_fraction, refresh_decision,
    requests_until_quit, response_text, spawn_policy, status_line, unit_suffix,
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
        Command::Quit,
        Command::Load("2".to_string()),
    ];
    let expected = vec![
        Request::Save,
        Request::Set {
            param: "preamp.gain".to_string(),
            value: 70.0,
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
    let rows = bank_rows(&starter_list());
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
    assert!(!bank_rows(&no_active)[0].active);
}

#[test]
fn key_digit_loads_program_only_when_valid() {
    let rows = bank_rows(&starter_list());
    assert_eq!(
        key_to_command(KeyEvent::Digit(3), &rows),
        Some(Command::Load("2".to_string()))
    );
    assert_eq!(
        key_to_command(KeyEvent::Digit(1), &rows),
        Some(Command::Load("0".to_string()))
    );
    assert_eq!(key_to_command(KeyEvent::Digit(4), &rows), None);
    assert_eq!(key_to_command(KeyEvent::Digit(5), &rows), None);
    assert_eq!(key_to_command(KeyEvent::Digit(0), &rows), None);
}

#[test]
fn key_ctrl_s_is_save() {
    assert_eq!(key_to_command(KeyEvent::CtrlS, &[]), Some(Command::Save));
}

fn two_module_get() -> serde_json::Value {
    json!({"params": [
        {"module": "preamp", "param": "gain", "value": 40.0, "unit": "%", "min": 0.0, "max": 100.0, "integer": false},
        {"module": "preamp", "param": "stages", "value": 3.0, "unit": "", "min": 1.0, "max": 4.0, "integer": true},
        {"module": "delay", "param": "time", "value": 350.0, "unit": "ms", "min": 1.0, "max": 2000.0, "integer": false},
        {"module": "delay", "param": "pingpong", "value": 0.0, "unit": "", "min": 0.0, "max": 1.0, "integer": true},
    ]})
}

#[test]
fn param_rows_in_rack_order_with_units() {
    let row =
        |module: &str, param: &str, value: f64, min: f64, max: f64, unit: &str, integer: bool| {
            ParamRow {
                address: format!("{module}.{param}"),
                module: module.to_string(),
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
        {"module": "preamp", "param": "bias", "value": 0.0, "unit": "", "min": -1.0, "max": 1.0, "integer": false},
        {"module": "preamp", "param": "stages", "value": 3.0, "unit": "", "min": 1.0, "max": 4.0, "integer": true},
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
fn save_response_text() {
    assert_eq!(
        response_text(
            &Request::Save,
            Ok(Response::Ok(json!({"path": "/bank/01-clean.toml"})))
        ),
        "saved /bank/01-clean.toml"
    );
    assert_eq!(
        response_text(
            &Request::Save,
            Ok(Response::Err {
                error: "no preset loaded".to_string()
            })
        ),
        "error: no preset loaded"
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
