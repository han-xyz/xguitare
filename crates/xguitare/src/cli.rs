use std::fmt::{Display, Write as _};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use xguitare_client::{
    TakesCleanStopped, Transport, clean_takes_in_batches, newer_protocol_warning,
};
use xguitare_protocol::{
    GetReply, IndexOrName, ItemsReply, MetronomeStatus, Position, RenderBits, RenderRange, Request,
    Response, SILENCE_DB, SendLevel, StatusReply, TransportAction, Value, from_payload,
    position_frame, shown_file_name,
};

use crate::{
    BusCmd, BypassState, ClipCmd, ClipKind, Cmd, IrCmd, ItemCmd, MetronomeCmd, MuteState,
    ProjectCmd, RackCmd, RenderArgs, RenderBitsArg, RenderCmd, Switch, TakesCmd, TrackCmd, parse,
};

const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const WATCH_LINE_COLUMNS: usize = 24;
const BAD_DELTA: &str = "move needs <±seconds>s or <±bars>[.<beats>]";
const NO_SAMPLE_RATE: &str = "the items answer has no sample_rate";

/// What a client command needs from its surroundings besides the transport.
pub struct CliEnv {
    /// The config's `tuner_mute`: whether `tuner --watch` turns Mute on while it runs.
    pub tuner_mute: bool,
    /// Whether stdout is a terminal: `tuner --watch` then rewrites one line instead of printing one per reading.
    pub is_tty: bool,
    /// Set to end `tuner --watch`.
    pub stop: Arc<AtomicBool>,
}

/// Runs one client subcommand against the Daemon; returns the exit code (0 on success and `--help`, 1 after `error: ...` lines on `err`).
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
    let mut warnings = String::new();
    let result = match cli.cmd {
        Cmd::Tuner { watch: true } => watch_tuner(transport, out, env).map(|()| String::new()),
        Cmd::Takes(TakesCmd::Clean { yes }) => return clean_takes(transport, yes, out, err),
        cmd => request_for(cmd, transport).and_then(|req| match req {
            Request::Render { .. } => render_and_wait(transport, out, &req),
            req => {
                let payload = request(transport, &req)?;
                warnings = warning_lines(&req, &payload);
                render(&req, &payload)
            }
        }),
    };
    let written = match result {
        Ok(text) => write!(err, "{warnings}")
            .and_then(|()| write!(out, "{text}"))
            .map(|()| 0),
        Err(message) => write!(err, "{warnings}")
            .and_then(|()| writeln!(err, "error: {}", without_control_chars(&message)))
            .map(|()| 1),
    };
    written.unwrap_or(1)
}

/// A `warning: ...` line for each of the reply's `warnings`, which `project_open` gives, and one for a `status` from a Daemon with a newer protocol.
fn warning_lines(req: &Request, payload: &Value) -> String {
    let mut lines = String::new();
    for warning in items(&payload["warnings"]) {
        let _ = writeln!(lines, "warning: {}", text(warning));
    }
    if matches!(req, Request::Status)
        && let Some(warning) = newer_protocol_warning(payload)
    {
        let _ = writeln!(lines, "warning: {warning}");
    }
    lines
}

fn request(transport: &mut dyn Transport, req: &Request) -> Result<Value, String> {
    match transport.request(req).map_err(|e| e.to_string())? {
        Response::Ok(payload) => Ok(payload),
        Response::Err { error, .. } => Err(error),
    }
}

fn status_reply(payload: &Value) -> Result<StatusReply, String> {
    from_payload(payload).map_err(|error| malformed("status", &error))
}

fn items_reply(payload: &Value) -> Result<ItemsReply, String> {
    from_payload(payload).map_err(|error| malformed("items", &error))
}

fn malformed(answer: &str, error: &impl Display) -> String {
    format!("the {answer} answer is malformed: {error}")
}

/// Lists the unused Take files; with `yes`, deletes them all instead and writes an `error:` line per file that could not be deleted. The exit code as [`run_cli`].
fn clean_takes(
    transport: &mut dyn Transport,
    yes: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let (text, errors) = match deleted_takes(transport, yes) {
        Ok(text_and_failures) => text_and_failures,
        Err(message) => (String::new(), vec![message]),
    };
    let mut written = write!(out, "{text}");
    for error in &errors {
        written = written.and(writeln!(err, "error: {}", without_control_chars(error)));
    }
    i32::from(written.is_err() || !errors.is_empty())
}

