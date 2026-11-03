use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Deserializer};

use crate::engine::SWITCHER_MAX_FRAMES;

const CONFIG_FILE: &str = "config.toml";
const SOCKET_FILE: &str = "xguitare.sock";
const SOCKET_DIR_MODE: u32 = 0o700;
pub const AUTO_PORT: &str = "auto";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub bank_dir: PathBuf,
    pub bank_kind: BankKind,
    pub buffer_size: u32,
    pub crossfade_ms: f32,
    pub ir_max_ms: f32,
    pub socket: PathBuf,
    /// True when `$XDG_RUNTIME_DIR` is unset and `socket` is the `/tmp/xguitare-<uid>` fallback.
    pub socket_is_fallback: bool,
    pub ports: Ports,
}

/// JACK ports the Daemon connects to `in`, `out_l` and `out_r`. Each value is one of:
/// `"auto"`: among physical ports only, the only device's first port in JACK's channel order (`output_right`: its second); several devices connect nothing and warn with their port names.
/// `""`: do not connect.
/// A pattern: an exact port name, or a glob where `*` matches any run of characters and the rest must match the whole name (e.g. `"Scarlett*capture_FL"`; a leading or trailing `*` for a suffix or prefix), matched against every audio port of that direction, physical or not (PipeWire split nodes, other apps), except the Daemon's own; the first match in sorted order wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ports {
    pub input: String,
    pub output_left: String,
    pub output_right: String,
}

/// Which starter Bank the first run seeds into a missing `bank_dir`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BankKind {
    #[default]
    Guitar,
    Bass,
}

