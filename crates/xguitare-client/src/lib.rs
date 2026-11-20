use std::fmt;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use config::current_uid;
pub use config::{ClientConfig, ClientConfigError, check_private_dir};
use xguitare_protocol::{Request, Response, Value};

mod config;

const DAEMON_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(1);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// The encoded file names one `takes_clean` request carries at most: half the Daemon's 64 KiB request line, so a batch never reaches it; one name (at most 255 bytes, at most doubled by JSON escaping) always fits.
const TAKES_CLEAN_BATCH_BYTES: usize = 32 * 1024;

pub trait Transport {
    /// # Errors
    ///
    /// A [`ClientError`] when the request cannot reach the Daemon or its reply is no Response line.
    fn request(&mut self, req: &Request) -> Result<Response, ClientError>;
}

#[derive(Debug)]
pub enum ClientError {
    NotRunning(PathBuf),
    NotResponding(PathBuf),
    Io(io::Error),
    Protocol(String),
    InsecureSocketDir(String),
}

/// Connects to the Daemon's socket once per request: one request line out, one response line back.
pub struct UnixSocketTransport {
    config: ClientConfig,
}

impl UnixSocketTransport {
    pub fn from_config(config: &ClientConfig) -> Self {
        UnixSocketTransport {
            config: config.clone(),
        }
    }
}

impl Transport for UnixSocketTransport {
    fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        let mut reader = connect(&self.config, DAEMON_TIMEOUT, DAEMON_TIMEOUT)?;
        exchange(&mut reader, req, &self.config.socket)
    }
}

/// One persistent socket to the Daemon; after any error every request fails until the caller connects again.
pub struct Connection {
    path: PathBuf,
    reader: Option<BufReader<UnixStream>>,
}

impl Connection {
    /// # Errors
    ///
    /// [`ClientError::NotRunning`] when no Daemon listens at the config's socket, [`ClientError::InsecureSocketDir`] when its `/tmp` fallback directory is not ours and 0700, [`ClientError::Io`] for any other connect failure.
    pub fn connect(config: &ClientConfig) -> Result<Connection, ClientError> {
        let reader = connect(config, READ_TIMEOUT, WRITE_TIMEOUT)?;
        Ok(Connection {
            path: config.socket.clone(),
            reader: Some(reader),
        })
    }

    /// # Errors
    ///
    /// [`ClientError::Io`] with `NotConnected` after an earlier error; otherwise [`ClientError::NotResponding`] on a timeout, [`ClientError::Protocol`] for a closed connection or a line that is no Response, [`ClientError::Io`] for any other I/O failure.
    pub fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        let reader = self
            .reader
            .as_mut()
            .ok_or_else(|| ClientError::Io(ErrorKind::NotConnected.into()))?;
        let response = exchange(reader, req, &self.path);
        if response.is_err() {
            self.reader = None;
        }
        response
    }
}

impl Transport for Connection {
    fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        Connection::request(self, req)
    }
}

/// A `Connection` that reconnects on the request after an error; the failed request's error is returned unchanged and not retried.
pub struct Reconnecting {
    config: ClientConfig,
    connection: Option<Connection>,
}

impl Reconnecting {
    pub fn new(config: ClientConfig) -> Reconnecting {
        Reconnecting {
            config,
            connection: None,
        }
    }
}

impl Transport for Reconnecting {
    fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        let connection = match self.connection.as_mut() {
            Some(connection) => connection,
            None => self.connection.insert(Connection::connect(&self.config)?),
        };
        let response = connection.request(req);
        if response.is_err() {
            self.connection = None;
        }
        response
    }
}

/// A batched clean-up that an error stopped.
#[derive(Debug)]
pub struct TakesCleanStopped {
    /// `{deleted, bytes, failed}` summed over the batches before the error; `None` when the first batch failed.
    pub cleaned: Option<Value>,
    /// The error reply or [`ClientError`] text of the batch that failed.
    pub error: String,
}