/// The `takes clean` output, and one `could not delete <file>: <error>` per file the Daemon could not delete, then the error that stopped the batches.
fn deleted_takes(
    transport: &mut dyn Transport,
    yes: bool,
) -> Result<(String, Vec<String>), String> {
    let unused = request(transport, &Request::TakesUnused)?;
    let files: Vec<String> = items(&unused["files"])
        .filter_map(|file| file["file"].as_str().map(str::to_string))
        .collect();
    if !yes || files.is_empty() {
        let mut lines = render_takes_unused(&unused);
        if !yes {
            lines.push_str("nothing deleted; pass --yes to delete\n");
        }
        return Ok((lines, Vec::new()));
    }
    let (payload, stopped_by) = match clean_takes_in_batches(&files, |req| transport.request(req)) {
        Ok(payload) => (payload, None),
        Err(TakesCleanStopped {
            cleaned: None,
            error,
        }) => return Err(error),
        Err(TakesCleanStopped {
            cleaned: Some(payload),
            error,
        }) => (payload, Some(error)),
    };
    let mut errors: Vec<String> = items(&payload["failed"])
        .map(|failed| {
            format!(
                "could not delete {}: {}",
                shown_take_file(&failed["file"]),
                text(&failed["error"])
            )
        })
        .collect();
    errors.extend(stopped_by);
    Ok((render(&Request::TakesClean { files }, &payload)?, errors))
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

/// Starts the Render, prints its file, a line at each further tenth of progress, and `rendered <file>` once it is done.
fn render_and_wait(
    transport: &mut dyn Transport,
    out: &mut dyn Write,
    req: &Request,
) -> Result<String, String> {
    let started = request(transport, req)?;
    write!(out, "{}", render(req, &started)?)
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())?;
    let mut tenths = 0;
    loop {
        thread::sleep(WATCH_INTERVAL);
        let job = status_reply(&request(transport, &Request::Status)?)?.render;
        match job.as_ref().map(|job| job.state.as_str()) {
            Some("running") => {
                let progress = job.as_ref().map_or(0.0, |job| job.progress);
                let now = (progress * 10.0).floor() as u32;
                if now > tenths {
                    tenths = now;
                    writeln!(out, "{}%", now * 10)
                        .and_then(|()| out.flush())
                        .map_err(|e| e.to_string())?;
                }
            }
            Some("done") => {
                let file = job.as_ref().map(|job| job.file.as_str());
                return Ok(format!("rendered {}\n", shown(file)));
            }
            Some("cancelled") => return Err("render cancelled".to_string()),
            _ => {
                let error = job.as_ref().and_then(|job| job.error.as_deref());
                return Err(format!("render failed: {}", shown(error)));
            }
        }
    }
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

/// The open Project's sample rate, tempo and beats per bar, which turn the times of an Item command into frames.
struct Timing {
    sample_rate: u32,
    tempo_bpm: f64,
    beats_per_bar: u32,
}

impl Timing {
    /// Reads the sample rate from `items` and the tempo from `status`.
    fn read(transport: &mut dyn Transport) -> Result<Timing, String> {
        let items = items_reply(&request(transport, &Request::Items)?)?;
        let status = status_reply(&request(transport, &Request::Status)?)?;
        let tempo = status.tempo.ok_or("the status answer has no tempo")?;
        if items.sample_rate == 0 {
            return Err(NO_SAMPLE_RATE.to_string());
        }
        Ok(Timing {
            sample_rate: items.sample_rate,
            tempo_bpm: tempo.bpm,
            beats_per_bar: tempo.meter.0,
        })
    }

    fn frame(&self, time: &str) -> Result<u64, String> {
        position_frame(time, self.sample_rate, self.tempo_bpm, self.beats_per_bar)
            .map(|frame| frame as u64)
    }

    /// `±<seconds>s`, or `±<bars>[.<beats>]` counted from 0, in frames; no sign is later.
    fn delta(&self, delta: &str) -> Result<i64, String> {
        let (sign, magnitude) = match delta.strip_prefix('-') {
            Some(rest) => (-1.0, rest),
            None => (1.0, delta.strip_prefix('+').unwrap_or(delta)),
        };
        let frames = if let Some(seconds) = magnitude.strip_suffix('s') {
            seconds
                .parse::<f64>()
                .ok()
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .ok_or(BAD_DELTA)?
                * f64::from(self.sample_rate)
        } else {
            let (bars, beats) = magnitude.split_once('.').unwrap_or((magnitude, "0"));
            let (Ok(bars), Ok(beats)) = (bars.parse::<u32>(), beats.parse::<u32>()) else {
                return Err(BAD_DELTA.to_string());
            };
            if beats >= self.beats_per_bar {
                return Err(BAD_DELTA.to_string());
            }
            let beat_frames = f64::from(self.sample_rate) * 60.0 / self.tempo_bpm;
            (f64::from(bars) * f64::from(self.beats_per_bar) + f64::from(beats)) * beat_frames
        };
        Ok((sign * frames).round() as i64)
    }
}

fn item_request(transport: &mut dyn Transport, cmd: ItemCmd) -> Result<Request, String> {
    Ok(match cmd {
        ItemCmd::List => Request::Items,
        ItemCmd::Delete { ids } => Request::ItemDelete { ids },
        ItemCmd::Duplicate { ids } => Request::ItemDuplicate { ids },
        ItemCmd::Copy { ids } => Request::ItemCopy { ids },
        ItemCmd::Paste { track, at } => Request::ItemPaste {
            at: Timing::read(transport)?.frame(&at)?,
            track: index_or_name(track),
        },
        ItemCmd::Take { id, take } => Request::ItemTake {
            id,
            take: take.checked_sub(1).ok_or("takes are numbered from 1")?,
        },
        ItemCmd::Move { mut ids, track } => {
            if ids.iter().any(|id| id == "--track") {
                return Err("give --track before the item ids".to_string());
            }
            let delta = ids.pop().unwrap_or_default();
            let ids = ids
                .iter()
                .map(|id| {
                    id.parse()
                        .map_err(|_| format!("item id '{id}' is not a number"))
                })
                .collect::<Result<Vec<u64>, _>>()?;
            Request::ItemMove {
                ids,
                delta: Timing::read(transport)?.delta(&delta)?,
                track: track.map(index_or_name),
            }
        }
        ItemCmd::Trim { id, start, end } => {
            if start.is_none() && end.is_none() {
                return Err("item trim needs --start, --end or both".to_string());
            }
            let timing = Timing::read(transport)?;
            Request::ItemTrim {
                id,
                start: start.map(|time| timing.frame(&time)).transpose()?,
                end: end.map(|time| timing.frame(&time)).transpose()?,
            }
        }
        ItemCmd::Split { ids, at } => Request::ItemSplit {
            at: Timing::read(transport)?.frame(&at)?,
            ids: (!ids.is_empty()).then_some(ids),
        },
    })
}

/// The request for `cmd`; an Item command first reads the Project's timing through `client`.
fn request_for(cmd: Cmd, client: &mut dyn Transport) -> Result<Request, String> {
    Ok(match cmd {
        Cmd::Run => return Err("'run' starts the daemon; it is not a client command".to_string()),
        Cmd::Status => Request::Status,
        Cmd::List => Request::List,
        Cmd::Load { preset } => Request::Load {
            preset,
            track: None,
            bus: None,
            master: false,
        },
        Cmd::Set { param, value, rack } => {
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
                bus: rack.bus.map(index_or_name),
                master: rack.master,
            }
        }
        Cmd::Get { param, rack } => Request::Get {
            param,
            track: None,
            bus: rack.bus.map(index_or_name),
            master: rack.master,
        },
        Cmd::Save { name } => Request::Save { name },
        Cmd::Modules => Request::Modules,
        Cmd::Rack(cmd) => rack_request(cmd),
        Cmd::Ir(IrCmd::List) => Request::IrList,
        Cmd::Ir(IrCmd::Set { file, id, rack }) => Request::RackIr {
            ir: file,
            id,
            track: None,
            bus: rack.bus.map(index_or_name),
            master: rack.master,
        },
        Cmd::Tuner { .. } => Request::Tuner,
        Cmd::Mute { state } => Request::Mute {
            on: matches!(state, MuteState::On),
        },
        Cmd::Bypass { id, state, rack } => Request::Bypass {
            id,
            on: matches!(state, BypassState::On),
            track: None,
            bus: rack.bus.map(index_or_name),
            master: rack.master,
        },
        Cmd::Project(cmd) => project_request(cmd),
        Cmd::Track(cmd) => track_request(cmd),
        Cmd::Item(cmd) => item_request(client, cmd)?,
        Cmd::Takes(TakesCmd::Unused) => Request::TakesUnused,
        Cmd::Takes(TakesCmd::Clean { .. }) => {
            unreachable!("run_cli handles takes clean before request_for")
        }
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
        Cmd::Metronome(cmd) => metronome_request(cmd)?,
        Cmd::Bus(cmd) => bus_request(cmd),
        Cmd::Send {
            track,
            bus,
            level,
            pre,
        } => Request::SendSet {
            track: index_or_name(track),
            bus: index_or_name(bus),
            level: Some(parse_level(&level)?),
            pre: pre.map(is_on),
        },
        Cmd::Master { volume } => Request::MasterSet {
            volume: parse_master_volume(&volume)?,
        },
        Cmd::Clip(ClipCmd::Reset { kind, name }) => clip_reset_request(kind, name)?,
        Cmd::Render(args) => render_request(args),
    })
}

