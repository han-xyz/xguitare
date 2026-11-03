use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::Deserialize;

const CONFIG_FILE: &str = "config.toml";
const SOCKET_FILE: &str = "xguitare.sock";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub bank_dir: PathBuf,
    pub buffer_size: u32,
    pub crossfade_ms: f32,
    pub ir_max_ms: f32,
    pub socket: PathBuf,
    pub ports: Ports,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ports {
    pub input: String,
    pub output_left: String,
    pub output_right: String,
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    bank_dir: Option<PathBuf>,
    buffer_size: Option<u32>,
    crossfade_ms: Option<f32>,
    ir_max_ms: Option<f32>,
    socket: Option<PathBuf>,
    ports: Option<PortsFile>,
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
        let path = path.unwrap_or(&default_path);
        let defaults = Config::default_for(&dirs)?;
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
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
        Ok(defaults.overridden_by(file))
    }

    fn default_for(dirs: &ProjectDirs) -> Result<Config, ConfigError> {
        let socket_dir = match dirs.runtime_dir() {
            Some(dir) => dir.to_path_buf(),
            None => fallback_runtime_dir()?,
        };
        Ok(Config {
            bank_dir: dirs.config_dir().join("bank"),
            buffer_size: 128,
            crossfade_ms: 20.0,
            ir_max_ms: 100.0,
            socket: socket_dir.join(SOCKET_FILE),
            ports: Ports {
                input: "system:capture_1".to_string(),
                output_left: "system:playback_1".to_string(),
                output_right: "system:playback_2".to_string(),
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
            buffer_size: file.buffer_size.unwrap_or(self.buffer_size),
            crossfade_ms: file.crossfade_ms.unwrap_or(self.crossfade_ms),
            ir_max_ms: file.ir_max_ms.unwrap_or(self.ir_max_ms),
            socket: file.socket.unwrap_or(self.socket),
            ports,
        }
    }
}

fn fallback_runtime_dir() -> Result<PathBuf, ConfigError> {
    let uid = fs::metadata("/proc/self").map_err(ConfigError::Uid)?.uid();
    Ok(PathBuf::from(format!("/tmp/xguitare-{uid}")))
}

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
