use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, ErrorKind};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use signal_hook::consts::{SIGINT, SIGTERM};
use tracing::{error, info, warn};
use xguitare_core::build;
use xguitare_core::control::{
    EngineState, Reprepare, Request, Response, follow_sample_rate, handle, needs_reprepare,
    passthrough_rack, reprepare,
};
use xguitare_core::engine::{EngineShared, SWITCHER_MAX_FRAMES, engine_queues};
use xguitare_core::firstrun::ensure_bank;
use xguitare_core::ports::{AudioPorts, capture_track_inputs};
use xguitare_core::project::MAX_TRACKS;
use xguitare_core::project_control::{
    ClosedProject, LastTakes, ProjectEvent, add_finished_takes, autosave_if_due, close_for_stop,
    finish_closed, finish_stopped, project_rate_warning, take_project_events,
};
use xguitare_core::render::RenderTask;
use xguitare_core::render_control::{RenderFinished, cancel_render, finished_render};
use xguitare_core::take_capture::Latency;
use xguitare_core::tuner::Tuner;
use xguitare_core::watch::{Action, FileStamp, apply, changed_paths};
use xguitare_core::{Bank, BankEntry, BankKind, Config, ConfigError, Preset, SocketDirError};
use xguitare_dsp::Rack;

use crate::disk_writer::{DiskThread, TakeWriter};
use crate::jack_adapter::{self, JackError, Session};
use crate::socket_server::{self, SharedPorts, SocketError};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS_PER_REPORT: u32 = 10;
const QUEUE_CAPACITY: usize = 64;
const DEBOUNCE: Duration = Duration::from_millis(150);
const STARTER_IR_RATE: u32 = 48_000;

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
    if let Some(warning) = &config.state_warning {
        warn!("{warning}; ignoring it");
    }
    if config.socket_is_fallback {
        warn!("XDG_RUNTIME_DIR unset, using {}", config.socket.display());
        socket_server::secure_fallback_dir(&config.socket).map_err(DaemonError::SocketDir)?;
    }
    let socket = socket_server::bind(
        &config.socket,
        socket_server::MAX_CONNECTIONS,
        socket_server::IDLE_TIMEOUT,
    )
    .map_err(DaemonError::Socket)?;
    create_starter_bank(&config.bank_dir, config.bank_kind);
    let bank = load_bank(&config.bank_dir);
    let preset = first_preset(&bank);
    let stop = register_stop_signals()?;
    let shared = Arc::new(EngineShared::default());
    let (queues, audio_queues, tap) = engine_queues(QUEUE_CAPACITY);
    let mut active = None;
    let session = jack_adapter::start(
        config.buffer_size,
        config.crossfade_ms,
        |sample_rate, max_frames| {
            let built = preset
                .as_ref()
                .and_then(|(_, preset)| build_rack(preset, sample_rate, max_frames, &config));
            if built.is_some() {
                active = preset;
            }
            built.unwrap_or_else(|| passthrough_rack(sample_rate, max_frames))
        },
        &shared,
        audio_queues,
    )
    .map_err(DaemonError::Jack)?;
    let connections = session.connect(&config.ports);
    let sample_rate = session.built_sample_rate() as f32;
    let tuner = Tuner::new(tap, sample_rate, config.tuner_reference_hz);
    let state = Arc::new(Mutex::new(
        EngineState::new(
            bank,
            Arc::clone(&shared),
            queues,
            tuner,
            config,
            sample_rate,
            session.max_frames(),
        )
        .with_active(active)
        .with_connections(connections),
    ));
    let mut bank_watch = BankWatch::start(&state_guard(&state).bank.dir);
    let session = Arc::new(session);
    let ports: SharedPorts = Arc::downgrade(&session) as Weak<_>;
    let control = socket
        .spawn(Arc::clone(&state), ports, Arc::clone(&stop))
        .map_err(DaemonError::Socket)?;
    let mut disk = ProjectDisk::default();
    watch_until_stopped(&stop, &shared, &state, &session, &mut bank_watch, &mut disk);
    if control.join().is_err() {
        error!("control thread panicked");
    }
    let latencies: Vec<Latency> = (0..MAX_TRACKS)
        .map(|track| session.track_latency(track))
        .collect();
    let (session, open) = {
        let mut state = state_guard(&state);
        state.queues.drain_from_audio();
        (Arc::try_unwrap(session), close_for_stop(&mut state))
    };
    let stopped = if let Ok(session) = session {
        session.stop().map_err(DaemonError::Jack)
    } else {
        error!("jack session still borrowed at stop");
        Ok(())
    };
    disk.finish_at_stop(&mut state_guard(&state), open, &latencies);
    stopped?;
    info!("stopped");
    Ok(())
}

