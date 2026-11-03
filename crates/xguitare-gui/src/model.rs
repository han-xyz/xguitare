use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;
use xguitare_client::ClientError;
use xguitare_core::control::{Request, Response};

pub const SPAWN_AFTER_FAILURES: u32 = 3;
pub const SPAWN_BACKOFF: Duration = Duration::from_secs(5);
pub const SILENCE_DB: f64 = -120.0;
pub const METER_FLOOR_DB: f64 = -60.0;
pub const METER_RED_DB: f64 = -6.0;
pub const SET_INTERVAL: Duration = Duration::from_millis(30);
pub const FEEDBACK_FOR: Duration = Duration::from_secs(3);
pub const INVALID_PRESET: &str = "invalid preset";

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
    pub failed_set: Option<(String, Instant)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Load(String),
    Set { param: String, value: f64 },
    Save,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BankRow {
    pub program: usize,
    pub name: String,
    pub active: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyEvent {
    Digit(u8, KeyModifiers),
    CtrlS,
}

/// The Bank rows of a `list` payload, and how many entries were skipped for lacking an integer `program`.
pub fn bank_rows(list_payload: &Value) -> (Vec<BankRow>, usize) {
    let active = list_payload["active"].as_u64();
    let entries = list_payload["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let rows: Vec<BankRow> = entries
        .iter()
        .filter_map(|entry| {
            let program = entry["program"].as_u64()?;
            let valid = entry["valid"].as_bool().unwrap_or_default();
            Some(BankRow {
                program: program as usize,
                name: entry["name"].as_str().unwrap_or_default().to_string(),
                active: active == Some(program),
                error: (!valid).then(|| {
                    entry["error"]
                        .as_str()
                        .unwrap_or(INVALID_PRESET)
                        .to_string()
                }),
            })
        })
        .collect();
    let skipped = entries.len() - rows.len();
    (rows, skipped)
}

pub fn key_to_command(key: KeyEvent, rows: &[BankRow]) -> Option<Command> {
    match key {
        KeyEvent::CtrlS => Some(Command::Save),
        KeyEvent::Digit(_, KeyModifiers { ctrl: true, .. } | KeyModifiers { alt: true, .. }) => {
            None
        }
        KeyEvent::Digit(digit @ 1..=9, _) => {
            let program = usize::from(digit - 1);
            rows.iter()
                .find(|row| row.program == program && row.error.is_none())
                .map(|row| Command::Load(row.program.to_string()))
        }
        KeyEvent::Digit(..) => None,
    }
}

pub fn spawn_policy(failures: u32, last_spawn: Option<Instant>, now: Instant) -> bool {
    failures >= SPAWN_AFTER_FAILURES
        && last_spawn.is_none_or(|spawned| now.duration_since(spawned) >= SPAWN_BACKOFF)
}

/// Only a missing socket counts: a Daemon that does not answer may still be alive, and an insecure socket directory fails a spawned Daemon too.
pub fn counts_toward_spawn(err: &ClientError) -> bool {
    matches!(err, ClientError::NotRunning(_))
}

pub fn requests_until_quit(queued: Vec<Command>) -> (Vec<Request>, bool) {
    let mut requests = Vec::new();
    for command in queued {
        let req = match command {
            Command::Load(preset) => Request::Load { preset },
            Command::Set { param, value } => Request::Set { param, value },
            Command::Save => Request::Save,
            Command::Quit => return (requests, true),
        };
        requests.push(req);
    }
    (requests, false)
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

#[derive(Debug, Clone, PartialEq)]
pub struct ParamRow {
    pub address: String,
    pub module: String,
    pub param: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub unit: String,
    pub integer: bool,
}

/// Groups the `get` payload's `params` per Module, keeping Rack order.
pub fn param_rows(get_payload: &Value) -> Vec<(String, Vec<ParamRow>)> {
    let mut modules: Vec<(String, Vec<ParamRow>)> = Vec::new();
    for param in get_payload["params"].as_array().into_iter().flatten() {
        let module = param["module"].as_str().unwrap_or_default().to_string();
        let name = param["param"].as_str().unwrap_or_default().to_string();
        let row = ParamRow {
            address: format!("{module}.{name}"),
            module: module.clone(),
            param: name,
            value: param["value"].as_f64().unwrap_or_default(),
            min: param["min"].as_f64().unwrap_or_default(),
            max: param["max"].as_f64().unwrap_or_default(),
            unit: param["unit"].as_str().unwrap_or_default().to_string(),
            integer: param["integer"].as_bool().unwrap_or_default(),
        };
        match modules.last_mut() {
            Some((id, rows)) if *id == module => rows.push(row),
            _ => modules.push((module, vec![row])),
        }
    }
    modules
}

pub fn unit_suffix(unit: &str) -> &'static str {
    match unit {
        "dB" => " dB",
        "ms" => " ms",
        "Hz" => " Hz",
        "%" => " %",
        _ => "",
    }
}

#[derive(Debug, Default)]
struct Drag {
    last_sent: Option<Instant>,
    pending: Option<f64>,
    released_at: Option<u64>,
}

impl Drag {
    fn is_due(&self, now: Instant) -> bool {
        self.last_sent
            .is_none_or(|sent| now.duration_since(sent) >= SET_INTERVAL)
    }
}

/// Per Parameter address: while dragged, a `set` at most every [`SET_INTERVAL`], a throttled value sent by [`Throttle::tick`] once the interval has passed; the slider keeps its local value until a snapshot newer than the release Revision arrives.
#[derive(Debug, Default)]
pub struct Throttle {
    drags: HashMap<String, Drag>,
}

impl Throttle {
    pub fn on_drag(&mut self, address: &str, value: f64, now: Instant) -> Option<Command> {
        let drag = self.drags.entry(address.to_string()).or_default();
        drag.released_at = None;
        if !drag.is_due(now) {
            drag.pending = Some(value);
            return None;
        }
        drag.last_sent = Some(now);
        drag.pending = None;
        Some(set_command(address, value))
    }

    pub fn tick(&mut self, now: Instant) -> Vec<Command> {
        let mut due = Vec::new();
        for (address, drag) in &mut self.drags {
            if !drag.is_due(now) {
                continue;
            }
            if let Some(value) = drag.pending.take() {
                drag.last_sent = Some(now);
                due.push(set_command(address, value));
            }
        }
        due
    }

    pub fn has_pending(&self) -> bool {
        self.drags.values().any(|drag| drag.pending.is_some())
    }

    pub fn on_release(&mut self, address: &str, value: f64, revision: u64) -> Command {
        let drag = self.drags.entry(address.to_string()).or_default();
        drag.last_sent = None;
        drag.pending = None;
        drag.released_at = Some(revision);
        set_command(address, value)
    }

    pub fn hold_local(&self, address: &str, revision_now: u64) -> bool {
        self.drags.get(address).is_some_and(|drag| {
            drag.released_at
                .is_none_or(|released| revision_now <= released)
        })
    }

    pub fn forget(&mut self, address: &str) {
        self.drags.remove(address);
    }

    pub fn retain(&mut self, is_present: impl Fn(&str) -> bool) {
        self.drags.retain(|address, _| is_present(address));
    }
}

fn set_command(address: &str, value: f64) -> Command {
    Command::Set {
        param: address.to_string(),
        value,
    }
}

#[derive(Debug, Default)]
pub struct Feedback {
    message: Option<(String, Instant)>,
}

impl Feedback {
    pub fn show(&mut self, text: String, at: Instant) {
        self.message = Some((text, at));
    }

    pub fn text(&self, now: Instant) -> Option<&str> {
        self.message
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) < FEEDBACK_FOR)
            .map(|(text, _)| text.as_str())
    }
}

/// The status bar message for a command's response: a saved path or an error; other successes are silent.
pub fn response_text(req: &Request, response: Result<Response, ClientError>) -> Option<String> {
    match (req, response) {
        (Request::Save, Ok(Response::Ok(payload))) => Some(format!(
            "saved {}",
            payload["path"].as_str().unwrap_or_default()
        )),
        (_, Ok(Response::Ok(_))) => None,
        (_, Ok(Response::Err { error })) => Some(format!("error: {error}")),
        (_, Err(err)) => Some(format!("error: {err}")),
    }
}

/// The address of a `set` that the Daemon refused or that never reached it.
pub fn failed_set(req: &Request, response: &Result<Response, ClientError>) -> Option<String> {
    match (req, response) {
        (_, Ok(Response::Ok(_))) => None,
        (Request::Set { param, .. }, _) => Some(param.clone()),
        _ => None,
    }
}
