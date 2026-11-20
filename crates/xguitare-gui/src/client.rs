use std::collections::{HashMap, HashSet};
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::mixer_model::{MixerTarget, ShownRack, rack_selectors};
use crate::model::{
    Command, Link, Message, Outgoing, Restore, Snapshot, bank_rows, counts_toward_spawn,
    failed_set, failure_title, ir_names, module_types, param_rows, refresh_decision,
    refused_remove, refused_render, requests_until_quit, response_message, spawn_policy,
    takes_cleaned_message,
};
use crate::project_model::{autosave_offer, item_blocks, project_name, still_closing};
use serde_json::Value;
use tracing::{info, warn};
use xguitare_client::{ClientConfig, ClientError, Connection, clean_takes_in_batches};
use xguitare_protocol::{Request, Response, STILL_CLOSING, newer_protocol_warning};

pub const TICK: Duration = Duration::from_millis(50);
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_EXECUTABLE: &str = "xguitare";
const REOPEN_TRIES: u32 = 40;

pub fn spawn(
    config: ClientConfig,
    config_path: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    repaint: impl Fn() + Send + 'static,
) -> (Sender<Command>, Receiver<Snapshot>, JoinHandle<()>) {
    let (command_tx, command_rx) = mpsc::channel();
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        Client::new(config, config_path, stop).run(&command_rx, &snapshot_tx, &repaint);
    });
    (command_tx, snapshot_rx, handle)
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent per-request fetch flags, not states of one machine"
)]
struct Client {
    config: ClientConfig,
    config_path: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    connection: Option<Connection>,
    last_ignored_error: Option<String>,
    failures: u32,
    last_spawn: Option<Instant>,
    daemon: Option<Child>,
    revision: Option<u64>,
    modules_fetched: bool,
    modules_warned: bool,
    irs_fetched: bool,
    ports_fetched: bool,
    settings_fetched: bool,
    projects_fetched: bool,
    tuner_answered: bool,
    /// The Rack the window last named; a Track stands for the selected one.
    shown_rack: Option<ShownRack>,
    /// The Project the cached peak tables belong to.
    peaks_project: Option<String>,
    /// The Take frames each cached peak table was fetched for.
    peak_frames: HashMap<String, u64>,
    /// The Take files and frames whose peaks the Daemon refused, not asked for again.
    refused_peaks: HashSet<(String, u64)>,
    snapshot: Snapshot,
}

impl Client {
    fn new(config: ClientConfig, config_path: Option<PathBuf>, stop: Arc<AtomicBool>) -> Client {
        Client {
            config,
            config_path,
            stop,
            connection: None,
            last_ignored_error: None,
            failures: 0,
            last_spawn: None,
            daemon: None,
            revision: None,
            modules_fetched: false,
            modules_warned: false,
            irs_fetched: false,
            ports_fetched: false,
            settings_fetched: false,
            projects_fetched: false,
            tuner_answered: false,
            shown_rack: None,
            peaks_project: None,
            peak_frames: HashMap::new(),
            refused_peaks: HashSet::new(),
            snapshot: Snapshot {
                link: Link::Disconnected {
                    since: Instant::now(),
                },
                status: None,
                bank: None,
                params: None,
                tuner: None,
                ports: None,
                settings: None,
                projects: None,
                items: None,
                peaks: HashMap::new(),
                module_types: Vec::new(),
                irs: Vec::new(),
                last_message: None,
                failed_set: None,
                refused_remove: None,
                refused_render: None,
                autosave_offer: None,
                unused_takes: None,
            },
        }
    }