/// Sends `takes_clean` for `files` through `request` in batches whose request line stays well under the Daemon's limit; replies `{deleted, bytes, failed}` summed over every batch.
///
/// # Errors
///
/// The first batch that fails stops the run: [`TakesCleanStopped`] with what the batches before it deleted.
pub fn clean_takes_in_batches(
    files: &[String],
    mut request: impl FnMut(&Request) -> Result<Response, ClientError>,
) -> Result<Value, TakesCleanStopped> {
    let mut cleaned: Option<(u64, u64, Vec<Value>)> = None;
    for batch in takes_clean_batches(files) {
        let req = Request::TakesClean { files: batch };
        let error = match request(&req) {
            Ok(Response::Ok(payload)) => {
                let (deleted, bytes, failed) = cleaned.get_or_insert_default();
                *deleted += payload["deleted"].as_u64().unwrap_or(0);
                *bytes += payload["bytes"].as_u64().unwrap_or(0);
                failed.extend(payload["failed"].as_array().into_iter().flatten().cloned());
                continue;
            }
            Ok(Response::Err { error }) => error,
            Err(err) => err.to_string(),
        };
        return Err(TakesCleanStopped {
            cleaned: cleaned.map(cleaned_payload),
            error,
        });
    }
    Ok(cleaned_payload(cleaned.unwrap_or_default()))
}

fn takes_clean_batches(files: &[String]) -> Vec<Vec<String>> {
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut batch_bytes = 0;
    for file in files {
        let bytes = Value::from(file.as_str()).to_string().len() + 1;
        match batches.last_mut() {
            Some(batch) if batch_bytes + bytes <= TAKES_CLEAN_BATCH_BYTES => {
                batch.push(file.clone());
                batch_bytes += bytes;
            }
            _ => {
                batches.push(vec![file.clone()]);
                batch_bytes = bytes;
            }
        }
    }
    batches
}

fn cleaned_payload((deleted, bytes, failed): (u64, u64, Vec<Value>)) -> Value {
    Value::Object(
        [
            ("deleted".to_string(), Value::from(deleted)),
            ("bytes".to_string(), Value::from(bytes)),
            ("failed".to_string(), Value::Array(failed)),
        ]
        .into_iter()
        .collect(),
    )
}

/// Before each connect to the `/tmp` fallback socket: a missing directory means no Daemon, and an existing one must be ours and 0700.
fn check_fallback_dir(config: &ClientConfig) -> Result<(), ClientError> {
    let Some(dir) = config.socket.parent().filter(|_| config.socket_is_fallback) else {
        return Ok(());
    };
    match dir.symlink_metadata() {
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(ClientError::NotRunning(config.socket.clone()));
        }
        _ => {}
    }
    let uid = current_uid().map_err(ClientError::Io)?;
    check_private_dir(dir, uid).map_err(ClientError::InsecureSocketDir)
}

fn connect(
    config: &ClientConfig,
    read_timeout: Duration,
    write_timeout: Duration,
) -> Result<BufReader<UnixStream>, ClientError> {
    check_fallback_dir(config)?;
    let path = &config.socket;
    let stream = UnixStream::connect(path).map_err(|err| match err.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => ClientError::NotRunning(path.clone()),
        _ => ClientError::Io(err),
    })?;
    stream
        .set_read_timeout(Some(read_timeout))
        .and_then(|()| stream.set_write_timeout(Some(write_timeout)))
        .map_err(ClientError::Io)?;
    Ok(BufReader::new(stream))
}

fn exchange(
    reader: &mut BufReader<UnixStream>,
    req: &Request,
    path: &Path,
) -> Result<Response, ClientError> {
    let reply = exchange_line(reader, req).map_err(|err| match err.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => {
            ClientError::NotResponding(path.to_path_buf())
        }
        _ => ClientError::Io(err),
    })?;
    if reply.is_empty() {
        return Err(ClientError::Protocol(
            "daemon closed the connection".to_string(),
        ));
    }
    Response::from_line(&reply).map_err(|err| ClientError::Protocol(err.to_string()))
}

fn exchange_line(reader: &mut BufReader<UnixStream>, req: &Request) -> io::Result<String> {
    let mut writer = reader.get_ref();
    writer.write_all(format!("{}\n", req.to_line()).as_bytes())?;
    let mut reply = String::new();
    reader.read_line(&mut reply)?;
    Ok(reply)
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::NotRunning(path) => {
                write!(f, "daemon not running (socket {})", path.display())
            }
            ClientError::NotResponding(path) => {
                write!(f, "daemon not responding (socket {})", path.display())
            }
            ClientError::Io(err) => write!(f, "control socket: {err}"),
            ClientError::Protocol(msg) => write!(f, "bad response from daemon: {msg}"),
            ClientError::InsecureSocketDir(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ClientError {}
