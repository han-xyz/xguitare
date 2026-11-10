use std::fmt::Write as _;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use xguitare_client::Transport;
use xguitare_core::control::{
    Position, Request, Response, SILENCE_DB, TrackRef, TransportAction, Value,
};

use crate::{BypassState, Cmd, IrCmd, MuteState, ProjectCmd, RackCmd, Switch, TrackCmd, parse};

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
        cmd => request_for(cmd).and_then(|req| render(&req, &request(transport, &req)?)),
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
        Cmd::Load { preset } => Request::Load {
            preset,
            track: None,
        },
        Cmd::Set { param, value } => {
            let value: f64 = value
                .parse()
                .map_err(|_| "value must be a number".to_string())?;
            if !value.is_finite() {
                return Err("value must be a finite number".to_string());
            }
            Request::Set {
                param,
                value,
                track: None,
            }
        }
        Cmd::Get { param } => Request::Get { param, track: None },
        Cmd::Save { name } => Request::Save { name },
        Cmd::Modules => Request::Modules,
        Cmd::Rack(RackCmd::Add {
            type_name,
            after,
            at,
        }) => Request::RackAdd {
            r#type: type_name,
            after,
            at,
            track: None,
        },
        Cmd::Rack(RackCmd::Remove { id }) => Request::RackRemove { id, track: None },
        Cmd::Rack(RackCmd::Move { id, to }) => Request::RackMove {
            id,
            to,
            track: None,
        },
        Cmd::Ir(IrCmd::List) => Request::IrList,
        Cmd::Ir(IrCmd::Set { file, id }) => Request::RackIr {
            ir: file,
            id,
            track: None,
        },
        Cmd::Tuner { .. } => Request::Tuner,
        Cmd::Mute { state } => Request::Mute {
            on: matches!(state, MuteState::On),
        },
        Cmd::Bypass { id, state } => Request::Bypass {
            id,
            on: matches!(state, BypassState::On),
            track: None,
        },
        Cmd::Project(cmd) => project_request(cmd),
        Cmd::Track(cmd) => track_request(cmd),
        Cmd::Play => transport(TransportAction::Play, None),
        Cmd::Stop => transport(TransportAction::Stop, None),
        Cmd::Record => transport(TransportAction::Record, None),
        Cmd::Locate { position } => {
            transport(TransportAction::Locate, Some(Position::Text(position)))
        }
        Cmd::Tempo { bpm, meter } => Request::Tempo {
            bpm: Some(bpm),
            meter: meter.as_deref().map(parse_meter).transpose()?,
        },
        Cmd::Click { state, count_in } => Request::Click {
            on: Some(is_on(state)),
            count_in,
        },
    })
}

fn project_request(cmd: ProjectCmd) -> Request {
    match cmd {
        ProjectCmd::New { name } => Request::ProjectNew { name },
        ProjectCmd::Open { name, restore } => Request::ProjectOpen { name, restore },
        ProjectCmd::Save => Request::ProjectSave,
        ProjectCmd::Close => Request::ProjectClose,
        ProjectCmd::List => Request::ProjectList,
        ProjectCmd::Items => Request::Items,
    }
}

fn track_request(cmd: TrackCmd) -> Request {
    match cmd {
        TrackCmd::Add { name } => Request::TrackAdd { name },
        TrackCmd::Remove { track } => Request::TrackRemove {
            track: track_ref(track),
        },
        TrackCmd::Edit {
            track,
            input,
            volume,
            pan,
            mute,
            solo,
            arm,
            name,
        } => Request::TrackSet {
            track: track_ref(track),
            input,
            volume,
            pan,
            mute: mute.map(is_on),
            solo: solo.map(is_on),
            arm: arm.map(is_on),
            name,
        },
        TrackCmd::Select { track } => Request::TrackSelect {
            track: track_ref(track),
        },
    }
}

fn transport(action: TransportAction, position: Option<Position>) -> Request {
    Request::Transport { action, position }
}

fn is_on(state: Switch) -> bool {
    matches!(state, Switch::On)
}

/// Digits name a Track by index, anything else by name.
fn track_ref(track: String) -> TrackRef {
    match track.parse() {
        Ok(index) => TrackRef::Index(index),
        Err(_) => TrackRef::Name(track),
    }
}

fn parse_meter(meter: &str) -> Result<(u32, u32), String> {
    meter
        .split_once('/')
        .and_then(|(beats, note)| Some((beats.parse().ok()?, note.parse().ok()?)))
        .ok_or_else(|| "meter must be <beats>/<note>".to_string())
}

