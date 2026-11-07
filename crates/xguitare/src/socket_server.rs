use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::warn;
use xguitare_core::control::{
    ConnectionId, EngineState, close_connection, handle_line, open_connection,
};
use xguitare_core::{SOCKET_DIR_MODE, SocketDirError, secure_socket_dir};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const READ_TICK: Duration = Duration::from_secs(1);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_LINE: usize = 64 * 1024;
const TOO_LONG: &str = r#"{"ok":false,"error":"request too long"}"#;
const TOO_MANY: &str = r#"{"ok":false,"error":"too many connections"}"#;
const LOCK_FILE_MODE: u32 = 0o600;

/// The Daemon's cap on concurrent control connections.
pub const MAX_CONNECTIONS: usize = 32;
/// How long the Daemon keeps a control connection that sends nothing.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub enum SocketError {
    AnotherDaemon,
    Lock(PathBuf, io::Error),
    AlreadyRunning(PathBuf),
    NotASocket(PathBuf),
    Bind(PathBuf, io::Error),
    Spawn(io::Error),
}

/// The bound control socket; dropping it removes the socket file and releases the Daemon lock.
pub struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
    max_connections: usize,
    idle_timeout: Duration,
    _daemon_lock: File,
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

/// Binds the Control API socket at `path`, refusing when another Daemon holds `<path>.lock`, a Daemon already answers there or the path is not a socket, and replacing a stale socket.
/// Each connection past `max_connections` gets one `too many connections` error line and is closed; a connection that sends nothing for `idle_timeout` is closed.
pub fn bind(
    path: &Path,
    max_connections: usize,
    idle_timeout: Duration,
) -> Result<ControlSocket, SocketError> {
    let bind_error = |err| SocketError::Bind(path.to_path_buf(), err);
    if let Some(dir) = path.parent() {
        create_socket_dir(dir).map_err(bind_error)?;
    }
    let daemon_lock = lock_daemon(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_socket() => {
            return Err(SocketError::NotASocket(path.to_path_buf()));
        }
        Ok(_) => {
            if UnixStream::connect(path).is_ok() {
                return Err(SocketError::AlreadyRunning(path.to_path_buf()));
            }
            fs::remove_file(path).map_err(bind_error)?;
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(bind_error(err)),
    }
    let listener = UnixListener::bind(path).map_err(bind_error)?;
    listener.set_nonblocking(true).map_err(bind_error)?;
    Ok(ControlSocket {
        listener,
        path: path.to_path_buf(),
        max_connections,
        idle_timeout,
        _daemon_lock: daemon_lock,
    })
}

fn lock_daemon(socket: &Path) -> Result<File, SocketError> {
    let mut lock_path = socket.as_os_str().to_owned();
    lock_path.push(".lock");
    let lock_path = PathBuf::from(lock_path);
    let lock_error = |err| SocketError::Lock(lock_path.clone(), err);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(LOCK_FILE_MODE)
        .open(&lock_path)
        .map_err(lock_error)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(SocketError::AnotherDaemon),
        Err(TryLockError::Error(err)) => Err(lock_error(err)),
    }
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

    fn accept_until_stopped(self, state: &Arc<Mutex<EngineState>>, stop: &Arc<AtomicBool>) {
        let mut accepted: u64 = 0;
        let open = Arc::new(AtomicUsize::new(0));
        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if open.load(Ordering::Relaxed) >= self.max_connections {
                        refuse(&stream);
                        continue;
                    }
                    accepted += 1;
                    let slot = ConnectionSlot::take(&open);
                    let state = Arc::clone(state);
                    let stop = Arc::clone(stop);
                    let idle_timeout = self.idle_timeout;
                    // Connection threads are never joined: each sees `stop` within READ_TICK, and process exit ends any still writing.
                    let spawned = thread::Builder::new()
                        .name(format!("control-{accepted}"))
                        .spawn(move || {
                            let _slot = slot;
                            serve_connection(&stream, &state, &stop, idle_timeout);
                        });
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

/// One open control connection, counted while it lives.
struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    fn take(open: &Arc<AtomicUsize>) -> Self {
        open.fetch_add(1, Ordering::Relaxed);
        ConnectionSlot(Arc::clone(open))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn refuse(mut stream: &UnixStream) {
    if let Err(err) = stream.write_all(format!("{TOO_MANY}\n").as_bytes()) {
        warn!("cannot refuse control connection: {err}");
    }
}

fn create_socket_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(SOCKET_DIR_MODE)
        .create(dir)
}

fn serve_connection(
    stream: &UnixStream,
    state: &Mutex<EngineState>,
    stop: &AtomicBool,
    idle_timeout: Duration,
) {
    let connection = open_connection(&mut lock(state));
    match answer_lines(stream, state, connection, stop, idle_timeout) {
        Err(err) if !matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            warn!("control connection: {err}");
        }
        _ => {}
    }
    close_connection(&mut lock(state), connection);
}

fn lock(state: &Mutex<EngineState>) -> MutexGuard<'_, EngineState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn answer_lines(
    stream: &UnixStream,
    state: &Mutex<EngineState>,
    connection: ConnectionId,
    stop: &AtomicBool,
    idle_timeout: Duration,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(READ_TICK.min(idle_timeout)))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut reader = BufReader::new(stream);
    let mut writer = stream;
    let mut line = Vec::new();
    let mut last_read = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let limit = (MAX_LINE + 1 - line.len()) as u64;
        let read_before = line.len();
        match (&mut reader).take(limit).read_until(b'\n', &mut line) {
            Ok(_) => last_read = Instant::now(),
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if line.len() > read_before {
                    last_read = Instant::now();
                }
                if last_read.elapsed() >= idle_timeout {
                    return Ok(());
                }
                continue;
            }
            Err(err) => return Err(err),
        }
        let complete = line.ends_with(b"\n");
        if line.len() > MAX_LINE && !complete {
            return writer.write_all(format!("{TOO_LONG}\n").as_bytes());
        }
        if !line.is_empty() {
            let request = String::from_utf8_lossy(&line);
            let (reply, warnings) = handle_line(&mut lock(state), connection, request.trim_end());
            for warning in warnings {
                warn!("{warning}");
            }
            writer.write_all(format!("{reply}\n").as_bytes())?;
            line.clear();
        }
        if !complete {
            return Ok(());
        }
    }
    Ok(())
}

impl fmt::Display for SocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SocketError::AnotherDaemon => write!(f, "another Daemon is running"),
            SocketError::Lock(path, err) => write!(f, "cannot lock {}: {err}", path.display()),
            SocketError::AlreadyRunning(path) => {
                write!(f, "daemon already running at {}", path.display())
            }
            SocketError::NotASocket(path) => {
                write!(
                    f,
                    "socket path {} exists and is not a socket",
                    path.display()
                )
            }
            SocketError::Bind(path, err) => {
                write!(f, "cannot bind control socket {}: {err}", path.display())
            }
            SocketError::Spawn(err) => write!(f, "cannot start control thread: {err}"),
        }
    }
}

impl std::error::Error for SocketError {}
