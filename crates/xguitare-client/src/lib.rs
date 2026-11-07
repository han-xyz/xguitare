use std::fmt;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use xguitare_core::control::{Request, Response};
use xguitare_core::{Config, check_private_dir, current_uid};

const DAEMON_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(1);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub trait Transport {
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
    config: Config,
}

impl UnixSocketTransport {
    pub fn from_config(config: &Config) -> Self {
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
    pub fn connect(config: &Config) -> Result<Connection, ClientError> {
        let reader = connect(config, READ_TIMEOUT, WRITE_TIMEOUT)?;
        Ok(Connection {
            path: config.socket.clone(),
            reader: Some(reader),
        })
    }

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
    config: Config,
    connection: Option<Connection>,
}

impl Reconnecting {
    pub fn new(config: Config) -> Reconnecting {
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

/// Before each connect to the `/tmp` fallback socket: a missing directory means no Daemon, and an existing one must be ours and 0700.
fn check_fallback_dir(config: &Config) -> Result<(), ClientError> {
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
    check_private_dir(dir, uid).map_err(|err| ClientError::InsecureSocketDir(err.to_string()))
}

fn connect(
    config: &Config,
    read_timeout: Duration,
    write_timeout: Duration,
) -> Result<BufReader<UnixStream>, ClientError> {
    check_fallback_dir(config)?;
    let path = &config.socket;
    let stream = UnixStream::connect(path).map_err(|err| match err.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => {
            ClientError::NotRunning(path.to_path_buf())
        }
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
