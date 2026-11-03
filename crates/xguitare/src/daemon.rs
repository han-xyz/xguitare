use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use signal_hook::consts::{SIGINT, SIGTERM};
use tracing::{error, info, warn};
use xguitare_core::control::{EngineState, Request, Response, handle};
use xguitare_core::engine::{EngineShared, engine_queues};
use xguitare_core::watch::{Action, apply};
use xguitare_core::{Bank, BankEntry, Config, ConfigError, Preset, SocketDirError};
use xguitare_dsp::Rack;

use crate::jack_adapter::{self, JackError};
use crate::socket_server::{self, SocketError};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS_PER_REPORT: u32 = 10;
const QUEUE_CAPACITY: usize = 64;
const DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Debug)]
pub enum DaemonError {
    Config(ConfigError),
    Socket(SocketError),
    SocketDir(SocketDirError),
    Jack(JackError),
    Signal(io::Error),
}

pub fn run(config_path: Option<&Path>) -> Result<(), DaemonError> {
    let config = Config::load(config_path).map_err(DaemonError::Config)?;
    if config.socket_is_fallback {
        warn!("XDG_RUNTIME_DIR unset, using {}", config.socket.display());
        socket_server::secure_fallback_dir(&config.socket).map_err(DaemonError::SocketDir)?;
    }
    let socket = socket_server::bind(&config.socket).map_err(DaemonError::Socket)?;
    let bank = load_bank(&config.bank_dir);
    let preset = first_preset(&bank);
    let stop = register_stop_signals()?;
    let shared = Arc::new(EngineShared::default());
    let (queues, audio_queues) = engine_queues(QUEUE_CAPACITY);
    let mut active = None;
    let session = jack_adapter::start(
        config.buffer_size,
        config.crossfade_ms,
        |sample_rate, max_frames| {
            let built = preset
                .as_ref()
                .and_then(|preset| build_rack(preset, sample_rate, max_frames, &config));
            if built.is_some() {
                active = preset.map(|preset| (0, preset));
            }
            built.unwrap_or_else(|| passthrough_rack(sample_rate, max_frames))
        },
        Arc::clone(&shared),
        audio_queues,
    )
    .map_err(DaemonError::Jack)?;
    let connections = session.connect(&config.ports);
    let state = Arc::new(Mutex::new(EngineState {
        bank,
        active,
        shared: Arc::clone(&shared),
        queues,
        config,
        connections,
        sample_rate: shared.sample_rate.load(Ordering::Relaxed) as f32,
        max_frames: session.max_frames(),
    }));
    let mut bank_watch = BankWatch::start(&state_guard(&state).bank.dir);
    let control = socket
        .spawn(Arc::clone(&state), Arc::clone(&stop))
        .map_err(DaemonError::Socket)?;
    watch_until_stopped(
        &stop,
        &shared,
        &state,
        &mut bank_watch,
        session.max_frames(),
    );
    if control.join().is_err() {
        error!("control thread panicked");
    }
    session.stop().map_err(DaemonError::Jack)?;
    info!("stopped");
    Ok(())
}

fn load_bank(bank_dir: &Path) -> Bank {
    let entries = match Bank::load_dir(bank_dir) {
        Ok(bank) => return bank,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            warn!(
                "bank dir {} missing, running passthrough",
                bank_dir.display()
            );
            Vec::new()
        }
        Err(err) => {
            warn!(
                "cannot read bank dir {}: {err}, running passthrough",
                bank_dir.display()
            );
            Vec::new()
        }
    };
    Bank {
        dir: bank_dir.to_path_buf(),
        entries,
    }
}

fn first_preset(bank: &Bank) -> Option<Preset> {
    match bank.entries.first() {
        None => {
            warn!("bank {} is empty, running passthrough", bank.dir.display());
            None
        }
        Some(BankEntry::Invalid { file, error }) => {
            error!("preset {}: {error}, running passthrough", file.display());
            None
        }
        Some(BankEntry::Valid { preset, .. }) => Some(preset.clone()),
    }
}

fn build_rack(
    preset: &Preset,
    sample_rate: f32,
    max_frames: usize,
    config: &Config,
) -> Option<Rack> {
    match preset.build(sample_rate, max_frames, &config.bank_dir, config.ir_max_ms) {
        Ok((rack, warnings)) => {
            for warning in &warnings {
                warn!("{warning}");
            }
            info!("loaded 0 {}", preset.name);
            Some(rack)
        }
        Err(err) => {
            error!("preset '{}': {err}, running passthrough", preset.name);
            None
        }
    }
}