fn metronome_request(cmd: MetronomeCmd) -> Result<Request, String> {
    let change = |on, bpm, meter, volume| Request::Metronome {
        on,
        bpm,
        meter,
        volume,
    };
    Ok(match cmd {
        MetronomeCmd::On => change(Some(true), None, None, None),
        MetronomeCmd::Off => change(Some(false), None, None, None),
        MetronomeCmd::Tempo { bpm, meter } => change(
            None,
            Some(bpm),
            meter.as_deref().map(parse_meter).transpose()?,
            None,
        ),
        MetronomeCmd::Volume { db } => change(None, None, None, Some(db)),
        MetronomeCmd::Tap => Request::MetronomeTap,
    })
}

fn rack_request(cmd: RackCmd) -> Request {
    match cmd {
        RackCmd::Add {
            type_name,
            after,
            at,
            rack,
        } => Request::RackAdd {
            r#type: type_name,
            after,
            at,
            track: None,
            bus: rack.bus.map(index_or_name),
            master: rack.master,
        },
        RackCmd::Remove { id, rack } => Request::RackRemove {
            id,
            track: None,
            bus: rack.bus.map(index_or_name),
            master: rack.master,
        },
        RackCmd::Move { id, to, rack } => Request::RackMove {
            id,
            to,
            track: None,
            bus: rack.bus.map(index_or_name),
            master: rack.master,
        },
    }
}