fn render(req: &Request, payload: &Value) -> Result<String, String> {
    let rendered = match req {
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
        Request::Get { param, .. } => render_get(payload, param.as_deref()),
        Request::Save { .. } | Request::ProjectSave => {
            format!("saved {}\n", text(&payload["path"]))
        }
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
        Request::Bypass { id, .. } => {
            let id = without_control_chars(id);
            if payload["bypass"] == true {
                format!("{id}: bypassed\n")
            } else {
                format!("{id}: on\n")
            }
        }
        Request::IrList => items(&payload["irs"])
            .map(|name| text(name) + "\n")
            .collect(),
        Request::RackIr { .. } => {
            format!("ir {} {}\n", text(&payload["id"]), text(&payload["ir"]))
        }
        Request::ProjectNew { .. } => format!(
            "created {} at {}\n",
            text(&payload["name"]),
            text(&payload["path"])
        ),
        Request::ProjectOpen { .. } => render_open(payload),
        Request::ProjectClose => format!("closed {}\n", text(&payload["name"])),
        Request::ProjectList => render_projects(payload),
        Request::Items => render_items(payload)?,
        Request::TrackAdd { .. } => format!("added track {}\n", track_label(payload)),
        Request::TrackRemove { .. } => format!("removed track {}\n", track_label(payload)),
        Request::TrackSet { .. } => render_track(payload),
        Request::TrackSelect { .. } => format!("selected track {}\n", track_label(payload)),
        Request::Transport { action, .. } => match action {
            TransportAction::Play => "playing\n".to_string(),
            TransportAction::Stop => "stopped\n".to_string(),
            TransportAction::Record => "recording\n".to_string(),
            TransportAction::Locate => format!("located at frame {}\n", text(&payload["frames"])),
        },
        Request::Tempo { .. } => format!(
            "tempo {} bpm {}\n",
            text(&payload["bpm"]),
            meter_text(&payload["meter"])
        ),
        Request::Click { .. } => format!("{}\n", click_text(payload)),
        Request::Ports
        | Request::SetPorts { .. }
        | Request::Settings
        | Request::SetSettings { .. }
        | Request::Loop { .. } => format!("{payload}\n"),
    };
    Ok(rendered)
}

fn render_open(payload: &Value) -> String {
    let name = text(&payload["name"]);
    if payload["restored"] == true {
        return format!("opened {name} from its autosave\n");
    }
    let mut lines = format!("opened {name}\n");
    if payload["autosave_newer"] == true {
        lines.push_str("its autosave is newer; open it with --restore to load the autosave\n");
    }
    lines
}

fn render_projects(payload: &Value) -> String {
    items(&payload["projects"])
        .map(|project| {
            if project["valid"] == false {
                format!(
                    "{}! ({})\n",
                    text(&project["folder"]),
                    text(&project["error"])
                )
            } else {
                format!("{}\n", text(&project["name"]))
            }
        })
        .collect()
}

fn render_items(payload: &Value) -> Result<String, String> {
    let sample_rate = payload["sample_rate"]
        .as_f64()
        .ok_or("the items answer has no sample_rate")?;
    let mut lines = String::new();
    for track in items(&payload["tracks"]) {
        for item in items(&track["items"]) {
            let seconds = item["length"].as_f64().unwrap_or(0.0) / sample_rate;
            let take = item["take"].as_u64().unwrap_or(0) + 1;
            let _ = writeln!(
                lines,
                "{} {} {seconds:.2}s take {take}/{}",
                text(&track["name"]),
                text(&item["bars_beats"]),
                items(&item["takes"]).count()
            );
        }
    }
    Ok(lines)
}

fn track_label(payload: &Value) -> String {
    format!("{} {}", text(&payload["index"]), text(&payload["name"]))
}

fn render_track(payload: &Value) -> String {
    let yes_no = |key: &str| if payload[key] == true { "yes" } else { "no" };
    format!(
        "track {}: input {}, volume {} dB, pan {}, mute {}, solo {}, arm {}\n",
        track_label(payload),
        text(&payload["input"]),
        text(&payload["volume_db"]),
        text(&payload["pan"]),
        yes_no("mute"),
        yes_no("solo"),
        yes_no("arm"),
    )
}

fn meter_text(meter: &Value) -> String {
    format!("{}/{}", text(&meter[0]), text(&meter[1]))
}

fn click_text(click: &Value) -> String {
    let on = if click["on"] == true { "on" } else { "off" };
    format!("click {on}, count-in {} bars", text(&click["count_in"]))
}

