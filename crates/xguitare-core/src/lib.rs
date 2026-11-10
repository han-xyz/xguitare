mod address;
pub mod arrangement;
mod bank;
pub mod build;
mod config;
pub mod control;
pub mod engine;
pub mod firstrun;
mod ir;
mod irdir;
pub mod ports;
mod preset;
pub mod project;
mod registry;
mod state;
pub mod take_capture;
pub mod tuner;
pub mod watch;

pub use address::{AddressError, ParamAddress, ResolveError, resolve};
pub use bank::{Bank, BankEntry};
pub use config::{
    AUTO_PORT, BankKind, Config, ConfigError, Ports, SOCKET_DIR_MODE, SocketDirError,
    check_private_dir, current_uid, secure_socket_dir,
};
pub use ir::{IrError, load_ir};
pub use irdir::list_irs;
pub use preset::{BuildError, BuildWarning, Preset, PresetError, PresetModule};
pub use registry::ModuleRegistry;
pub use state::{Banks, SavedState};

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[line_start..].chars().count() + 1)
}

fn toml_position(text: &str, error: &toml::de::Error) -> (usize, usize) {
    error
        .span()
        .map_or((1, 1), |span| line_col(text, span.start))
}