/// The disk thread of the open Project, and those of Projects closed on the previous tick, which are still finishing the Takes their Arrangement had open.
#[derive(Default)]
struct ProjectDisk {
    open: Option<DiskThread>,
    closing: Vec<(DiskThread, ClosedProject)>,
    /// The thread of the running Render, or of the last one.
    render: Option<thread::JoinHandle<()>>,
}

impl ProjectDisk {
    /// Carries out the Project events, turns finished Takes into Takes and autosaves when due.
    fn follow(&mut self, state: &mut EngineState, session: &Session) {
        // A Project closed on an earlier tick: the audio thread has retired its Arrangement since, and this drain drops it, which ends its open Takes.
        state.queues.drain_from_audio();
        for (thread, closed) in mem::take(&mut self.closing) {
            let last = finish_closed(state, closed, &thread.stop(), |track| {
                session.track_latency(track)
            });
            log_last_takes(&last);
        }
        for event in take_project_events(state) {
            self.apply(state, event, session);
        }
        if let Some(thread) = &self.open {
            let finished = thread.finished();
            for take in &finished {
                info!(
                    "recorded {}: {} frames from frame {}",
                    take.file, take.span.frames, take.span.start
                );
            }
            let failures =
                add_finished_takes(state, &finished, |track| session.track_latency(track));
            for failure in failures {
                warn!("{failure}");
            }
        }
        log_finished_render(state);
        match autosave_if_due(state, Instant::now()) {
            Some(Ok(file)) => info!("autosaved {}", file.display()),
            Some(Err(err)) => warn!("cannot autosave: {err}"),
            None => {}
        }
    }

    fn apply(&mut self, state: &mut EngineState, event: ProjectEvent, session: &Session) {
        match event {
            ProjectEvent::Opened {
                dir,
                sample_rate,
                writing,
                warnings,
            } => {
                info!("opened project {}", dir.display());
                for warning in warnings {
                    warn!("{warning}");
                }
                match DiskThread::spawn(TakeWriter::new(dir, sample_rate, writing)) {
                    Ok(thread) => self.open = Some(thread),
                    Err(err) => error!("cannot start the disk thread: {err}; recording is off"),
                }
            }
            ProjectEvent::Readers(readers) => {
                if let Some(thread) = &self.open {
                    thread.add_readers(readers);
                }
            }
            ProjectEvent::Inputs(inputs) => {
                let (inputs, warnings) = capture_track_inputs(&inputs, &session.audio_ports().0);
                for warning in warnings {
                    warn!("{warning}");
                }
                session.connect_track_inputs(&inputs);
            }
            ProjectEvent::Render(task) => self.start_render(task),
            ProjectEvent::Closed(closed) => {
                info!("closed project {}", closed.dir.display());
                match self.open.take() {
                    Some(thread) => self.closing.push((thread, closed)),
                    None => {
                        log_last_takes(&finish_closed(state, closed, &[], |_| Latency::default()));
                    }
                }
            }
        }
    }

    fn start_render(&mut self, task: RenderTask) {
        let job = Arc::clone(&task.job);
        info!("rendering {}", job.file().display());
        if let Some(previous) = self.render.take()
            && previous.join().is_err()
        {
            error!("render thread panicked");
        }
        match thread::Builder::new()
            .name("render".to_string())
            .spawn(move || task.run())
        {
            Ok(handle) => self.render = Some(handle),
            Err(err) => job.fail(format!("cannot start the render thread: {err}")),
        }
    }

    /// Cancels a running Render and waits for its thread, which removes the part file.
    fn stop_render(&mut self, state: &mut EngineState) {
        cancel_render(state);
        if let Some(handle) = self.render.take()
            && handle.join().is_err()
        {
            error!("render thread panicked");
        }
        log_finished_render(state);
    }

