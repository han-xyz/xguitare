use std::time::{Duration, Instant};

use serde_json::Value;
use xguitare_client::ClientError;

pub const SPAWN_AFTER_FAILURES: u32 = 3;
pub const SPAWN_BACKOFF: Duration = Duration::from_secs(5);
pub const SILENCE_DB: f64 = -120.0;
pub const METER_FLOOR_DB: f64 = -60.0;
pub const METER_RED_DB: f64 = -6.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Connected,
    Disconnected { since: Instant },
    Starting { since: Instant },
}

/// What the client thread last saw; the payloads are the Control API's `status`, `list` and `get` replies as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub link: Link,
    pub status: Option<Value>,
    pub bank: Option<Value>,
    pub params: Option<Value>,
    pub last_message: Option<(String, Instant)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Load(String),
    Set { param: String, value: f64 },
    Save,
    Quit,
}

pub fn spawn_policy(failures: u32, last_spawn: Option<Instant>, now: Instant) -> bool {
    failures >= SPAWN_AFTER_FAILURES
        && last_spawn.is_none_or(|spawned| now.duration_since(spawned) >= SPAWN_BACKOFF)
}

/// Only a missing socket counts: a Daemon that does not answer may still be alive, and an insecure socket directory fails a spawned Daemon too.
pub fn counts_toward_spawn(err: &ClientError) -> bool {
    matches!(err, ClientError::NotRunning(_))
}

pub fn refresh_decision(last_seen: Option<u64>, revision: u64) -> bool {
    last_seen != Some(revision)
}

pub fn meter_fraction(db: f64) -> f32 {
    ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0) as f32
}

/// A peak the payload lacks or carries as `null` (a non-finite value serialises as `null`) reads as −120 dB.
pub fn peak_fraction(status: &Value, key: &str) -> f32 {
    meter_fraction(status[key].as_f64().unwrap_or(SILENCE_DB))
}

pub fn status_line(status: &Value) -> String {
    format!(
        "{} Hz · {}/{} frames · xruns {} · cb {}/{} µs",
        status["sample_rate"],
        status["buffer_size"],
        status["prepared_frames"],
        status["xruns"],
        status["callback_last_us"],
        status["callback_max_us"],
    )
}

pub fn link_text(link: &Link, now: Instant) -> String {
    match link {
        Link::Connected => "connected".to_string(),
        Link::Disconnected { since } => format!("disconnected {}s", seconds_since(*since, now)),
        Link::Starting { since } => format!("starting daemon {}s", seconds_since(*since, now)),
    }
}

fn seconds_since(since: Instant, now: Instant) -> u64 {
    now.saturating_duration_since(since).as_secs()
}
