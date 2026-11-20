mod address;
pub mod arrangement;
pub mod atomic_file;
mod bank;
pub mod build;
mod config;
pub mod control;
pub mod engine;
pub mod firstrun;
pub mod flatten;
mod history;
mod ir;
mod irdir;
mod item_control;
pub mod metronome;
mod metronome_control;
mod mixer_control;
pub mod ports;
mod preset;
pub mod project;
pub mod project_control;
mod registry;
pub mod render;
pub mod render_control;
mod state;
pub mod take_capture;
pub mod take_peaks;
pub mod takes_control;
pub mod tuner;
pub mod watch;

pub use address::{AddressError, ParamAddress, ResolveError, resolve};
pub use bank::{Bank, BankEntry};
pub use config::{
    AUTO_PORT, BankKind, Config, ConfigError, Ports, SOCKET_DIR_MODE, SocketDirError, current_uid,
    secure_socket_dir,
};
pub use ir::{IrError, load_ir};
pub use irdir::list_irs;
pub use preset::{BuildError, BuildWarning, Preset, PresetError, PresetModule};
pub use registry::ModuleRegistry;
pub use state::{Banks, SavedMetronome, SavedState};
pub use xguitare_dsp::eq_response_db;
pub use xguitare_dsp::modules::eq::MAX_FREQ_PER_RATE as EQ_MAX_FREQ_PER_RATE;

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
