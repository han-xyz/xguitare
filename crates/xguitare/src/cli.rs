use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use xguitare_client::Transport;
use xguitare_core::control::{Request, Response, Value};

use crate::{Cmd, MuteState, RackCmd, parse};

const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const WATCH_LINE_COLUMNS: usize = 24;

/// What a client command needs from its surroundings besides the transport.
pub struct CliEnv {
    /// The config's `tuner_mute`: whether `tuner --watch` turns Mute on while it runs.
    pub tuner_mute: bool,
    /// Whether stdout is a terminal: `tuner --watch` then rewrites one line instead of printing one per reading.
    pub is_tty: bool,
    /// Set to end `tuner --watch`.
    pub stop: Arc<AtomicBool>,
}

/// Runs one client subcommand against the Daemon; returns the exit code (0 on success and `--help`, 1 after one `error: ...` line on `err`).
pub fn run_cli(
    argv: &[String],
    transport: &mut dyn Transport,
    out: &mut dyn Write,
    err: &mut dyn Write,
    env: &CliEnv,
) -> i32 {
    let cli = match parse(argv, out, err) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let result = match cli.cmd {
        Cmd::Tuner { watch: true } => watch_tuner(transport, out, env).map(|()| String::new()),
        cmd => request_for(cmd).and_then(|req| Ok(render(&req, &request(transport, &req)?))),
    };
    let written = match result {
        Ok(text) => write!(out, "{text}").map(|()| 0),
        Err(message) => writeln!(err, "error: {}", without_control_chars(&message)).map(|()| 1),
    };
    written.unwrap_or(1)
}

fn request(transport: &mut dyn Transport, req: &Request) -> Result<Value, String> {
    match transport.request(req).map_err(|e| e.to_string())? {
        Response::Ok(payload) => Ok(payload),
        Response::Err { error } => Err(error),
    }
}

fn watch_tuner(
    transport: &mut dyn Transport,
    out: &mut dyn Write,
    env: &CliEnv,
) -> Result<(), String> {
    if env.tuner_mute {
        request(transport, &Request::Mute { on: true })?;
    }
    let watched = print_tuner_readings(transport, out, env);
    let unmuted = if env.tuner_mute {
        request(transport, &Request::Mute { on: false }).map(drop)
    } else {
        Ok(())
    };
    watched.and(unmuted)
}