fn bus_request(cmd: BusCmd) -> Request {
    match cmd {
        BusCmd::Add { name } => Request::BusAdd { name },
        BusCmd::Remove { bus } => Request::BusRemove {
            bus: index_or_name(bus),
        },
        BusCmd::Edit {
            bus,
            name,
            volume,
            pan,
            mute,
            solo,
        } => Request::BusSet {
            bus: index_or_name(bus),
            name,
            volume,
            pan,
            mute: mute.map(is_on),
            solo: solo.map(is_on),
        },
    }
}

/// `off`, or a number in dB.
fn parse_level(level: &str) -> Result<SendLevel, String> {
    if level == "off" {
        return Ok(SendLevel::Off);
    }
    level
        .parse()
        .ok()
        .filter(|db: &f32| db.is_finite())
        .map(SendLevel::Db)
        .ok_or_else(|| "level must be a number in dB or off".to_string())
}

/// `off`, or a finite number in dB.
fn parse_master_volume(volume: &str) -> Result<SendLevel, String> {
    if volume == "off" {
        return Ok(SendLevel::Off);
    }
    volume
        .parse()
        .ok()
        .filter(|db: &f32| db.is_finite())
        .map(SendLevel::Db)
        .ok_or_else(|| "master volume must be off or between -60 and 6 dB".to_string())
}

fn clip_reset_request(kind: Option<ClipKind>, name: Option<String>) -> Result<Request, String> {
    let (track, bus, master) = match (kind, name) {
        (None, _) => (None, None, false),
        (Some(ClipKind::Master), None) => (None, None, true),
        (Some(ClipKind::Master), Some(_)) => {
            return Err("clip reset master takes no name".to_string());
        }
        (Some(ClipKind::Track), Some(name)) => (Some(index_or_name(name)), None, false),
        (Some(ClipKind::Bus), Some(name)) => (None, Some(index_or_name(name)), false),
        (Some(ClipKind::Track | ClipKind::Bus), None) => {
            return Err("clip reset track and bus need an index or a name".to_string());
        }
    };
    Ok(Request::ClipReset { track, bus, master })
}

fn render_request(args: RenderArgs) -> Request {
    if let Some(RenderCmd::Cancel) = args.action {
        return Request::RenderCancel;
    }
    Request::Render {
        range: args.loop_region.then_some(RenderRange::Loop),
        bits: args.bits.map(|bits| match bits {
            RenderBitsArg::Pcm16 => RenderBits::Pcm16,
            RenderBitsArg::Pcm24 => RenderBits::Pcm24,
            RenderBitsArg::Float32 => RenderBits::Float32,
        }),
        name: args.name,
    }
}

fn project_request(cmd: ProjectCmd) -> Request {
    match cmd {
        ProjectCmd::New { name } => Request::ProjectNew { name },
        ProjectCmd::Open { name, restore } => Request::ProjectOpen { name, restore },
        ProjectCmd::Save => Request::ProjectSave,
        ProjectCmd::Close => Request::ProjectClose,
        ProjectCmd::List => Request::ProjectList,
        ProjectCmd::Items => Request::Items,
        ProjectCmd::Undo => Request::ProjectUndo,
        ProjectCmd::Redo => Request::ProjectRedo,
    }
}

