use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Deserializer, Serialize};

use crate::engine::SWITCHER_MAX_FRAMES;
use crate::state::{SavedState, read_state};
use xguitare_protocol::BankKind;
use xguitare_protocol::default_socket;

const CONFIG_FILE: &str = "config.toml";
const STATE_FILE: &str = "state.toml";
const BANK_DIR: &str = "bank";
const PROJECTS_DIR: &str = "Music/xguitare";
pub const SOCKET_DIR_MODE: u32 = 0o700;
const TUNER_REFERENCE_HZ: std::ops::RangeInclusive<f32> = 400.0..=480.0;
const CROSSFADE_MS_MAX: f32 = 1000.0;
const IR_MAX_MS_MAX: f32 = 10000.0;
pub const AUTO_PORT: &str = "auto";

#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent config flags, not states of one machine"
)]
pub struct Config {
    pub bank_dir: PathBuf,
    pub bank_kind: BankKind,
    pub buffer_size: u32,
    pub crossfade_ms: f32,
    pub ir_max_ms: f32,
    /// The IR directory: where a Preset's Cabinet IR names are looked up first, before `bank_dir`.
    pub ir_dir: PathBuf,
    pub socket: PathBuf,
    /// True when `$XDG_RUNTIME_DIR` is unset and `socket` is the `/tmp/xguitare-<uid>` fallback.
    pub socket_is_fallback: bool,
    pub ports: Ports,
    /// The frequency of A4 the Tuner names notes against.
    pub tuner_reference_hz: f32,
    /// Whether a client's tuner mode turns on Mute while it is open.
    pub tuner_mute: bool,
    /// The Daemon-owned `state.toml` beside the config file.
    pub state_file: PathBuf,
    /// What `state_file` held when loaded, already applied to the fields above; empty when it was missing or ignored.
    pub state: SavedState,
    /// Why `state_file` was ignored: its path and the read or parse error.
    pub state_warning: Option<String>,
    /// Whether the config file names `bank_dir`, which then wins over `bank_kind`'s directory.
    pub bank_dir_in_config: bool,
    /// Whether the config file names `ir_dir`; without it and without a saved one, `ir_dir` is the Bank directory.
    pub ir_dir_in_config: bool,
    /// The folder that holds one sub-folder per Project: `projects_dir` from `state.toml`, else `~/Music/xguitare`.
    pub projects_dir: PathBuf,
}

/// JACK ports the Daemon connects to `in`, `out_l` and `out_r`. Each value is one of:
/// `"auto"`: among physical ports only, the only device's first port in JACK's channel order (`output_right`: its second); several devices connect nothing and warn with their port names.
/// `""`: do not connect.
/// A pattern: an exact port name, or a glob where `*` matches any run of characters and the rest must match the whole name (e.g. `"Scarlett*capture_FL"`; a leading or trailing `*` for a suffix or prefix), matched against every audio port of that direction, physical or not (PipeWire split nodes, other apps), except the Daemon's own; the first match in sorted order wins.
#[allow(clippy::doc_markdown, reason = "PipeWire is a product name, not code")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    BufferSize {
        path: PathBuf,
    },
    NotFound {
        path: PathBuf,
    },
    TunerReference {
        path: PathBuf,
    },
    CrossfadeMs {
        path: PathBuf,
    },
    IrMaxMs {
        path: PathBuf,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    bank_dir: Option<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_bank_kind")]
    bank_kind: Option<BankKind>,
    buffer_size: Option<u32>,
    crossfade_ms: Option<f32>,
    ir_max_ms: Option<f32>,
    ir_dir: Option<PathBuf>,
    socket: Option<PathBuf>,
    ports: Option<PortsFile>,
    tuner_reference_hz: Option<f32>,
    tuner_mute: Option<bool>,
}

