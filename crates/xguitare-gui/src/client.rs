use std::env;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{info, warn};
use xguitare_client::{ClientError, Connection};
use xguitare_core::Config;
use xguitare_core::control::{Request, Response};
use xguitare_gui::model::{Command, Link, Snapshot, refresh_decision, spawn_policy};

pub const TICK: Duration = Duration::from_millis(50);
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_BINARY: &str = "xguitare";

/// Starts the client thread; it polls the Daemon every 50 ms until it receives `Command::Quit` or the sender is dropped, then stops a Daemon it started.
pub fn spawn(
    config: Config,
    config_path: Option<PathBuf>,
    ctx: egui::Context,
) -> (Sender<Command>, Receiver<Snapshot>, JoinHandle<()>) {
    let (command_tx, command_rx) = mpsc::channel();
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        Client::new(config, config_path).run(&command_rx, &snapshot_tx, &ctx);
    });
    (command_tx, snapshot_rx, handle)
}

struct Client {
    config: Config,
    config_path: Option<PathBuf>,
    connection: Option<Connection>,
    failures: u32,
    last_spawn: Option<Instant>,
    daemon: Option<Child>,
    revision: Option<u64>,
    snapshot: Snapshot,
}

impl Client {
    fn new(config: Config, config_path: Option<PathBuf>) -> Client {
        Client {
            config,
            config_path,
            connection: None,
            failures: 0,
            last_spawn: None,
            daemon: None,
            revision: None,
            snapshot: Snapshot {
                link: Link::Disconnected {
                    since: Instant::now(),
                },
                status: None,
                bank: None,
                params: None,
                last_message: None,
            },
        }
    }

    fn run(
        mut self,
        commands: &Receiver<Command>,
        snapshots: &Sender<Snapshot>,
        ctx: &egui::Context,
    ) {
        let mut sent: Option<Snapshot> = None;
        while self.handle_commands(commands) {
            self.poll(Instant::now());
            if sent.as_ref() != Some(&self.snapshot) {
                if snapshots.send(self.snapshot.clone()).is_err() {
                    break;
                }
                ctx.request_repaint();
                sent = Some(self.snapshot.clone());
            }
            thread::sleep(TICK);
        }
        self.stop_daemon();
    }

    /// Sends every queued command; false once `Quit` arrives or the UI is gone.
    fn handle_commands(&mut self, commands: &Receiver<Command>) -> bool {
        loop {
            let command = match commands.try_recv() {
                Ok(command) => command,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            };
            let Some(req) = request_for(command) else {
                return false;
            };
            let text = match self.request(&req) {
                Ok(Response::Ok(payload)) => payload.to_string(),
                Ok(Response::Err { error }) => format!("error: {error}"),
                Err(err) => format!("error: {err}"),
            };
            self.snapshot.last_message = Some((text, Instant::now()));
        }
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
            Ok(Response::Err { error }) => warn!("status: {error}"),
            Err(_) => {}
        }
    }

    fn connect(&mut self, now: Instant) {
        let err = match Connection::connect(&self.config) {
            Ok(connection) => {
                self.connection = Some(connection);
                return;
            }
            Err(err) => err,
        };
        self.failures += 1;
        self.reap_daemon();
        if self.daemon.is_none() && spawn_policy(self.failures, self.last_spawn, now) {
            self.spawn_daemon(&err, now);
        }
    }

    fn on_status(&mut self, status: Value) {
        if self.snapshot.link != Link::Connected {
            info!("connected to {}", self.config.socket.display());
            self.snapshot.link = Link::Connected;
        }
        self.failures = 0;
        let revision = status["revision"].as_u64().unwrap_or(0);
        self.snapshot.status = Some(status);
        if refresh_decision(self.revision, revision) {
            self.refresh(revision);
        }
    }

    fn refresh(&mut self, revision: u64) {
        self.snapshot.bank = self.payload(&Request::List);
        self.snapshot.params = self.payload(&Request::Get { param: None });
        if self.connection.is_none() {
            return;
        }
        if let Some(previous) = self.revision {
            info!("revision {previous} → {revision}, refreshed");
        }
        self.revision = Some(revision);
    }

    fn payload(&mut self, req: &Request) -> Option<Value> {
        match self.request(req) {
            Ok(Response::Ok(payload)) => Some(payload),
            Ok(Response::Err { .. }) | Err(_) => None,
        }
    }

    /// Any error drops the connection; the next tick reconnects.
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
        self.snapshot.status = None;
        self.snapshot.link = Link::Disconnected {
            since: Instant::now(),
        };
    }

    fn spawn_daemon(&mut self, reason: &ClientError, now: Instant) {
        self.last_spawn = Some(now);
        let program = daemon_program();
        let mut process = Process::new(&program);
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
                    program.display(),
                    child.id()
                );
                self.daemon = Some(child);
                self.snapshot.link = Link::Starting { since: now };
            }
            Err(err) => warn!("cannot start daemon {}: {err}", program.display()),
        }
    }

    fn reap_daemon(&mut self) {
        let Some(child) = &mut self.daemon else {
            return;
        };
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                warn!("daemon exited: {status}");
                self.daemon = None;
            }
            Err(err) => {
                warn!("daemon state unknown: {err}");
                self.daemon = None;
            }
        }
    }

    /// SIGTERM lets the Daemon disconnect from JACK and remove its socket; SIGKILL only after the timeout.
    fn stop_daemon(&mut self) {
        let Some(mut child) = self.daemon.take() else {
            return;
        };
        info!("stopping daemon (pid {})", child.id());
        let signalled = Process::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        if !matches!(signalled, Ok(status) if status.success()) {
            warn!("SIGTERM to daemon failed: {signalled:?}");
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

fn request_for(command: Command) -> Option<Request> {
    match command {
        Command::Load(preset) => Some(Request::Load { preset }),
        Command::Set { param, value } => Some(Request::Set { param, value }),
        Command::Save => Some(Request::Save),
        Command::Quit => None,
    }
}

/// The `xguitare` binary beside this one (a cargo build or an install prefix), else the one on `PATH`.
fn daemon_program() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(DAEMON_BINARY)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(DAEMON_BINARY))
}