fn track_request(cmd: TrackCmd) -> Request {
    match cmd {
        TrackCmd::Add { name } => Request::TrackAdd { name },
        TrackCmd::Remove { track } => Request::TrackRemove {
            track: index_or_name(track),
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
            to_master,
        } => Request::TrackSet {
            track: index_or_name(track),
            input,
            volume,
            pan,
            mute: mute.map(is_on),
            solo: solo.map(is_on),
            arm: arm.map(is_on),
            name,
            to_master: to_master.map(is_on),
        },
        TrackCmd::Select { track } => Request::TrackSelect {
            track: index_or_name(track),
        },
    }
}

fn transport(action: TransportAction, position: Option<Position>) -> Request {
    Request::Transport { action, position }
}

fn is_on(state: Switch) -> bool {
    matches!(state, Switch::On)
}

/// Digits name a Track or Bus by index, anything else by name.
fn index_or_name(text: String) -> IndexOrName {
    match text.parse() {
        Ok(index) => IndexOrName::Index(index),
        Err(_) => IndexOrName::Name(text),
    }
}

fn parse_meter(meter: &str) -> Result<(u32, u32), String> {
    meter
        .split_once('/')
        .and_then(|(beats, note)| Some((beats.parse().ok()?, note.parse().ok()?)))
        .ok_or_else(|| "meter must be <beats>/<note>".to_string())
}

#[allow(clippy::too_many_lines, reason = "one match arm per request")]
fn render(req: &Request, payload: &Value) -> Result<String, String> {
    let rendered = match req {
        Request::Status => render_status(&status_reply(payload)?),
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
        Request::Get { param, .. } => render_get(
            &from_payload(payload).map_err(|error| malformed("get", &error))?,
            param.as_deref(),
        ),
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
        Request::ProjectUndo => format!("undid {}\n", text(&payload["label"])),
        Request::ProjectRedo => format!("redid {}\n", text(&payload["label"])),
        Request::Items => render_items(&items_reply(payload)?)?,
        Request::TakesUnused => render_takes_unused(payload),
        Request::TakesClean { .. } => {
            let deleted = payload["deleted"].as_u64().unwrap_or(0);
            format!(
                "deleted {deleted} {}, {} freed\n",
                takes_word(deleted),
                size_text(payload["bytes"].as_u64().unwrap_or(0))
            )
        }
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
            meter_text(text(&payload["meter"][0]), text(&payload["meter"][1]))
        ),
        Request::Click { .. } => format!(
            "{}\n",
            click_text(payload["on"] == true, text(&payload["count_in"]))
        ),
        Request::Metronome { .. } => format!(
            "metronome {}\n",
            metronome_text(
                payload["on"] == true,
                text(&payload["bpm"]),
                &meter_text(text(&payload["meter"][0]), text(&payload["meter"][1])),
                text(&payload["volume_db"])
            )
        ),
        Request::MetronomeTap => match &payload["bpm"] {
            Value::Null => "tap\n".to_string(),
            bpm => format!("tap {} bpm\n", text(bpm)),
        },
        Request::BusAdd { .. } => format!("added bus {}\n", track_label(&payload["bus"])),
        Request::BusRemove { .. } => format!("removed bus {}\n", track_label(&payload["bus"])),
        Request::BusSet { .. } => render_bus(&payload["bus"]),
        Request::SendSet { .. } => format!(
            "send track {} -> bus {}: {}, {}\n",
            text(&payload["track"]),
            text(&payload["bus"]),
            level_text(&payload["level"]),
            pre_text(payload["pre"] == true)
        ),
        Request::MasterSet { .. } => master_line(&payload["volume_db"]),
        Request::ClipReset { .. } => "clip reset\n".to_string(),
        Request::Render { .. } => format!("rendering {}\n", text(&payload["file"])),
        Request::RenderCancel => "render cancelling\n".to_string(),
        Request::ItemMove { .. }
        | Request::ItemTrim { .. }
        | Request::ItemSplit { .. }
        | Request::ItemTake { .. }
        | Request::ItemDuplicate { .. } => render_changed_items(payload),
        Request::ItemPaste { .. } => render_pasted_items(payload),
        Request::ItemDelete { .. } => {
            let ids: Vec<String> = items(&payload["deleted"]).map(text).collect();
            format!("deleted {}\n", ids.join(" "))
        }
        Request::ItemCopy { .. } => {
            let ids: Vec<String> = items(&payload["copied"]).map(text).collect();
            format!("copied {}\n", ids.join(" "))
        }
        Request::Ports
        | Request::SetPorts { .. }
        | Request::Settings
        | Request::SetSettings { .. }
        | Request::Loop { .. }
        | Request::TakePeaks { .. } => format!("{payload}\n"),
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

fn render_items(reply: &ItemsReply) -> Result<String, String> {
    if reply.sample_rate == 0 {
        return Err(NO_SAMPLE_RATE.to_string());
    }
    let sample_rate = f64::from(reply.sample_rate);
    let mut lines = String::new();
    for track in &reply.tracks {
        for item in &track.items {
            let seconds = item.length as f64 / sample_rate;
            let _ = writeln!(
                lines,
                "{} {} {} {seconds:.2}s take {}/{}",
                item.id,
                without_control_chars(&track.name),
                without_control_chars(&item.bars_beats),
                item.take + 1,
                item.takes.len()
            );
        }
    }
    Ok(lines)
}

/// One line per unused Take file with its size, then the total.
fn render_takes_unused(payload: &Value) -> String {
    let mut lines = String::new();
    let files = items(&payload["files"]);
    let mut count = 0;
    for file in files {
        count += 1;
        let _ = writeln!(
            lines,
            "{} {}",
            shown_take_file(&file["file"]),
            size_text(file["bytes"].as_u64().unwrap_or(0))
        );
    }
    let total = size_text(payload["bytes"].as_u64().unwrap_or(0));
    let _ = writeln!(lines, "{count} unused {}, {total}", takes_word(count));
    lines
}

fn takes_word(count: u64) -> &'static str {
    if count == 1 { "take" } else { "takes" }
}

