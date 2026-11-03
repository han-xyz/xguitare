use std::fmt;
use std::fs::{self, DirBuilder};
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tracing::warn;
use xguitare_core::control::{EngineState, handle_line};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
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
        DirBuilder::new()
            .recursive(true)
            .mode(SOCKET_DIR_MODE)
            .create(dir)
            .map_err(bind_error)?;
    }
    let listener = UnixListener::bind(path).map_err(bind_error)?;
    listener.set_nonblocking(true).map_err(bind_error)?;
    Ok(ControlSocket {
        listener,
        path: path.to_path_buf(),
    })
}

impl ControlSocket {
    /// Serves connections one at a time on a thread named `control` until `stop` is set, then removes the socket file.
    pub fn spawn(
        self,
        state: Arc<Mutex<EngineState>>,
        stop: Arc<AtomicBool>,
    ) -> Result<JoinHandle<()>, SocketError> {
        thread::Builder::new()
            .name("control".to_string())
            .spawn(move || self.serve(&state, &stop))
            .map_err(SocketError::Spawn)
    }

    fn serve(self, state: &Mutex<EngineState>, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Err(err) = answer_lines(&stream, state, stop) {
                        warn!("control connection: {err}");
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

fn answer_lines(
    stream: &UnixStream,
    state: &Mutex<EngineState>,
    stop: &AtomicBool,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(POLL_INTERVAL))?;
    let mut reader = BufReader::new(stream);
    let mut writer = stream;
    let mut line = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        match reader.read_until(b'\n', &mut line) {
            Ok(0) if line.is_empty() => return Ok(()),
            Ok(_) => {
                let request = String::from_utf8_lossy(&line);
                let reply = handle_line(
                    &mut state.lock().unwrap_or_else(PoisonError::into_inner),
                    request.trim_end(),
                );
                writer.write_all(format!("{reply}\n").as_bytes())?;
                line.clear();
            }
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
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
