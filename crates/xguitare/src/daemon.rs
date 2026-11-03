use std::fmt;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use signal_hook::consts::{SIGINT, SIGTERM};
use tracing::{error, info, warn};
use xguitare_core::engine::EngineShared;
use xguitare_core::{Bank, BankEntry, Config, ConfigError, Preset};
use xguitare_dsp::Rack;

use crate::jack_adapter::{self, JackError};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS_PER_REPORT: u32 = 10;

#[derive(Debug)]
pub enum DaemonError {
    Config(ConfigError),
    Jack(JackError),
    Signal(io::Error),
}

pub fn run(config_path: Option<&Path>) -> Result<(), DaemonError> {
    let config = Config::load(config_path).map_err(DaemonError::Config)?;
    let preset = first_preset(&config.bank_dir);
    let stop = register_stop_signals()?;
    let shared = Arc::new(EngineShared::default());
    let session = jack_adapter::start(
        config.buffer_size,
        |sample_rate, max_frames| {
            build_rack(
                preset.as_ref(),
                sample_rate,
                max_frames,
                &config.bank_dir,
                config.ir_max_ms,
            )
        },
        Arc::clone(&shared),
    )
    .map_err(DaemonError::Jack)?;
    session.connect(&config.ports);
    watch_until_stopped(&stop, &shared, session.max_frames());
    session.stop().map_err(DaemonError::Jack)?;
    info!("stopped");
    Ok(())
}

fn first_preset(bank_dir: &Path) -> Option<Preset> {
    let bank = match Bank::load_dir(bank_dir) {
        Ok(bank) => bank,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            warn!(
                "bank dir {} missing, running passthrough",
                bank_dir.display()
            );
            return None;
        }
        Err(err) => {
            warn!(
                "cannot read bank dir {}: {err}, running passthrough",
                bank_dir.display()
            );
            return None;
        }
    };
    match bank.entries.into_iter().next() {
        None => {
            warn!("bank {} is empty, running passthrough", bank_dir.display());
            None
        }
        Some(BankEntry::Invalid { file, error }) => {
            error!("preset {}: {error}, running passthrough", file.display());
            None
        }
        Some(BankEntry::Valid { preset, .. }) => Some(preset),
    }
}

fn build_rack(
    preset: Option<&Preset>,
    sample_rate: f32,
    max_frames: usize,
    bank_dir: &Path,
    ir_max_ms: f32,
) -> Rack {
    if let Some(preset) = preset {
        match preset.build(sample_rate, max_frames, bank_dir, ir_max_ms) {
            Ok((rack, warnings)) => {
                for warning in &warnings {
                    warn!("{warning}");
                }
                info!("loaded 0 {}", preset.name);
                return rack;
            }
            Err(err) => error!("preset '{}': {err}, running passthrough", preset.name),
        }
    }
    let mut rack = Rack::new(Vec::new()).expect("an empty Rack has no layout to violate");
    rack.prepare(sample_rate, max_frames);
    rack
}

fn register_stop_signals() -> Result<Arc<AtomicBool>, DaemonError> {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop)).map_err(DaemonError::Signal)?;
    }
    Ok(stop)
}

fn watch_until_stopped(stop: &AtomicBool, shared: &EngineShared, max_frames: usize) {
    let mut reported_xruns = 0;
    let mut reported_oversize = 0;
    let mut reported_buffer_size = shared.buffer_size.load(Ordering::Relaxed);
    let mut polls = 0;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(POLL_INTERVAL);
        polls += 1;
        if polls % POLLS_PER_REPORT != 0 {
            continue;
        }
        let xruns = shared.xruns.load(Ordering::Relaxed);
        if xruns > reported_xruns {
            warn!("xruns: {xruns} (+{})", xruns - reported_xruns);
            reported_xruns = xruns;
        }
        let buffer_size = shared.buffer_size.load(Ordering::Relaxed);
        if buffer_size != reported_buffer_size {
            warn!("buffer size changed to {buffer_size}");
            reported_buffer_size = buffer_size;
        }
        let oversize = shared.oversize_blocks.load(Ordering::Relaxed);
        if oversize > reported_oversize {
            warn!(
                "{} oversize blocks dropped: quantum {buffer_size} > prepared {max_frames}; restart or force the quantum",
                oversize - reported_oversize
            );
            reported_oversize = oversize;
        }
    }
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DaemonError::Config(err) => err.fmt(f),
            DaemonError::Jack(err) => err.fmt(f),
            DaemonError::Signal(err) => write!(f, "cannot register signal handler: {err}"),
        }
    }
}

impl std::error::Error for DaemonError {}