/// A Take file name from the Daemon with its control characters and bidi overrides escaped.
fn shown_take_file(file: &Value) -> String {
    shown_file_name(file.as_str().unwrap_or("-"))
}

/// `bytes` in B, KB, MB or GB (powers of 1000), one decimal past B.
fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss, reason = "a size shown to one decimal")]
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// One line per Item an edit changed: `<id> track <index> at <bar.beat> (frame <position>), <length> frames, take <n>/<takes>`.
fn render_pasted_items(payload: &Value) -> String {
    let mut lines = render_changed_items(payload);
    match payload["dropped"].as_u64().unwrap_or(0) {
        0 => {}
        1 => lines.push_str("dropped 1 Item: no Track to paste it on\n"),
        dropped => {
            let _ = writeln!(lines, "dropped {dropped} Items: no Track to paste them on");
        }
    }
    lines
}

fn render_changed_items(payload: &Value) -> String {
    let mut lines = String::new();
    for item in items(&payload["items"]) {
        let _ = writeln!(
            lines,
            "{} track {} at {} (frame {}), {} frames, take {}/{}",
            text(&item["id"]),
            text(&item["track"]),
            text(&item["bars_beats"]),
            text(&item["position"]),
            text(&item["length"]),
            item["take"].as_u64().unwrap_or(0) + 1,
            items(&item["takes"]).count()
        );
    }
    lines
}

fn track_label(payload: &Value) -> String {
    format!("{} {}", text(&payload["index"]), text(&payload["name"]))
}

fn render_track(payload: &Value) -> String {
    let yes_no = |key: &str| if payload[key] == true { "yes" } else { "no" };
    format!(
        "track {}: input {}, volume {} dB, pan {}, mute {}, solo {}, arm {}, to master {}\n",
        track_label(payload),
        text(&payload["input"]),
        text(&payload["volume_db"]),
        text(&payload["pan"]),
        yes_no("mute"),
        yes_no("solo"),
        yes_no("arm"),
        yes_no("to_master"),
    )
}

fn render_bus(bus: &Value) -> String {
    let yes_no = |key: &str| if bus[key] == true { "yes" } else { "no" };
    format!(
        "bus {}: volume {} dB, pan {}, mute {}, solo {}\n",
        track_label(bus),
        text(&bus["volume_db"]),
        text(&bus["pan"]),
        yes_no("mute"),
        yes_no("solo"),
    )
}

fn level_text(level: &Value) -> String {
    if level.is_number() {
        format!("{} dB", text(level))
    } else {
        text(level)
    }
}

fn pre_text(pre: bool) -> &'static str {
    if pre { "pre" } else { "post" }
}

fn meter_text(beats: impl Display, note: impl Display) -> String {
    format!("{beats}/{note}")
}

fn metronome_text(on: bool, bpm: impl Display, meter: &str, volume_db: impl Display) -> String {
    let on = if on { "on" } else { "off" };
    format!("{on}, {bpm} bpm {meter}, volume {volume_db} dB")
}