fn print_tuner_readings(
    transport: &mut dyn Transport,
    out: &mut dyn Write,
    env: &CliEnv,
) -> Result<(), String> {
    while !env.stop.load(Ordering::Relaxed) {
        let line = tuner_line(&request(transport, &Request::Tuner)?);
        if env.is_tty {
            write!(out, "\r{line:<WATCH_LINE_COLUMNS$}")
        } else {
            writeln!(out, "{line}")
        }
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())?;
        thread::sleep(WATCH_INTERVAL);
    }
    if env.is_tty {
        writeln!(out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn tuner_line(payload: &Value) -> String {
    if payload["confident"] != true {
        return "no signal".to_string();
    }
    format!(
        "{:<3} {:.1} Hz {:+} ct",
        text(&payload["note"]),
        payload["hz"].as_f64().unwrap_or_default(),
        payload["cents"].as_i64().unwrap_or_default()
    )
}

fn request_for(cmd: Cmd) -> Result<Request, String> {
    Ok(match cmd {
        Cmd::Run => return Err("'run' starts the daemon; it is not a client command".to_string()),
        Cmd::Status => Request::Status,
        Cmd::List => Request::List,
        Cmd::Load { preset } => Request::Load { preset },
        Cmd::Set { param, value } => {
            let value: f64 = value
                .parse()
                .map_err(|_| "value must be a number".to_string())?;
            if !value.is_finite() {
                return Err("value must be a finite number".to_string());
            }
            Request::Set { param, value }
        }
        Cmd::Get { param } => Request::Get { param },
        Cmd::Save => Request::Save,
        Cmd::Modules => Request::Modules,
        Cmd::Rack(RackCmd::Add {
            type_name,
            after,
            at,
        }) => Request::RackAdd {
            r#type: type_name,
            after,
            at,
        },
        Cmd::Rack(RackCmd::Remove { id }) => Request::RackRemove { id },
        Cmd::Rack(RackCmd::Move { id, to }) => Request::RackMove { id, to },
        Cmd::Tuner { .. } => Request::Tuner,
        Cmd::Mute { state } => Request::Mute {
            on: matches!(state, MuteState::On),
        },
    })
}

fn render(req: &Request, payload: &Value) -> String {
    match req {
        Request::Status => render_status(payload),
        Request::List => render_list(payload),
        Request::Load { .. } => format!(
            "loaded {} {}\n",
            text(&payload["program"]),
            text(&payload["name"])
        ),
        Request::Set { .. } => {
            let clamped = if payload["clamped"] == true {
                " (clamped)"
            } else {
                ""
            };
            format!(
                "{} = {}{clamped}\n",
                text(&payload["param"]),
                text(&payload["value"])
            )
        }
        Request::Get { .. } => items(&payload["params"])
            .map(|param| {
                let line = format!(
                    "{}.{} = {} {}",
                    text(&param["module"]),
                    text(&param["param"]),
                    text(&param["value"]),
                    text(&param["unit"])
                );
                format!("{}\n", line.trim_end())
            })
            .collect(),
        Request::Save => format!("saved {}\n", text(&payload["path"])),
        Request::Modules => render_modules(payload),
        Request::RackAdd { .. } => format!("added {}\n", text(&payload["id"])),
        Request::RackRemove { .. } => format!("removed {}\n", text(&payload["id"])),
        Request::RackMove { .. } => format!(
            "moved {} to {}\n",
            text(&payload["id"]),
            text(&payload["to"])
        ),
        Request::Tuner => format!("{}\n", tuner_line(payload)),
        Request::Mute { .. } => {
            if payload["muted"] == true {
                "muted\n".to_string()
            } else {
                "unmuted\n".to_string()
            }
        }
    }
}

fn render_status(payload: &Value) -> String {
    let mut lines: String = [
        "preset",
        "program",
        "sample_rate",
        "buffer_size",
        "prepared_frames",
        "revision",
        "muted",
        "input_peak_db",
        "output_peak_db",
        "xruns",
        "callback_max_us",
        "callback_last_us",
    ]
    .iter()
    .map(|key| format!("{key}: {}\n", status_text(key, &payload[key])))
    .collect();
    lines.push_str("connections:\n");
    for connection in items(&payload["connections"]) {
        lines.push_str(&format!(
            "  {} -> {}\n",
            text(&connection["from"]),
            text(&connection["to"])
        ));
    }
    lines
}

fn status_text(key: &str, value: &Value) -> String {
    match value.as_f64() {
        Some(db) if key.ends_with("_db") => format!("{db:.1}"),
        _ => text(value),
    }
}

fn render_list(payload: &Value) -> String {
    items(&payload["entries"])
        .map(|entry| {
            let program = text(&entry["program"]);
            let name = text(&entry["name"]);
            if entry["valid"] == false {
                format!("{program:>2}! {name} ({})\n", text(&entry["error"]))
            } else if entry["program"] == payload["active"] {
                format!("{program:>2}* {name}\n")
            } else {
                format!("{program:>2}  {name}\n")
            }
        })
        .collect()
}

fn render_modules(payload: &Value) -> String {
    items(&payload["modules"])
        .map(|module| {
            let params: Vec<String> = items(&module["params"])
                .map(|param| text(&param["name"]))
                .collect();
            format!(
                "{:<12}{:<16}{}\n",
                text(&module["type"]),
                text(&module["channels"]),
                params.join(",")
            )
        })
        .collect()
}

fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(text) => without_control_chars(text),
        Value::Number(number) if number.is_f64() => number
            .as_f64()
            .map_or_else(|| number.to_string(), |float| float.to_string()),
        other => other.to_string(),
    }
}

/// Replaces every control character with U+FFFD, so text from the Daemon cannot send terminal escape sequences.
fn without_control_chars(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                char::REPLACEMENT_CHARACTER
            } else {
                c
            }
        })
        .collect()
}