/// After a cabinet Module's last Parameter line comes `<id>.ir = <name>`, or `none` when it has no Cabinet IR; then `<id>.bypass = yes|no`; a `key` naming one Parameter gets neither line.
fn render_get(payload: &Value, key: Option<&str>) -> String {
    let names_one_param = key.is_some_and(|key| key.contains('.'));
    let params: Vec<&Value> = items(&payload["params"]).collect();
    let mut lines = String::new();
    for (index, param) in params.iter().enumerate() {
        let module = text(&param["module"]);
        let line = format!(
            "{module}.{} = {} {}",
            text(&param["param"]),
            text(&param["value"]),
            text(&param["unit"])
        );
        let _ = writeln!(lines, "{}", line.trim_end());
        let is_last_of_module = params
            .get(index + 1)
            .is_none_or(|next| next["module"] != param["module"]);
        if let Some(ir) = param
            .get("ir")
            .filter(|_| is_last_of_module && !names_one_param)
        {
            let name = if ir.is_null() {
                "none".to_string()
            } else {
                text(ir)
            };
            let _ = writeln!(lines, "{module}.ir = {name}");
        }
        if let Some(bypass) = param
            .get("bypass")
            .filter(|_| is_last_of_module && !names_one_param)
        {
            let answer = if bypass == true { "yes" } else { "no" };
            let _ = writeln!(lines, "{module}.bypass = {answer}");
        }
    }
    lines
}

fn render_status(payload: &Value) -> String {
    let mut lines = String::new();
    for key in [
        "preset",
        "program",
        "modified",
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
    ] {
        let _ = writeln!(lines, "{key}: {}", status_text(key, &payload[key]));
    }
    lines.push_str("connections:\n");
    for connection in items(&payload["connections"]) {
        let _ = writeln!(
            lines,
            "  {} -> {}",
            text(&connection["from"]),
            text(&connection["to"])
        );
    }
    let _ = writeln!(
        lines,
        "disk_overflows: {}",
        text(&payload["disk_overflows"])
    );
    let _ = writeln!(lines, "project: {}", text(&payload["project"]));
    if !payload["project"].is_string() {
        return lines;
    }
    render_project_status(&mut lines, payload);
    lines
}

fn render_project_status(lines: &mut String, payload: &Value) {
    let position = &payload["position"];
    let tempo = &payload["tempo"];
    let looped = &payload["loop"];
    let _ = writeln!(lines, "transport: {}", text(&payload["transport"]));
    let _ = writeln!(
        lines,
        "position: {} (frame {})",
        text(&position["bars_beats"]),
        text(&position["frames"])
    );
    let _ = writeln!(
        lines,
        "tempo: {} bpm {}",
        text(&tempo["bpm"]),
        meter_text(&tempo["meter"])
    );
    let _ = writeln!(
        lines,
        "loop: {} {}..{}",
        if looped["on"] == true { "on" } else { "off" },
        text(&looped["start"]),
        text(&looped["end"])
    );
    let _ = writeln!(lines, "{}", click_text(&payload["click"]));
    lines.push_str("tracks:\n");
    for (index, track) in items(&payload["tracks"]).enumerate() {
        let selected = if payload["selected"] == index {
            "*"
        } else {
            " "
        };
        let flags: Vec<&str> = [("armed", "armed"), ("mute", "muted"), ("solo", "solo")]
            .into_iter()
            .filter(|(key, _)| track[*key] == true)
            .map(|(_, flag)| flag)
            .collect();
        let flags = if flags.is_empty() {
            "-".to_string()
        } else {
            flags.join(", ")
        };
        let peak = track["input_peak_db"].as_f64().unwrap_or(SILENCE_DB);
        let _ = writeln!(
            lines,
            "  {index}{selected} {}: {flags}; input {}; {} dB, pan {}; peak {peak:.1} dB",
            text(&track["name"]),
            text(&track["input"]),
            text(&track["volume_db"]),
            text(&track["pan"]),
        );
    }
}

fn status_text(key: &str, value: &Value) -> String {
    match (key, value.as_bool(), value.as_f64()) {
        ("modified", Some(modified), _) => if modified { "yes" } else { "no" }.to_string(),
        (_, _, Some(db)) if key.ends_with("_db") => format!("{db:.1}"),
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
    let mut lines = String::new();
    for module in items(&payload["modules"]) {
        let params: Vec<String> = items(&module["params"])
            .map(|param| text(&param["name"]))
            .collect();
        let _ = writeln!(
            lines,
            "{:<12}{:<16}{}",
            text(&module["type"]),
            text(&module["channels"]),
            params.join(",")
        );
    }
    lines
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