    fn run(
        mut self,
        commands: &Receiver<Command>,
        snapshots: &Sender<Snapshot>,
        repaint: &impl Fn(),
    ) {
        let mut sent: Option<Snapshot> = None;
        let mut next_poll = Instant::now();
        let mut stop_seen = false;
        loop {
            // The thread ends only on the UI's Quit, so that the unmute on_exit queues before it still goes out; a signal only wakes the UI to close the window.
            if !stop_seen && self.stop.load(Ordering::Relaxed) {
                stop_seen = true;
                repaint();
            }
            let now = Instant::now();
            if now >= next_poll {
                self.poll(now);
                next_poll = now + TICK;
            }
            if !self.publish(&mut sent, snapshots, repaint) {
                break;
            }
            let first =
                match commands.recv_timeout(next_poll.saturating_duration_since(Instant::now())) {
                    Ok(command) => command,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                };
            if !self.handle_commands(first, commands) {
                break;
            }
        }
        self.stop_daemon();
        repaint();
    }

    fn publish(
        &self,
        sent: &mut Option<Snapshot>,
        snapshots: &Sender<Snapshot>,
        repaint: &impl Fn(),
    ) -> bool {
        if sent.as_ref() == Some(&self.snapshot) {
            return true;
        }
        if snapshots.send(self.snapshot.clone()).is_err() {
            return false;
        }
        repaint();
        *sent = Some(self.snapshot.clone());
        true
    }