    /// After the JACK client stopped, which ended every open Take: adds the last Takes to the Projects closing, which are saved, and to the one still open, which is autosaved when it changed.
    fn finish_at_stop(
        mut self,
        state: &mut EngineState,
        open: Option<ClosedProject>,
        latencies: &[Latency],
    ) {
        self.stop_render(state);
        let latency = |track: usize| latencies.get(track).copied().unwrap_or_default();
        for (thread, closed) in self.closing {
            log_last_takes(&finish_closed(state, closed, &thread.stop(), latency));
        }
        let Some(open) = open else {
            return;
        };
        let finished = self.open.map_or_default(DiskThread::stop);
        log_last_takes(&finish_stopped(open, &finished, latency));
    }
}

fn log_finished_render(state: &mut EngineState) {
    match finished_render(state) {
        Some(RenderFinished::Rendered(file)) => info!("rendered {}", file.display()),
        Some(RenderFinished::Cancelled) => info!("render cancelled"),
        Some(RenderFinished::Failed(error)) => error!("render failed: {error}"),
        None => {}
    }
}

fn log_last_takes(last: &LastTakes) {
    for file in &last.added {
        info!("added take {file}");
    }
    for failure in &last.failures {
        warn!("{failure}");
    }
    if let Some(file) = &last.saved {
        info!("saved {}", file.display());
    }
}

fn create_starter_bank(bank_dir: &Path, kind: BankKind) {
    match ensure_bank(bank_dir, kind, STARTER_IR_RATE) {
        Ok(created) if created.presets > 0 => {
            info!("created starter bank ({kind}) in {}", bank_dir.display());
        }
        Ok(_) => {}
        Err(err) => warn!(
            "cannot create starter bank in {}: {err}",
            bank_dir.display()
        ),
    }
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

fn first_preset(bank: &Bank) -> Option<(PathBuf, Preset)> {
    match bank.entries.first() {
        None => {
            warn!("bank {} is empty, running passthrough", bank.dir.display());
            None
        }
        Some(BankEntry::Invalid { file, error }) => {
            error!("preset {}: {error}, running passthrough", file.display());
            None
        }
        Some(BankEntry::Valid { file, preset }) => Some((file.clone(), preset.clone())),
    }
}

fn build_rack(
    preset: &Preset,
    sample_rate: f32,
    max_frames: usize,
    config: &Config,
) -> Option<Rack> {
    match build::build_rack(config, sample_rate, max_frames, preset) {
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

fn register_stop_signals() -> Result<Arc<AtomicBool>, DaemonError> {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop)).map_err(DaemonError::Signal)?;
    }
    Ok(stop)
}

/// Watches the Bank dir; holds the debouncer, which stops watching when dropped.
struct BankWatch {
    dir: PathBuf,
    _debouncer: Option<Debouncer<RecommendedWatcher>>,
    events: Receiver<DebounceEventResult>,
    stamps: HashMap<PathBuf, FileStamp>,
    retries: Vec<PathBuf>,
}

impl BankWatch {
    fn start(dir: &Path) -> BankWatch {
        let (sender, events) = mpsc::channel();
        let paths: Vec<PathBuf> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        BankWatch {
            dir: dir.to_path_buf(),
            _debouncer: debounced_watcher(dir, sender),
            events,
            stamps: snapshot(&paths),
            retries: Vec::new(),
        }
    }

    /// Returns the paths whose modification time or length changed since last seen, so reading a file (the re-scan included) is not a change.
    fn changed_paths(&mut self) -> Vec<PathBuf> {
        // Editors save by truncating or by writing a new file and renaming it, so every event kind on a path is a candidate change.
        let events = self
            .events
            .try_iter()
            .filter_map(|result| result.inspect_err(|err| warn!("bank watcher: {err}")).ok())
            .flatten()
            .map(|event| event.path);
        let candidates: Vec<PathBuf> = mem::take(&mut self.retries)
            .into_iter()
            .chain(events)
            .collect();
        let current = snapshot(&candidates);
        let changed = changed_paths(&self.stamps, &current, &candidates);
        for path in &candidates {
            match current.get(path) {
                Some(stamp) => self.stamps.insert(path.clone(), *stamp),
                None => self.stamps.remove(path),
            };
        }
        changed
    }

    /// Makes `file` count as changed on the next poll, whether or not it changes again.
    fn retry_next_tick(&mut self, file: PathBuf) {
        self.stamps.remove(&file);
        self.retries.push(file);
    }
}

