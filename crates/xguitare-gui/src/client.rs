use std::env;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::model::{
    Command, Link, Snapshot, bank_rows, counts_toward_spawn, failed_set, module_types,
    refresh_decision, requests_until_quit, response_text, spawn_policy,
};
use serde_json::Value;
use tracing::{info, warn};
use xguitare_client::{ClientError, Connection};
use xguitare_core::Config;
use xguitare_core::control::{Request, Response};

pub const TICK: Duration = Duration::from_millis(50);
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_EXECUTABLE: &str = "xguitare";

pub fn spawn(
    config: Config,
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

struct Client {
    config: Config,
    config_path: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    connection: Option<Connection>,
    last_ignored_error: Option<String>,
    failures: u32,
    last_spawn: Option<Instant>,
    daemon: Option<Child>,
    revision: Option<u64>,
    snapshot: Snapshot,
}

impl Client {
    fn new(config: Config, config_path: Option<PathBuf>, stop: Arc<AtomicBool>) -> Client {
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
            snapshot: Snapshot {
                link: Link::Disconnected {
                    since: Instant::now(),
                },
                status: None,
                bank: None,
                params: None,
                module_types: Vec::new(),
                last_message: None,
                failed_set: None,
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
        while !self.stop.load(Ordering::Relaxed) {
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
        let (requests, quit) = requests_until_quit(queued);
        for req in requests {
            let response = self.request(&req);
            if let Some(param) = failed_set(&req, &response) {
                self.snapshot.failed_set = Some((param, Instant::now()));
            }
            if let Some(text) = response_text(&req, response) {
                self.snapshot.last_message = Some((text, Instant::now()));
            }
        }
        !quit && !ui_gone
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
            self.snapshot.module_types = self
                .payload(&Request::Modules)
                .as_ref()
                .map(module_types)
                .unwrap_or_default();
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
        if let Some(list) = &self.snapshot.bank {
            let (_, skipped) = bank_rows(list);
            if skipped > 0 {
                warn!("list: skipped {skipped} entries without an integer program");
            }
        }
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
        // SAFETY: prctl is async-signal-safe and touches only the forked child. The signal fires when this (client) thread exits, which happens only after stop_daemon.
        unsafe {
            process.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
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

fn daemon_executable() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(DAEMON_EXECUTABLE)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(DAEMON_EXECUTABLE))
}