    fn handle_commands(&mut self, first: Command, commands: &Receiver<Command>) -> bool {
        let mut queued = vec![first];
        let ui_gone = loop {
            match commands.try_recv() {
                Ok(command) => queued.push(command),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        if queued.contains(&Command::RefreshIrs) {
            self.irs_fetched = false;
        }
        if queued.contains(&Command::RefreshPorts) {
            self.ports_fetched = false;
        }
        if queued.contains(&Command::RefreshProjects) {
            self.projects_fetched = false;
        }
        let queued_refresh_unused = queued.contains(&Command::RefreshUnusedTakes);
        let rack = self.rack_target();
        let rack_moved = self.follow_shown_rack(&queued);
        let (requests, quit) = requests_until_quit(queued, rack, self.snapshot.status.as_ref());
        let sent = requests.len();
        for outgoing in requests {
            match outgoing {
                Outgoing::Request(Request::TakesClean { files }) => self.clean_takes(files),
                Outgoing::Request(req) => {
                    self.send(&req);
                }
                Outgoing::Restore(restore) => self.restore(&restore),
                Outgoing::RestoreAutosave(name) => self.restore_autosave(name),
            }
        }
        if quit {
            info!("quit: sent {sent} final requests");
        } else if queued_refresh_unused {
            let reply = match self.request(&Request::TakesUnused) {
                Ok(Response::Ok(payload)) => Ok(payload),
                Ok(Response::Err { error, .. }) => Err(error),
                Err(err) => Err(err.to_string()),
            };
            self.snapshot.unused_takes = Some((reply, Instant::now()));
        }
        if rack_moved && !quit {
            self.snapshot.params = self.payload(&self.rack_get());
        }
        !quit && !ui_gone
    }

    /// Sends `req` and notes its result in the snapshot; the payload of a success.
    fn send(&mut self, req: &Request) -> Option<Value> {
        let response = self.request(req);
        if let (Request::Mute { .. }, Ok(Response::Ok(payload))) = (req, &response) {
            log_mute(payload);
        }
        if let Request::RackIr { .. } = req {
            log_rack_ir(&response);
        }
        match req {
            Request::SetPorts { .. } => self.ports_fetched = false,
            Request::SetSettings { .. } => {
                self.settings_fetched = false;
                self.irs_fetched = false;
                self.projects_fetched = false;
            }
            Request::ProjectNew { .. }
            | Request::ProjectOpen { .. }
            | Request::ProjectSave
            | Request::ProjectClose => self.projects_fetched = false,
            _ => {}
        }
        if let Some(param) = failed_set(req, &response) {
            self.snapshot.failed_set = Some((param, Instant::now()));
        }
        if let Some(offer) = autosave_offer(req, &response) {
            self.snapshot.autosave_offer = Some((offer, Instant::now()));
        }
        if let Some(id) = refused_remove(req, &response) {
            self.snapshot.refused_remove = Some((id, Instant::now()));
        }
        if let Some(error) = refused_render(req, &response) {
            self.snapshot.refused_render = Some((error, Instant::now()));
        }
        let payload = match &response {
            Ok(Response::Ok(payload)) => Some(payload.clone()),
            _ => None,
        };
        if let Some(message) = response_message(req, response) {
            self.snapshot.last_message = Some((message, Instant::now()));
        }
        payload
    }

    /// Sends `takes_clean` for `files` in batches under the Daemon's line limit and notes the summed result.
    fn clean_takes(&mut self, files: Vec<String>) {
        let cleaned = clean_takes_in_batches(&files, |batch| self.request(batch));
        let any_batch_done = match &cleaned {
            Ok(_) => true,
            Err(stopped) => stopped.cleaned.is_some(),
        };
        // A Take file name is used again once its file is gone, maybe for a Take of the same frames.
        if any_batch_done {
            self.snapshot.peaks.clear();
            self.peak_frames.clear();
            self.refused_peaks.clear();
            self.snapshot.unused_takes = None;
        }
        if let Some(message) = takes_cleaned_message(&Request::TakesClean { files }, cleaned) {
            self.snapshot.last_message = Some((message, Instant::now()));
        }
    }

    /// The follow-ups go to the id the Daemon gave the re-added Module, and only when it was added. A Module of a Track is placed by that Track's Rack, read again, since another Track may be selected now.
    fn restore(&mut self, restore: &Restore) {
        let named = restore.track.is_some() || restore.bus.is_some() || restore.master;
        let rack = if named {
            self.payload(&Request::Get {
                param: None,
                track: restore.track.clone(),
                bus: restore.bus.clone(),
                master: restore.master,
            })
        } else {
            self.snapshot.params.clone()
        };
        let present: Vec<String> = rack
            .as_ref()
            .map_or_default(param_rows)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let Some(added) = self.send(&restore.add_request(&present)) else {
            return;
        };
        let Some(id) = added["id"].as_str() else {
            return;
        };
        for req in restore.follow_up_requests(id) {
            self.send(&req);
        }
    }

    /// The Rack that Rack requests act on while a Project is open: the Bus or Master Rack the mixer panel shows, else the selected Track's.
    fn rack_target(&self) -> Option<MixerTarget> {
        let status = self.snapshot.status.as_ref()?;
        project_name(Some(status))?;
        match self.shown_rack.and_then(|shown| shown.column(status)) {
            Some(target @ (MixerTarget::Bus(_) | MixerTarget::Master)) => Some(target),
            _ => usize::try_from(status["selected"].as_u64()?)
                .ok()
                .map(MixerTarget::Track),
        }
    }

    /// Mirrors the Rack the window names by the last [`Command::ShowRack`] among `queued`; a Project opened or closed names none. `true` when it changed, so that its Rack is read now.
    fn follow_shown_rack(&mut self, queued: &[Command]) -> bool {
        let before = self.shown_rack;
        for command in queued {
            match command {
                Command::ShowRack(target) => self.shown_rack = Some(*target),
                Command::ProjectNew(_)
                | Command::ProjectOpen(_)
                | Command::ProjectClose
                | Command::RestoreAutosave(_) => self.shown_rack = None,
                _ => {}
            }
        }
        self.shown_rack != before
    }

    /// The `get` of the Rack shown: the Bus or Master Rack the mixer panel shows, else the Daemon's own choice (the selected Track's).
    fn rack_get(&self) -> Request {
        let shown = self
            .rack_target()
            .filter(|target| !matches!(target, MixerTarget::Track(_)));
        let (track, bus, master) = rack_selectors(shown);
        Request::Get {
            param: None,
            track,
            bus,
            master,
        }
    }

    /// `project_open` waits for the closed Project's last Takes: it is sent again every tick while the Daemon answers that the Project is still closing.
    fn restore_autosave(&mut self, name: String) {
        self.send(&Request::ProjectClose);
        let open = Request::ProjectOpen {
            name,
            restore: true,
        };
        for _ in 0..REOPEN_TRIES {
            let response = self.request(&open);
            if !still_closing(&response) {
                if let Some(message) = response_message(&open, response) {
                    self.snapshot.last_message = Some((message, Instant::now()));
                }
                return;
            }
            thread::sleep(TICK);
        }
        warn!("project_open: still closing after {REOPEN_TRIES} tries");
        self.snapshot.last_message = Some((
            Message::Error {
                title: failure_title(&open),
                detail: STILL_CLOSING.to_string(),
            },
            Instant::now(),
        ));
    }

    fn poll(&mut self, now: Instant) {
        if self.connection.is_none() {
            self.connect(now);
        }
        if self.connection.is_none() {
            return;
        }
        match self.request(&Request::Status) {
            Ok(Response::Ok(status)) => self.on_status(status),
            Ok(Response::Err { error, .. }) => warn!("status: {error}"),
            Err(_) => {}
        }
        if self.connection.is_some() {
            self.poll_tuner();
        }
    }

    fn poll_tuner(&mut self) {
        self.snapshot.tuner = match self.request(&Request::Tuner) {
            Ok(Response::Ok(payload)) => {
                if !self.tuner_answered {
                    info!("tuner readings available");
                    self.tuner_answered = true;
                }
                Some(payload)
            }
            Ok(Response::Err { error, .. }) => {
                warn!("tuner: {error}");
                None
            }
            Err(_) => None,
        };
    }

    fn connect(&mut self, now: Instant) {
        let err = match Connection::connect(&self.config) {
            Ok(connection) => {
                self.connection = Some(connection);
                self.last_ignored_error = None;
                return;
            }
            Err(err) => err,
        };
        self.reap_daemon(now);
        if !counts_toward_spawn(&err) {
            let text = err.to_string();
            if self.last_ignored_error.as_ref() != Some(&text) {
                warn!("cannot connect: {text}");
                self.last_ignored_error = Some(text);
            }
            return;
        }
        self.failures += 1;
        if self.daemon.is_none() && spawn_policy(self.failures, self.last_spawn, now) {
            self.spawn_daemon(&err, now);
        }
    }

    fn on_status(&mut self, status: Value) {
        if self.snapshot.link != Link::Connected {
            info!("connected to {}", self.config.socket.display());
            self.snapshot.link = Link::Connected;
            if let Some(warning) = newer_protocol_warning(&status) {
                warn!("{warning}");
                self.snapshot.last_message = Some((Message::Notice(warning), Instant::now()));
            }
        }
        if !self.modules_fetched {
            self.fetch_modules();
            if self.connection.is_none() {
                return;
            }
        }
        if !self.irs_fetched {
            self.fetch_irs();
            if self.connection.is_none() {
                return;
            }
        }
        let revision = status["revision"].as_u64().unwrap_or(0);
        let changed = refresh_decision(self.revision, revision);
        if changed {
            self.ports_fetched = false;
            self.settings_fetched = false;
        }
        if !self.ports_fetched {
            self.snapshot.ports = self.fetch_once(&Request::Ports);
            self.ports_fetched = true;
        }
        if !self.settings_fetched {
            self.snapshot.settings = self.fetch_once(&Request::Settings);
            self.settings_fetched = true;
        }
        if !self.projects_fetched {
            self.snapshot.projects = self.fetch_once(&Request::ProjectList);
            self.projects_fetched = true;
        }
        if self.connection.is_none() {
            return;
        }
        self.failures = 0;
        self.snapshot.status = Some(status);
        if changed {
            self.refresh(revision);
        }
    }

    fn fetch_modules(&mut self) {
        match self.request(&Request::Modules) {
            Ok(Response::Ok(payload)) => {
                self.snapshot.module_types = module_types(&payload);
                self.modules_fetched = true;
                self.modules_warned = false;
            }
            Ok(Response::Err { error, .. }) => {
                if !self.modules_warned {
                    warn!("modules: {error}; retrying");
                    self.modules_warned = true;
                }
            }
            Err(_) => {}
        }
    }

    /// Fetched once whatever the answer, so that an unreadable IR directory shows one error, not one per tick.
    fn fetch_irs(&mut self) {
        let response = self.request(&Request::IrList);
        self.irs_fetched = true;
        match response {
            Ok(Response::Ok(payload)) => {
                self.snapshot.irs = ir_names(&payload);
                info!("ir_list: {} irs", self.snapshot.irs.len());
            }
            Ok(Response::Err { .. }) => {
                self.snapshot.irs.clear();
                if let Some(message) = response_message(&Request::IrList, response) {
                    self.snapshot.last_message = Some((message, Instant::now()));
                }
            }
            Err(_) => {}
        }
    }

    /// Fetched once whatever the answer, like `ir_list`; an error is logged, not shown.
    fn fetch_once(&mut self, req: &Request) -> Option<Value> {
        match self.request(req) {
            Ok(Response::Ok(payload)) => Some(payload),
            Ok(Response::Err { error, .. }) => {
                warn!("{req:?}: {error}");
                None
            }
            Err(_) => None,
        }
    }

    fn refresh(&mut self, revision: u64) {
        self.snapshot.bank = self.payload(&Request::List);
        if let Some(list) = &self.snapshot.bank {
            let (_, skipped) = bank_rows(list);
            if skipped > 0 {
                warn!("list: skipped {skipped} entries without an integer program");
            }
        }
        self.snapshot.params = self.payload(&self.rack_get());
        let project_open = project_name(self.snapshot.status.as_ref()).is_some();
        self.snapshot.items = if project_open {
            self.payload(&Request::Items)
        } else {
            None
        };
        let project = project_name(self.snapshot.status.as_ref()).map(str::to_string);
        if project != self.peaks_project {
            self.snapshot.unused_takes = None;
            self.snapshot.peaks.clear();
            self.peak_frames.clear();
            self.refused_peaks.clear();
            self.peaks_project = project;
        }
        self.fetch_peaks();
        if self.connection.is_none() {
            return;
        }
        if let Some(previous) = self.revision {
            info!("revision {previous} → {revision}, refreshed");
        }
        self.revision = Some(revision);
    }

    /// Fetches the peak table of each active Take of the Items not cached for its frames; a Take the Daemon refused, or whose reply names other frames, is not asked for again until its frames change, and one whose pages do not all arrive is asked for again at the next refresh.
    fn fetch_peaks(&mut self) {
        let items = self.snapshot.items.as_ref().map_or_default(item_blocks);
        for item in items {
            let wanted = (item.file, item.take_frames);
            if self.peak_frames.get(&wanted.0) == Some(&wanted.1)
                || self.refused_peaks.contains(&wanted)
            {
                continue;
            }
            match self.fetch_peak_table(&wanted.0) {
                Ok(Some((frames, table))) if frames == wanted.1 => {
                    self.snapshot.peaks.insert(wanted.0.clone(), table.into());
                    self.peak_frames.insert(wanted.0, frames);
                }
                Ok(Some(_)) | Err(()) => {
                    self.snapshot.peaks.remove(&wanted.0);
                    self.peak_frames.remove(&wanted.0);
                    self.refused_peaks.insert(wanted);
                }
                Ok(None) => {}
            }
        }
    }

    /// The Take `file`'s frames and peak table, `Err` when the Daemon refuses it, `None` when a page does not arrive whole.
    fn fetch_peak_table(&mut self, file: &str) -> Result<Option<(u64, Vec<i8>)>, ()> {
        let mut table = Vec::new();
        loop {
            let request = Request::TakePeaks {
                file: file.to_string(),
                first: Some(table.len() / 2),
                count: None,
            };
            let page = match self.request(&request) {
                Ok(Response::Ok(page)) => page,
                Ok(Response::Err { error, .. }) => {
                    warn!("{request:?}: {error}");
                    return Err(());
                }
                Err(_) => return Ok(None),
            };
            let Some(peaks) = page["peaks"].as_array() else {
                return Ok(None);
            };
            table.extend(
                peaks
                    .iter()
                    .filter_map(Value::as_i64)
                    .filter_map(|peak| i8::try_from(peak).ok()),
            );
            let (Some(frames), Some(total)) = (page["frames"].as_u64(), page["total"].as_u64())
            else {
                return Ok(None);
            };
            if peaks.is_empty() || table.len() as u64 / 2 >= total {
                return Ok(Some((frames, table)));
            }
        }
    }

    fn payload(&mut self, req: &Request) -> Option<Value> {
        match self.request(req) {
            Ok(Response::Ok(payload)) => Some(payload),
            Ok(Response::Err { .. }) | Err(_) => None,
        }
    }

    fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        let connection = self
            .connection
            .as_mut()
            .ok_or_else(|| ClientError::NotRunning(self.config.socket.clone()))?;
        let response = connection.request(req);
        if let Err(err) = &response {
            self.disconnect(err);
        }
        response
    }

    fn disconnect(&mut self, err: &ClientError) {
        warn!("disconnected: {err}");
        self.connection = None;
        self.revision = None;
        self.modules_fetched = false;
        self.irs_fetched = false;
        self.ports_fetched = false;
        self.settings_fetched = false;
        self.projects_fetched = false;
        self.snapshot.status = None;
        self.snapshot.tuner = None;
        self.snapshot.link = Link::Disconnected {
            since: Instant::now(),
        };
    }

    fn spawn_daemon(&mut self, reason: &ClientError, now: Instant) {
        self.last_spawn = Some(now);
        let executable = daemon_executable();
        let mut process = Process::new(&executable);
        if let Some(path) = &self.config_path {
            process.arg("--config").arg(path);
        }
        process
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        match process.spawn() {
            Ok(child) => {
                info!(
                    "starting daemon {} (pid {}): {reason}",
                    executable.display(),
                    child.id()
                );
                self.daemon = Some(child);
                self.snapshot.link = Link::Starting { since: now };
            }
            Err(err) => warn!("cannot start daemon {}: {err}", executable.display()),
        }
    }

    fn reap_daemon(&mut self, now: Instant) {
        let Some(child) = &mut self.daemon else {
            return;
        };
        match child.try_wait() {
            Ok(None) => return,
            Ok(Some(status)) => warn!("daemon exited: {status}"),
            Err(err) => warn!("daemon state unknown: {err}"),
        }
        self.daemon = None;
        if matches!(self.snapshot.link, Link::Starting { .. }) {
            self.snapshot.link = Link::Disconnected { since: now };
        }
    }

    /// SIGTERM lets the Daemon disconnect from JACK and remove its socket; SIGKILL only after the timeout.
    fn stop_daemon(&mut self) {
        let Some(mut child) = self.daemon.take() else {
            return;
        };
        info!("stopping daemon (pid {})", child.id());
        // SAFETY: the pid is our own child, not yet reaped, so it cannot have been reused.
        if unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) } == -1 {
            warn!("SIGTERM to daemon failed: {}", io::Error::last_os_error());
        }
        let deadline = Instant::now() + DAEMON_STOP_TIMEOUT;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(status)) => {
                    info!("daemon stopped: {status}");
                    return;
                }
                Ok(None) => thread::sleep(TICK),
                Err(err) => {
                    warn!("daemon state unknown: {err}");
                    break;
                }
            }
        }
        warn!("daemon did not stop; killing it");
        if let Err(err) = child.kill().and_then(|()| child.wait().map(drop)) {
            warn!("cannot kill daemon: {err}");
        }
    }
}

fn log_mute(payload: &Value) {
    if payload["muted"].as_bool() == Some(true) {
        info!("muted");
    } else {
        info!("unmuted");
    }
}

fn log_rack_ir(response: &Result<Response, ClientError>) {
    match response {
        Ok(Response::Ok(payload)) => info!(
            "rack_ir: {} ir {}",
            payload["id"].as_str().unwrap_or_default(),
            payload["ir"].as_str().unwrap_or_default()
        ),
        Ok(Response::Err { error, .. }) => info!("rack_ir: error: {error}"),
        Err(err) => info!("rack_ir: error: {err}"),
    }
}

fn daemon_executable() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(DAEMON_EXECUTABLE)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(DAEMON_EXECUTABLE))
}
