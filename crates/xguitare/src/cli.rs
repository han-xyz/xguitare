use std::fmt;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use xguitare_core::control::{Request, Response, Value};

use crate::{Cmd, parse};

pub trait Transport {
    fn request(&mut self, req: &Request) -> Result<Response, TransportError>;
}

#[derive(Debug)]
pub enum TransportError {
    NotRunning(PathBuf),
    Io(io::Error),
    Protocol(String),
}

/// Connects to the Daemon's socket once per request: one request line out, one response line back.
pub struct UnixSocketTransport {
    path: PathBuf,
}

impl UnixSocketTransport {
    pub fn new(path: PathBuf) -> Self {
        UnixSocketTransport { path }
    }
}

impl Transport for UnixSocketTransport {
    fn request(&mut self, req: &Request) -> Result<Response, TransportError> {
        let stream = UnixStream::connect(&self.path).map_err(|err| match err.kind() {
            ErrorKind::NotFound | ErrorKind::ConnectionRefused => {
                TransportError::NotRunning(self.path.clone())
            }
            _ => TransportError::Io(err),
        })?;
        (&stream)
            .write_all(format!("{}\n", req.to_line()).as_bytes())
            .map_err(TransportError::Io)?;
        let mut reply = String::new();
        BufReader::new(&stream)
            .read_line(&mut reply)
            .map_err(TransportError::Io)?;
        if reply.is_empty() {
            return Err(TransportError::Protocol(
                "daemon closed the connection".to_string(),
            ));
        }
        Response::from_line(&reply).map_err(|err| TransportError::Protocol(err.to_string()))
    }
}

/// Runs one client subcommand against the Daemon; returns the exit code (0 on success and `--help`, 1 after one `error: ...` line on `err`).
pub fn run_cli(
    argv: &[String],
    transport: &mut dyn Transport,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let cli = match parse(argv, out, err) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let result = request_for(cli.cmd).and_then(|req| {
        match transport.request(&req).map_err(|e| e.to_string())? {
            Response::Ok(payload) => Ok(render(&req, &payload)),
            Response::Err { error } => Err(error),
        }
    });
    let written = match result {
        Ok(text) => write!(out, "{text}").map(|()| 0),
        Err(message) => writeln!(err, "error: {message}").map(|()| 1),
    };
    written.unwrap_or(1)
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
    }
}

fn render_status(payload: &Value) -> String {
    let mut lines: String = [
        "preset",
        "program",
        "sample_rate",
        "buffer_size",
        "xruns",
        "callback_max_us",
        "callback_last_us",
    ]
    .iter()
    .map(|key| format!("{key}: {}\n", text(&payload[key])))
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

fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        Value::Number(number) if number.is_f64() => number
            .as_f64()
            .map_or_else(|| number.to_string(), |float| float.to_string()),
        other => other.to_string(),
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::NotRunning(path) => {
                write!(f, "daemon not running (socket {})", path.display())
            }
            TransportError::Io(err) => write!(f, "control socket: {err}"),
            TransportError::Protocol(msg) => write!(f, "bad response from daemon: {msg}"),
        }
    }
}

impl std::error::Error for TransportError {}