fn click_text(on: bool, count_in: impl Display) -> String {
    let on = if on { "on" } else { "off" };
    format!("click {on}, count-in {count_in} bars")
}

/// After a cabinet Module's last Parameter line comes `<id>.ir = <name>`, or `none` when it has no Cabinet IR; then `<id>.bypass = yes|no`; a `key` naming one Parameter gets neither line.
fn render_get(reply: &GetReply, key: Option<&str>) -> String {
    let names_one_param = key.is_some_and(|key| key.contains('.'));
    let mut lines = String::new();
    for (index, param) in reply.params.iter().enumerate() {
        let module = without_control_chars(&param.module);
        let line = format!(
            "{module}.{} = {} {}",
            without_control_chars(&param.param),
            param.value,
            without_control_chars(&param.unit)
        );
        let _ = writeln!(lines, "{}", line.trim_end());
        let is_last_of_module = reply
            .params
            .get(index + 1)
            .is_none_or(|next| next.module != param.module);
        if !is_last_of_module || names_one_param {
            continue;
        }
        if let Some(ir) = &param.ir {
            let name = ir
                .as_deref()
                .map_or_else(|| "none".to_string(), without_control_chars);
            let _ = writeln!(lines, "{module}.ir = {name}");
        }
        let answer = if param.bypass { "yes" } else { "no" };
        let _ = writeln!(lines, "{module}.bypass = {answer}");
    }
    lines
}

fn render_status(status: &StatusReply) -> String {
    let mut lines = String::new();
    let peak = |db: Option<f64>| db.map_or_else(|| "-".to_string(), |db| format!("{db:.1}"));
    let modified = if status.modified { "yes" } else { "no" };
    let _ = writeln!(lines, "preset: {}", shown(status.preset.as_deref()));
    let _ = writeln!(lines, "program: {}", or_dash(status.program));
    let _ = writeln!(lines, "modified: {modified}");
    let _ = writeln!(lines, "sample_rate: {}", status.sample_rate);
    let _ = writeln!(lines, "buffer_size: {}", status.buffer_size);
    let _ = writeln!(lines, "prepared_frames: {}", status.prepared_frames);
    let _ = writeln!(lines, "revision: {}", status.revision);
    let _ = writeln!(lines, "muted: {}", status.muted);
    let _ = writeln!(lines, "input_peak_db: {}", peak(status.input_peak_db));
    let _ = writeln!(lines, "output_peak_db: {}", peak(status.output_peak_db));
    let _ = writeln!(lines, "xruns: {}", status.xruns);
    let _ = writeln!(lines, "callback_max_us: {}", status.callback_max_us);
    let _ = writeln!(lines, "callback_last_us: {}", status.callback_last_us);
    if let Some(version) = status.protocol_version {
        let _ = writeln!(lines, "protocol_version: {version}");
    }
    lines.push_str("connections:\n");
    for connection in &status.connections {
        let _ = writeln!(
            lines,
            "  {} -> {}",
            without_control_chars(&connection.from),
            without_control_chars(&connection.to)
        );
    }
    let _ = writeln!(lines, "disk_overflows: {}", status.disk_overflows);
    let _ = writeln!(
        lines,
        "metronome: {}, beat {}",
        status_metronome_text(&status.metronome),
        status.metronome.beat
    );
    let _ = writeln!(lines, "project: {}", shown(status.project.as_deref()));
    if let Some(job) = &status.render {
        let percent = (job.progress * 100.0).round();
        let _ = write!(
            lines,
            "render: {} ({}) {} {percent}% {}",
            without_control_chars(&job.name),
            render_range_text(job.range),
            without_control_chars(&job.state),
            without_control_chars(&job.file)
        );
        if let Some(error) = &job.error {
            let _ = write!(lines, ": {}", without_control_chars(error));
        }
        lines.push('\n');
    }
    if status.project.is_none() {
        return lines;
    }
    render_project_status(&mut lines, status);
    lines
}

fn status_metronome_text(metronome: &MetronomeStatus) -> String {
    let (beats, note) = metronome.meter;
    metronome_text(
        metronome.on,
        metronome.bpm,
        &meter_text(beats, note),
        metronome.volume_db,
    )
}

