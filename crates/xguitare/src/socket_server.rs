use std::fmt;
use std::fs::{self, DirBuilder};
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tracing::warn;
use xguitare_core::control::{EngineState, handle_line};
use xguitare_core::{SocketDirError, secure_socket_dir};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_LINE: usize = 64 * 1024;
const TOO_LONG: &str = r#"{"ok":false,"error":"request too long"}"#;
const SOCKET_DIR_MODE: u32 = 0o700;

#[derive(Debug)]
pub enum SocketError {
    AlreadyRunning(PathBuf),
    Bind(PathBuf, io::Error),
    Spawn(io::Error),
}

/// The bound control socket; dropping it removes the socket file.
pub struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
}

/// Creates the `/tmp` fallback directory of `socket` if missing and refuses it unless it is ours and private (see `secure_socket_dir`).
pub fn secure_fallback_dir(socket: &Path) -> Result<(), SocketDirError> {
    let dir = socket.parent().unwrap_or(Path::new("/"));
    create_socket_dir(dir).map_err(|err| SocketDirError {
        dir: dir.to_path_buf(),
        reason: err.to_string(),
    })?;
    secure_socket_dir(dir)
}

/// Binds the Control API socket at `path`, refusing when a Daemon already answers there and replacing a stale file.
pub fn bind(path: &Path) -> Result<ControlSocket, SocketError> {
    let bind_error = |err| SocketError::Bind(path.to_path_buf(), err);
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(SocketError::AlreadyRunning(path.to_path_buf()));
        }
        fs::remove_file(path).map_err(bind_error)?;
    }
    if let Some(dir) = path.parent() {
        create_socket_dir(dir).map_err(bind_error)?;
    }
    let listener = UnixListener::bind(path).map_err(bind_error)?;
    listener.set_nonblocking(true).map_err(bind_error)?;
    Ok(ControlSocket {
        listener,
        path: path.to_path_buf(),
    })
}

impl ControlSocket {
    /// Accepts connections on a thread named `control` until `stop` is set, then removes the socket file; each connection gets its own thread `control-<n>`.
    pub fn spawn(
        self,
        state: Arc<Mutex<EngineState>>,
        stop: Arc<AtomicBool>,
    ) -> Result<JoinHandle<()>, SocketError> {
        thread::Builder::new()
            .name("control".to_string())
            .spawn(move || self.accept_until_stopped(&state, &stop))
            .map_err(SocketError::Spawn)
    }

    fn accept_until_stopped(self, state: &Arc<Mutex<EngineState>>, stop: &AtomicBool) {
        let mut accepted: u64 = 0;
        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    accepted += 1;
                    let state = Arc::clone(state);
                    // Connection threads are never joined: each ends within CONNECTION_TIMEOUT of silence or when its client hangs up, and process exit ends the rest.
                    let spawned = thread::Builder::new()
                        .name(format!("control-{accepted}"))
                        .spawn(move || serve_connection(&stream, &state));
                    if let Err(err) = spawned {
                        warn!("cannot start control connection thread: {err}");
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => thread::sleep(POLL_INTERVAL),
                Err(err) => {
                    warn!("control socket: {err}");
                    thread::sleep(POLL_INTERVAL);
                }
            }
        }
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path) {
            warn!("cannot remove {}: {err}", self.path.display());
        }
    }
}

fn create_socket_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(SOCKET_DIR_MODE)
        .create(dir)
}

fn serve_connection(stream: &UnixStream, state: &Mutex<EngineState>) {
    match answer_lines(stream, state) {
        Err(err) if !matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            warn!("control connection: {err}");
        }
        _ => {}
    }
}

fn answer_lines(stream: &UnixStream, state: &Mutex<EngineState>) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(CONNECTION_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;
    let mut reader = BufReader::new(stream);
    let mut writer = stream;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        if line.len() > MAX_LINE && !line.ends_with(b"\n") {
            return writer.write_all(format!("{TOO_LONG}\n").as_bytes());
        }
        let request = String::from_utf8_lossy(&line);
        let reply = handle_line(
            &mut state.lock().unwrap_or_else(PoisonError::into_inner),
            request.trim_end(),
        );
        writer.write_all(format!("{reply}\n").as_bytes())?;
    }
}

impl fmt::Display for SocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SocketError::AlreadyRunning(path) => {
                write!(f, "daemon already running at {}", path.display())
            }
            SocketError::Bind(path, err) => {
                write!(f, "cannot bind control socket {}: {err}", path.display())
            }
            SocketError::Spawn(err) => write!(f, "cannot start control thread: {err}"),
        }
    }
}

impl std::error::Error for SocketError {}