#[derive(Debug)]
pub enum ConfigError {
    NoHomeDir,
    Uid(std::io::Error),
    Syntax {
        path: PathBuf,
        line: usize,
        col: usize,
        msg: String,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    BufferSize {
        path: PathBuf,
    },
    NotFound {
        path: PathBuf,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    bank_dir: Option<PathBuf>,
    #[serde(default, deserialize_with = "bank_kind")]
    bank_kind: Option<BankKind>,
    buffer_size: Option<u32>,
    crossfade_ms: Option<f32>,
    ir_max_ms: Option<f32>,
    socket: Option<PathBuf>,
    ports: Option<PortsFile>,
}

fn bank_kind<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<BankKind>, D::Error> {
    BankKind::deserialize(deserializer)
        .map(Some)
        .map_err(|_| serde::de::Error::custom("bank_kind must be guitar or bass"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortsFile {
    input: Option<String>,
    output_left: Option<String>,
    output_right: Option<String>,
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
        let dirs = ProjectDirs::from("", "", "xguitare").ok_or(ConfigError::NoHomeDir)?;
        let default_path = dirs.config_dir().join(CONFIG_FILE);
        let explicit = path.is_some();
        let path = path.unwrap_or(&default_path);
        let defaults = Config::default_for(&dirs)?;
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound && explicit => {
                return Err(ConfigError::NotFound {
                    path: path.to_path_buf(),
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(defaults),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let file: ConfigFile = toml::from_str(&text).map_err(|error| {
            let (line, col) = crate::toml_position(&text, &error);
            ConfigError::Syntax {
                path: path.to_path_buf(),
                line,
                col,
                msg: error.message().to_string(),
            }
        })?;
        if file
            .buffer_size
            .is_some_and(|size| size == 0 || size as usize > SWITCHER_MAX_FRAMES)
        {
            return Err(ConfigError::BufferSize {
                path: path.to_path_buf(),
            });
        }
        Ok(defaults.overridden_by(file))
    }

    fn default_for(dirs: &ProjectDirs) -> Result<Config, ConfigError> {
        let (socket_dir, socket_is_fallback) = match dirs.runtime_dir() {
            Some(dir) => (dir.to_path_buf(), false),
            None => (fallback_runtime_dir()?, true),
        };
        Ok(Config {
            bank_dir: dirs.config_dir().join("bank"),
            bank_kind: BankKind::default(),
            buffer_size: 128,
            crossfade_ms: 20.0,
            ir_max_ms: 100.0,
            socket: socket_dir.join(SOCKET_FILE),
            socket_is_fallback,
            ports: Ports {
                input: AUTO_PORT.to_string(),
                output_left: AUTO_PORT.to_string(),
                output_right: AUTO_PORT.to_string(),
            },
        })
    }

    fn overridden_by(self, file: ConfigFile) -> Config {
        let ports = match file.ports {
            Some(ports) => Ports {
                input: ports.input.unwrap_or(self.ports.input),
                output_left: ports.output_left.unwrap_or(self.ports.output_left),
                output_right: ports.output_right.unwrap_or(self.ports.output_right),
            },
            None => self.ports,
        };
        Config {
            bank_dir: file.bank_dir.unwrap_or(self.bank_dir),
            bank_kind: file.bank_kind.unwrap_or(self.bank_kind),
            buffer_size: file.buffer_size.unwrap_or(self.buffer_size),
            crossfade_ms: file.crossfade_ms.unwrap_or(self.crossfade_ms),
            ir_max_ms: file.ir_max_ms.unwrap_or(self.ir_max_ms),
            socket_is_fallback: self.socket_is_fallback && file.socket.is_none(),
            socket: file.socket.unwrap_or(self.socket),
            ports,
        }
    }
}

#[derive(Debug)]
pub struct SocketDirError {
    pub dir: PathBuf,
    pub reason: String,
}

/// Checks that `dir`, the `/tmp` fallback socket directory, is a real directory owned by this user with mode 0700; a wrong mode on our own directory is tightened rather than refused.
pub fn secure_socket_dir(dir: &Path) -> Result<(), SocketDirError> {
    let refuse = |reason: String| SocketDirError {
        dir: dir.to_path_buf(),
        reason,
    };
    let meta = fs::symlink_metadata(dir).map_err(|err| refuse(err.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(refuse("it is a symlink".to_string()));
    }
    if !meta.is_dir() {
        return Err(refuse("not a directory".to_string()));
    }
    let uid = current_uid().map_err(|err| refuse(err.to_string()))?;
    if meta.uid() != uid {
        return Err(refuse(format!("owned by uid {}, not {uid}", meta.uid())));
    }
    if meta.mode() & 0o777 != SOCKET_DIR_MODE {
        fs::set_permissions(dir, fs::Permissions::from_mode(SOCKET_DIR_MODE))
            .map_err(|err| refuse(err.to_string()))?;
    }
    Ok(())
}

fn current_uid() -> std::io::Result<u32> {
    Ok(fs::metadata("/proc/self")?.uid())
}

fn fallback_runtime_dir() -> Result<PathBuf, ConfigError> {
    let uid = current_uid().map_err(ConfigError::Uid)?;
    Ok(PathBuf::from(format!("/tmp/xguitare-{uid}")))
}

impl fmt::Display for BankKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BankKind::Guitar => "guitar",
            BankKind::Bass => "bass",
        })
    }
}

impl fmt::Display for SocketDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "refusing socket directory {}: {}",
            self.dir.display(),
            self.reason
        )
    }
}

impl std::error::Error for SocketDirError {}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NoHomeDir => {
                f.write_str("no home directory to derive the default config paths from")
            }
            ConfigError::Uid(error) => {
                write!(f, "cannot determine uid for the socket fallback: {error}")
            }
            ConfigError::Syntax {
                path,
                line,
                col,
                msg,
            } => write!(f, "{}: line {line}:{col}: {msg}", path.display()),
            ConfigError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ConfigError::BufferSize { path } => write!(
                f,
                "{}: buffer_size must be between 1 and {SWITCHER_MAX_FRAMES}",
                path.display()
            ),
            ConfigError::NotFound { path } => write!(f, "{}: not found", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } | ConfigError::Uid(source) => Some(source),
            _ => None,
        }
    }
}