fn render_project_status(lines: &mut String, status: &StatusReply) {
    let position = status.position.as_ref();
    let tempo = status.tempo.as_ref();
    let looped = status.loop_region.as_ref();
    let click = status.click.as_ref();
    let _ = writeln!(lines, "transport: {}", shown(status.transport.as_deref()));
    let _ = writeln!(
        lines,
        "position: {} (frame {})",
        shown(position.map(|position| position.bars_beats.as_str())),
        or_dash(position.map(|position| position.frames))
    );
    let _ = writeln!(
        lines,
        "tempo: {} bpm {}",
        or_dash(tempo.map(|tempo| tempo.bpm)),
        meter_text(
            or_dash(tempo.map(|tempo| tempo.meter.0)),
            or_dash(tempo.map(|tempo| tempo.meter.1))
        )
    );
    let _ = writeln!(
        lines,
        "loop: {} {}..{}",
        if looped.is_some_and(|looped| looped.on) {
            "on"
        } else {
            "off"
        },
        or_dash(looped.map(|looped| looped.start)),
        or_dash(looped.map(|looped| looped.end))
    );
    let _ = writeln!(
        lines,
        "{}",
        click_text(
            click.is_some_and(|click| click.on),
            or_dash(click.map(|click| click.count_in))
        )
    );
    let (undo, redo) = (shown(status.undo.as_deref()), shown(status.redo.as_deref()));
    let _ = writeln!(lines, "undo: {undo}, redo: {redo}");
    render_tracks(lines, status);
    render_buses(lines, status);
    let master = status.master.as_ref();
    let _ = writeln!(
        lines,
        "master: {} dB; {}",
        or_dash(master.and_then(|master| master.volume_db)),
        out_text(
            master.map_or([None, None], |master| master.peak_db),
            master.is_some_and(|master| master.clip)
        )
    );
}

fn render_tracks(lines: &mut String, status: &StatusReply) {
    lines.push_str("tracks:\n");
    for (index, track) in status.tracks.iter().enumerate() {
        let selected = if status.selected == Some(index) {
            "*"
        } else {
            " "
        };
        let mut flags: Vec<&str> = [
            (track.armed, "armed"),
            (track.mute, "muted"),
            (track.solo, "solo"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, flag)| flag)
        .collect();
        if !track.to_master {
            flags.push("not to master");
        }
        let flags = if flags.is_empty() {
            "-".to_string()
        } else {
            flags.join(", ")
        };
        let peak = track.input_peak_db.unwrap_or(SILENCE_DB);
        let _ = writeln!(
            lines,
            "  {index}{selected} {}: {flags}; input {}; {} dB, pan {}; peak {peak:.1} dB; {}",
            without_control_chars(&track.name),
            shown(track.input.as_deref()),
            or_dash(track.volume_db),
            track.pan,
            out_text(track.peak_db, track.clip),
        );
        for send in &track.sends {
            let bus = status.buses.get(send.bus).map(|bus| bus.name.as_str());
            let _ = writeln!(
                lines,
                "      send {}: {} dB, {}",
                shown(bus),
                send.level_db,
                pre_text(send.pre)
            );
        }
    }
}

fn render_buses(lines: &mut String, status: &StatusReply) {
    lines.push_str("buses:\n");
    for (index, bus) in status.buses.iter().enumerate() {
        let flags: Vec<&str> = [(bus.mute, "muted"), (bus.solo, "solo")]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, flag)| flag)
            .collect();
        let flags = if flags.is_empty() {
            "-".to_string()
        } else {
            flags.join(", ")
        };
        let _ = writeln!(
            lines,
            "  {index} {}: {flags}; {} dB, pan {}; {}",
            without_control_chars(&bus.name),
            or_dash(bus.volume_db),
            bus.pan,
            out_text(bus.peak_db, bus.clip),
        );
    }
}

/// `out <left>/<right> dB`, then `, clip` when the clip flag is set.
fn out_text(peak_db: [Option<f64>; 2], clip: bool) -> String {
    let [left, right] = peak_db.map(|side| side.unwrap_or(SILENCE_DB));
    let clip = if clip { ", clip" } else { "" };
    format!("out {left:.1}/{right:.1} dB{clip}")
}

fn shown(text: Option<&str>) -> String {
    text.map_or_else(|| "-".to_string(), without_control_chars)
}

fn render_range_text(range: RenderRange) -> &'static str {
    match range {
        RenderRange::Project => "project",
        RenderRange::Loop => "loop",
    }
}

fn or_dash(value: Option<impl Display>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
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
        let params: Vec<String> = items(&module["params"]).map(param_name).collect();
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

/// A choice Parameter's name is followed by its labels: `mode (lp|bp|hp)`.
fn param_name(param: &Value) -> String {
    let labels: Vec<String> = items(&param["labels"]).map(text).collect();
    if labels.is_empty() {
        return text(&param["name"]);
    }
    format!("{} ({})", text(&param["name"]), labels.join("|"))
}

fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn master_line(volume_db: &Value) -> String {
    if volume_db.is_null() {
        return "master off\n".to_string();
    }
    format!("master {} dB\n", text(volume_db))
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