fn deserialize_bank_kind<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<BankKind>, D::Error> {
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
    /// # Errors
    ///
    /// A [`ConfigError`] when there is no home directory, the uid cannot be read, an explicit `path` is missing, the file cannot be read or parsed, or a value is out of range.
    pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
        let dirs = ProjectDirs::from("", "", "xguitare").ok_or(ConfigError::NoHomeDir)?;
        let default_path = dirs.config_dir().join(CONFIG_FILE);
        let explicit = path.is_some();
        let path = path.unwrap_or(&default_path);
        let config_dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let defaults = Config::default_for(&dirs, &config_dir)?;
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound && explicit => {
                return Err(ConfigError::NotFound {
                    path: path.to_path_buf(),
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
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
        if file
            .tuner_reference_hz
            .is_some_and(|hz| !TUNER_REFERENCE_HZ.contains(&hz))
        {
            return Err(ConfigError::TunerReference {
                path: path.to_path_buf(),
            });
        }
        if file
            .crossfade_ms
            .is_some_and(|ms| !is_above_zero_at_most(ms, CROSSFADE_MS_MAX))
        {
            return Err(ConfigError::CrossfadeMs {
                path: path.to_path_buf(),
            });
        }
        if file
            .ir_max_ms
            .is_some_and(|ms| !is_above_zero_at_most(ms, IR_MAX_MS_MAX))
        {
            return Err(ConfigError::IrMaxMs {
                path: path.to_path_buf(),
            });
        }
        let (state, state_warning) = read_state(&defaults.state_file);
        Ok(Config {
            state_warning,
            ..defaults.overridden_by(file, state)
        })
    }

    /// The Bank directory a switch to `kind` uses when the config file names no `bank_dir`: the one `[banks]` recorded, else `<config dir>/bank-<kind>`.
    pub(crate) fn kind_bank_dir(&self, kind: BankKind) -> PathBuf {
        self.state.banks.get(kind).cloned().unwrap_or_else(|| {
            let config_dir = self.state_file.parent().unwrap_or(Path::new(""));
            config_dir.join(format!("{BANK_DIR}-{kind}"))
        })
    }

    fn default_for(dirs: &ProjectDirs, config_dir: &Path) -> Result<Config, ConfigError> {
        let runtime_dir = dirs.runtime_dir();
        let socket = default_socket(runtime_dir, current_uid).map_err(ConfigError::Uid)?;
        let bank_dir = config_dir.join(BANK_DIR);
        let home = BaseDirs::new().ok_or(ConfigError::NoHomeDir)?;
        Ok(Config {
            ir_dir: bank_dir.clone(),
            bank_dir,
            bank_kind: BankKind::default(),
            buffer_size: 128,
            crossfade_ms: 20.0,
            ir_max_ms: 100.0,
            socket,
            socket_is_fallback: runtime_dir.is_none(),
            ports: Ports {
                input: AUTO_PORT.to_string(),
                output_left: AUTO_PORT.to_string(),
                output_right: AUTO_PORT.to_string(),
            },
            tuner_reference_hz: 440.0,
            tuner_mute: true,
            state_file: config_dir.join(STATE_FILE),
            state: SavedState::default(),
            state_warning: None,
            bank_dir_in_config: false,
            ir_dir_in_config: false,
            projects_dir: home.home_dir().join(PROJECTS_DIR),
        })
    }

    fn overridden_by(self, file: ConfigFile, state: SavedState) -> Config {
        let saved_kind = state.bank_kind.filter(|_| file.bank_dir.is_none());
        let bank_kind = saved_kind.or(file.bank_kind).unwrap_or(self.bank_kind);
        let bank_dir_in_config = file.bank_dir.is_some();
        let ir_dir_in_config = file.ir_dir.is_some();
        let bank_dir = file
            .bank_dir
            .or_else(|| state.banks.get(bank_kind).cloned())
            .unwrap_or_else(|| self.bank_dir.clone());
        let ports = match file.ports {
            Some(ports) => Ports {
                input: ports.input.unwrap_or(self.ports.input),
                output_left: ports.output_left.unwrap_or(self.ports.output_left),
                output_right: ports.output_right.unwrap_or(self.ports.output_right),
            },
            None => self.ports,
        };
        let ports = state.ports.clone().unwrap_or(ports);
        Config {
            ir_dir: state
                .ir_dir
                .clone()
                .or(file.ir_dir)
                .unwrap_or_else(|| bank_dir.clone()),
            bank_dir,
            bank_kind,
            buffer_size: file.buffer_size.unwrap_or(self.buffer_size),
            crossfade_ms: file.crossfade_ms.unwrap_or(self.crossfade_ms),
            ir_max_ms: file.ir_max_ms.unwrap_or(self.ir_max_ms),
            socket_is_fallback: self.socket_is_fallback && file.socket.is_none(),
            socket: file.socket.unwrap_or(self.socket),
            ports,
            tuner_reference_hz: file.tuner_reference_hz.unwrap_or(self.tuner_reference_hz),
            tuner_mute: file.tuner_mute.unwrap_or(self.tuner_mute),
            state_file: self.state_file,
            projects_dir: state.projects_dir.clone().unwrap_or(self.projects_dir),
            state,
            state_warning: None,
            bank_dir_in_config,
            ir_dir_in_config,
        }
    }
}

#[derive(Debug)]
pub struct SocketDirError {
    pub dir: PathBuf,
    pub reason: String,
}

/// Checks that `dir`, the `/tmp` fallback socket directory, is a real directory owned by this user with mode 0700; a wrong mode on our own directory is tightened rather than refused.
///
/// # Errors
///
/// A [`SocketDirError`] naming `dir` when it is a symlink, not a directory, owned by another user, or its mode cannot be changed.
pub fn secure_socket_dir(dir: &Path) -> Result<(), SocketDirError> {
    let uid = current_uid().map_err(|err| socket_dir_error(dir, err.to_string()))?;
    let meta = owned_dir(dir, uid)?;
    if meta.mode() & 0o777 != SOCKET_DIR_MODE {
        fs::set_permissions(dir, fs::Permissions::from_mode(SOCKET_DIR_MODE))
            .map_err(|err| socket_dir_error(dir, err.to_string()))?;
    }
    Ok(())
}

fn owned_dir(dir: &Path, uid: u32) -> Result<fs::Metadata, SocketDirError> {
    let meta = fs::symlink_metadata(dir).map_err(|err| socket_dir_error(dir, err.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(socket_dir_error(dir, "it is a symlink".to_string()));
    }
    if !meta.is_dir() {
        return Err(socket_dir_error(dir, "not a directory".to_string()));
    }
    if meta.uid() != uid {
        return Err(socket_dir_error(
            dir,
            format!("owned by uid {}, not {uid}", meta.uid()),
        ));
    }
    Ok(meta)
}

fn socket_dir_error(dir: &Path, reason: String) -> SocketDirError {
    SocketDirError {
        dir: dir.to_path_buf(),
        reason,
    }
}

/// This process's effective uid, the owner of `/proc/self`.
///
/// # Errors
///
/// The I/O error when `/proc/self` cannot be read.
pub(crate) fn current_uid() -> std::io::Result<u32> {
    Ok(fs::metadata("/proc/self")?.uid())
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

fn is_above_zero_at_most(value: f32, max: f32) -> bool {
    value > 0.0 && value <= max
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
            ConfigError::BufferSize { path } => write!(
                f,
                "{}: buffer_size must be between 1 and {SWITCHER_MAX_FRAMES}",
                path.display()
            ),
            ConfigError::NotFound { path } => write!(f, "{}: not found", path.display()),
            ConfigError::TunerReference { path } => write!(
                f,
                "{}: tuner_reference_hz must be between 400 and 480",
                path.display()
            ),
            ConfigError::CrossfadeMs { path } => write!(
                f,
                "{}: crossfade_ms must be above 0 and at most {CROSSFADE_MS_MAX}",
                path.display()
            ),
            ConfigError::IrMaxMs { path } => write!(
                f,
                "{}: ir_max_ms must be above 0 and at most {IR_MAX_MS_MAX}",
                path.display()
            ),
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
