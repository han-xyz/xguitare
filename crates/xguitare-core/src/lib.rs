mod address;
mod bank;
mod config;
pub mod control;
pub mod engine;
mod ir;
mod preset;
mod registry;
pub mod watch;

pub use address::{AddressError, ParamAddress, ResolveError, resolve};
pub use bank::{Bank, BankEntry};
pub use config::{AUTO_PORT, Config, ConfigError, Ports, SocketDirError, secure_socket_dir};
pub use ir::{IrError, load_ir};
pub use preset::{BuildError, BuildWarning, Preset, PresetError, PresetModule};
pub use registry::ModuleRegistry;

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
