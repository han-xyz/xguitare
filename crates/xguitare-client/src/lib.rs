use std::fmt;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use xguitare_core::control::{Request, Response};
use xguitare_core::{Config, secure_socket_dir};

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
    path: PathBuf,
}

impl UnixSocketTransport {
    /// Refuses an insecure `/tmp` fallback socket directory, as every client does before it connects.
    pub fn from_config(config: &Config) -> Result<Self, ClientError> {
        check_socket_dir(config)?;
        Ok(UnixSocketTransport {
            path: config.socket.clone(),
        })
    }
}

impl Transport for UnixSocketTransport {
    fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        let mut reader = connect(&self.path, DAEMON_TIMEOUT, DAEMON_TIMEOUT)?;
        exchange(&mut reader, req, &self.path)
    }
}

/// One persistent socket to the Daemon; after any error every request fails until the caller connects again.
pub struct Connection {
    path: PathBuf,
    reader: Option<BufReader<UnixStream>>,
}

impl Connection {
    /// Refuses an insecure `/tmp` fallback socket directory, then connects with a 1 s read and 5 s write timeout.
    pub fn connect(config: &Config) -> Result<Connection, ClientError> {
        check_socket_dir(config)?;
        let reader = connect(&config.socket, READ_TIMEOUT, WRITE_TIMEOUT)?;
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

fn check_socket_dir(config: &Config) -> Result<(), ClientError> {
    let fallback_dir = config
        .socket
        .parent()
        .filter(|dir| config.socket_is_fallback && dir.symlink_metadata().is_ok());
    match fallback_dir {
        Some(dir) => {
            secure_socket_dir(dir).map_err(|err| ClientError::InsecureSocketDir(err.to_string()))
        }
        None => Ok(()),
    }
}

fn connect(
    path: &Path,
    read_timeout: Duration,
    write_timeout: Duration,
) -> Result<BufReader<UnixStream>, ClientError> {
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
