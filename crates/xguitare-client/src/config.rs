use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Deserializer};
use xguitare_protocol::{BankKind, default_socket};

const CONFIG_FILE: &str = "config.toml";
const SOCKET_DIR_MODE: u32 = 0o700;

/// What a client reads from the Daemon's config file: the same file and search path, only the keys a client needs; every other key is the Daemon's and is ignored here.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientConfig {
    pub socket: PathBuf,
    /// True when `$XDG_RUNTIME_DIR` is unset and `socket` is the `/tmp/xguitare-<uid>` fallback.
    pub socket_is_fallback: bool,
    /// Whether a client's tuner mode turns on Mute while it is open.
    pub tuner_mute: bool,
    pub bank_kind: BankKind,
}

#[derive(Debug)]
pub enum ClientConfigError {
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
    NotFound {
        path: PathBuf,
    },
}

#[derive(Deserialize)]
struct ClientConfigFile {
    socket: Option<PathBuf>,
    tuner_mute: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_bank_kind")]
    bank_kind: Option<BankKind>,
}

fn deserialize_bank_kind<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<BankKind>, D::Error> {
    BankKind::deserialize(deserializer)
        .map(Some)
        .map_err(|_| serde::de::Error::custom("bank_kind must be guitar or bass"))
}

impl ClientConfig {
    /// Reads `path`, else `config.toml` in the user's config directory, where a missing file means every default.
    ///
    /// # Errors
    ///
    /// A [`ClientConfigError`] when there is no home directory, the uid cannot be read, an explicit `path` is missing, or the file cannot be read or parsed.
    pub fn load(path: Option<&Path>) -> Result<ClientConfig, ClientConfigError> {
        let dirs = ProjectDirs::from("", "", "xguitare").ok_or(ClientConfigError::NoHomeDir)?;
        let default_path = dirs.config_dir().join(CONFIG_FILE);
        let explicit = path.is_some();
        let path = path.unwrap_or(&default_path);
        let runtime_dir = dirs.runtime_dir();
        let socket = default_socket(runtime_dir, current_uid).map_err(ClientConfigError::Uid)?;
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound && explicit => {
                return Err(ClientConfigError::NotFound {
                    path: path.to_path_buf(),
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(ClientConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let file: ClientConfigFile = toml::from_str(&text).map_err(|error| {
            let (line, col) = toml_position(&text, &error);
            ClientConfigError::Syntax {
                path: path.to_path_buf(),
                line,
                col,
                msg: error.message().to_string(),
            }
        })?;
        Ok(ClientConfig {
            socket_is_fallback: runtime_dir.is_none() && file.socket.is_none(),
            socket: file.socket.unwrap_or(socket),
            tuner_mute: file.tuner_mute.unwrap_or(true),
            bank_kind: file.bank_kind.unwrap_or_default(),
        })
    }
}

/// Refuses `dir` unless it is a real directory owned by `uid` with mode 0700; it never changes the mode.
///
/// # Errors
///
/// The refusal text `refusing socket directory <dir>: <reason>` when `dir` is a symlink, not a directory, owned by another uid, or not mode 0700.
pub fn check_private_dir(dir: &Path, uid: u32) -> Result<(), String> {
    let refusal = |reason: String| format!("refusing socket directory {}: {reason}", dir.display());
    let meta = fs::symlink_metadata(dir).map_err(|err| refusal(err.to_string()))?;
    let mode = meta.mode() & 0o777;
    if meta.file_type().is_symlink() {
        Err(refusal("it is a symlink".to_string()))
    } else if !meta.is_dir() {
        Err(refusal("not a directory".to_string()))
    } else if meta.uid() != uid {
        Err(refusal(format!("owned by uid {}, not {uid}", meta.uid())))
    } else if mode != SOCKET_DIR_MODE {
        Err(refusal(format!(
            "mode {mode:04o}, not {SOCKET_DIR_MODE:04o}"
        )))
    } else {
        Ok(())
    }
}

/// This process's effective uid, the owner of `/proc/self`.
pub(crate) fn current_uid() -> std::io::Result<u32> {
    Ok(fs::metadata("/proc/self")?.uid())
}

fn toml_position(text: &str, error: &toml::de::Error) -> (usize, usize) {
    let Some(span) = error.span() else {
        return (1, 1);
    };
    let before = &text[..span.start.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[line_start..].chars().count() + 1)
}

impl fmt::Display for ClientConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientConfigError::NoHomeDir => {
                f.write_str("no home directory to derive the default config paths from")
            }
            ClientConfigError::Uid(error) => {
                write!(f, "cannot determine uid for the socket fallback: {error}")
            }
            ClientConfigError::Syntax {
                path,
                line,
                col,
                msg,
            } => write!(f, "{}: line {line}:{col}: {msg}", path.display()),
            ClientConfigError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ClientConfigError::NotFound { path } => write!(f, "{}: not found", path.display()),
        }
    }
}

impl std::error::Error for ClientConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientConfigError::Io { source, .. } | ClientConfigError::Uid(source) => Some(source),
            _ => None,
        }
    }
}