fn passthrough_rack(sample_rate: f32, max_frames: usize) -> Rack {
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

/// Watches the Bank dir; holds the debouncer, which stops watching when dropped.
struct BankWatch {
    _debouncer: Option<Debouncer<RecommendedWatcher>>,
    events: Receiver<DebounceEventResult>,
    stamps: HashMap<PathBuf, Option<FileStamp>>,
}

type FileStamp = (SystemTime, u64);

impl BankWatch {
    fn start(dir: &Path) -> BankWatch {
        let (sender, events) = mpsc::channel();
        let stamps = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| {
                let path = entry.path();
                let stamp = file_stamp(&path);
                (path, stamp)
            })
            .collect();
        BankWatch {
            _debouncer: debounced_watcher(dir, sender),
            events,
            stamps,
        }
    }

    /// Returns the paths whose modification time or length changed since last seen, so reading a file (the re-scan included) is not a change.
    fn changed_paths(&mut self) -> Vec<PathBuf> {
        // Editors save by truncating or by writing a new file and renaming it, so every event kind on a path is a candidate change.
        let candidates: Vec<PathBuf> = self
            .events
            .try_iter()
            .filter_map(|result| result.inspect_err(|err| warn!("bank watcher: {err}")).ok())
            .flatten()
            .map(|event| event.path)
            .collect();
        candidates
            .into_iter()
            .filter(|path| {
                let stamp = file_stamp(path);
                self.stamps.insert(path.clone(), stamp) != Some(stamp)
            })
            .collect()
    }
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn debounced_watcher(
    dir: &Path,
    events: mpsc::Sender<DebounceEventResult>,
) -> Option<Debouncer<RecommendedWatcher>> {
    let mut debouncer = match new_debouncer(DEBOUNCE, events) {
        Ok(debouncer) => debouncer,
        Err(err) => {
            warn!("cannot start bank watcher: {err}; hot reload off");
            return None;
        }
    };
    if let Err(err) = debouncer.watcher().watch(dir, RecursiveMode::NonRecursive) {
        warn!(
            "cannot watch bank dir {}: {err}; hot reload off",
            dir.display()
        );
        return None;
    }
    Some(debouncer)
}

fn state_guard(state: &Mutex<EngineState>) -> MutexGuard<'_, EngineState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn apply_bank_changes(state: &mut EngineState, bank_watch: &mut BankWatch) {
    let changed = bank_watch.changed_paths();
    if changed.is_empty() {
        return;
    }
    for action in apply(state, &changed) {
        match action {
            Action::Reindex => info!("bank reindexed: {} presets", state.bank.entries.len()),
            Action::Reload { program } => {
                let name = state.active.as_ref().map_or("", |(_, preset)| &preset.name);
                info!("reloaded {program} {name}");
            }
            Action::LogAndKeep { file, error } => {
                warn!(
                    "preset {}: {error}; keeping the running rack",
                    file.display()
                );
            }
            Action::Nothing => {}
        }
    }
}

fn apply_program_changes(state: &mut EngineState) {
    for program in state.queues.take_program_changes() {
        let len = state.bank.entries.len();
        if usize::from(program) >= len {
            warn!("midi program {program} out of range (bank has {len})");
            continue;
        }
        let load = Request::Load {
            preset: program.to_string(),
        };
        match handle(state, load) {
            Response::Ok(_) => {
                let name = state.active.as_ref().map_or("", |(_, preset)| &preset.name);
                info!("loaded {program} {name}");
            }
            Response::Err { error } => warn!("midi program {program}: {error}"),
        }
    }
}

fn watch_until_stopped(
    stop: &AtomicBool,
    shared: &EngineShared,
    state: &Mutex<EngineState>,
    bank_watch: &mut BankWatch,
    max_frames: usize,
) {
    let mut reported_xruns = 0;
    let mut reported_oversize = 0;
    let mut reported_buffer_size = shared.buffer_size.load(Ordering::Relaxed);
    let mut polls = 0;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(POLL_INTERVAL);
        {
            let mut state = state_guard(state);
            apply_bank_changes(&mut state, bank_watch);
            apply_program_changes(&mut state);
        }
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
            DaemonError::Socket(err) => err.fmt(f),
            DaemonError::SocketDir(err) => err.fmt(f),
            DaemonError::Jack(err) => err.fmt(f),
            DaemonError::Signal(err) => write!(f, "cannot register signal handler: {err}"),
        }
    }
}

impl std::error::Error for DaemonError {}
