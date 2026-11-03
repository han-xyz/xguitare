use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use xguitare_client::ClientError;
use xguitare_gui::model::{
    Link, counts_toward_spawn, meter_fraction, peak_fraction, refresh_decision, spawn_policy,
    status_line,
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