fn snapshot(paths: &[PathBuf]) -> HashMap<PathBuf, FileStamp> {
    paths
        .iter()
        .filter_map(|path| {
            let metadata = fs::metadata(path).ok()?;
            Some((path.clone(), (metadata.modified().ok()?, metadata.len())))
        })
        .collect()
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
            Action::EngineBusy { file } => {
                warn!("preset {}: engine busy; retrying", file.display());
                bank_watch.retry_next_tick(file);
            }
            Action::LogAndKeep { file, error } => {
                warn!(
                    "preset {}: {error}; keeping the running rack",
                    file.display()
                );
            }
            Action::Warn(warning) => warn!("{warning}"),
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
            track: None,
            bus: None,
            master: false,
        };
        let (response, warnings) = handle(state, load);
        for warning in warnings {
            warn!("{warning}");
        }
        match response {
            Response::Ok(_) => {
                let name = state.active.as_ref().map_or("", |(_, preset)| &preset.name);
                info!("loaded {program} {name}");
            }
            Response::Err { error, .. } => warn!("midi program {program}: {error}"),
        }
    }
}

fn watch_until_stopped(
    stop: &AtomicBool,
    shared: &EngineShared,
    state: &Mutex<EngineState>,
    session: &Session,
    bank_watch: &mut BankWatch,
    disk: &mut ProjectDisk,
) {
    let mut reported_xruns = 0;
    let mut reported_oversize = 0;
    let mut reported_over_limit = None;
    let mut reported_buffer_size = shared.buffer_size.load(Ordering::Relaxed);
    let mut polls = 0;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(POLL_INTERVAL);
        {
            let mut state = state_guard(state);
            if state.bank.dir != bank_watch.dir {
                info!(
                    "bank switched to {}: {} presets",
                    state.bank.dir.display(),
                    state.bank.entries.len()
                );
                *bank_watch = BankWatch::start(&state.bank.dir);
            }
            follow_sample_rate_change(&mut state);
            state.tuner.tick();
            apply_bank_changes(&mut state, bank_watch);
            apply_program_changes(&mut state);
            disk.follow(&mut state, session);
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
        let mut state = state_guard(state);
        let reprepares = needs_reprepare(state.max_frames, buffer_size);
        let oversize = shared.oversize_blocks.load(Ordering::Relaxed);
        if oversize > reported_oversize {
            warn!(
                "{} oversize blocks dropped: quantum {buffer_size}, rack running at {} frames{}",
                oversize - reported_oversize,
                shared.running_frames.load(Ordering::Relaxed),
                if reprepares { "; re-preparing" } else { "" }
            );
            reported_oversize = oversize;
        }
        if buffer_size as usize <= SWITCHER_MAX_FRAMES {
            reported_over_limit = None;
        } else if reported_over_limit != Some(buffer_size) {
            error!(
                "quantum {buffer_size} exceeds the switcher limit {SWITCHER_MAX_FRAMES}; output is silent until the quantum is lowered"
            );
            reported_over_limit = Some(buffer_size);
        }
        if reprepares {
            reprepare_rack(&mut state, buffer_size);
        }
    }
}

fn follow_sample_rate_change(state: &mut EngineState) {
    let old = state.sample_rate as u32;
    let Some(changed) = follow_sample_rate(state) else {
        return;
    };
    match changed {
        Ok(outcome) => {
            let new = state.sample_rate as u32;
            info!("sample rate changed: {old} -> {new}");
            if let Some(mismatch) = project_rate_warning(state) {
                warn!("{mismatch}; the project does not play or record until they match");
            }
            match outcome {
                Reprepare::Built(warnings) => {
                    for warning in &warnings {
                        warn!("{warning}");
                    }
                }
                Reprepare::Passthrough { preset, error } => {
                    error!(
                        "rebuild of '{preset}' at {new} Hz failed: {error}; running passthrough"
                    );
                }
            }
        }
        Err(err) => {
            let new = state.shared.sample_rate.load(Ordering::Relaxed);
            warn!("cannot rebuild rack at {new} Hz: {err}; retrying");
        }
    }
}

fn reprepare_rack(state: &mut EngineState, buffer_size: u32) {
    match reprepare(state, buffer_size) {
        Ok(Reprepare::Built(warnings)) => {
            for warning in &warnings {
                warn!("{warning}");
            }
            info!("re-prepared rack for {buffer_size} frames");
        }
        Ok(Reprepare::Passthrough { preset, error }) => {
            error!("re-prepare of '{preset}' failed: {error}; running passthrough");
        }
        Err(err) => warn!("cannot re-prepare rack for {buffer_size} frames: {err}; retrying"),
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
