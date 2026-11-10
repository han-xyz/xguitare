use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use tracing::warn;
use xguitare_core::arrangement::ArrangementCommand;
use xguitare_core::build::{build_arrangement, swap_arrangement};
use xguitare_core::control::passthrough_rack;
use xguitare_core::engine::{Command, EngineQueues, EngineShared, engine_queues};
use xguitare_core::project::Project;
use xguitare_core::take_capture::add_finished_take;
use xguitare_core::{Config, ConfigError};

use crate::disk_writer::{DiskThread, TakeWriter};
use crate::jack_adapter::{self, Session};

const QUEUE_CAPACITY: usize = 64;
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const FINISH_TIMEOUT: Duration = Duration::from_secs(3);

/// The Project being recorded and everything that turns finished Takes into Takes.
struct RecordCheck<'a> {
    config: Config,
    dir: &'a Path,
    project: Project,
    takes: HashMap<String, Arc<[f32]>>,
    session: Session,
    disk: DiskThread,
    queues: EngineQueues,
}

/// Opens the Project in `project_dir` on its own JACK client, records its armed Tracks for `seconds` from playhead frame `at`, stops, and prints each Take with its placement; the Project file is not written.
pub fn run(
    config_path: Option<&Path>,
    project_dir: &Path,
    seconds: f64,
    at: u64,
) -> Result<(), String> {
    let config = Config::load(config_path).map_err(|err: ConfigError| err.to_string())?;
    let project = Project::load(project_dir).map_err(|err| err.to_string())?;
    let shared = Arc::new(EngineShared::default());
    let (queues, audio_queues, _tap) = engine_queues(QUEUE_CAPACITY);
    let session = jack_adapter::start(
        config.buffer_size,
        config.crossfade_ms,
        passthrough_rack,
        &shared,
        audio_queues,
    )
    .map_err(|err| err.to_string())?;
    session.connect(&config.ports);
    let inputs: Vec<Option<String>> = project.tracks.iter().map(|t| t.input.clone()).collect();
    session.connect_track_inputs(&inputs);
    if session.built_sample_rate() != project.sample_rate {
        warn!(
            "project rate {} differs from JACK rate {}",
            project.sample_rate,
            session.built_sample_rate()
        );
    }
    let disk = DiskThread::spawn(TakeWriter::new(
        project_dir.to_path_buf(),
        project.sample_rate,
    ))
    .map_err(|err| format!("cannot start the disk thread: {err}"))?;
    let armed = project.tracks.iter().filter(|track| track.arm).count();
    let mut check = RecordCheck {
        config,
        dir: project_dir,
        project,
        takes: HashMap::new(),
        session,
        disk,
        queues,
    };
    check.swap_in()?;
    check.send(Command::Arrangement(ArrangementCommand::Locate(at)))?;
    check.send(Command::Arrangement(ArrangementCommand::Record))?;
    let stop_at = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < stop_at {
        thread::sleep(POLL_INTERVAL);
        check.absorb_finished()?;
    }
    check.send(Command::Arrangement(ArrangementCommand::Stop))?;
    let deadline = Instant::now() + FINISH_TIMEOUT;
    let mut after_stop = 0;
    while after_stop < armed && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
        after_stop += check.absorb_finished()?;
    }
    check.send(Command::CloseArrangement)?;
    thread::sleep(POLL_INTERVAL);
    check.queues.drain_from_audio();
    println!(
        "disk_overflows {}",
        shared.disk_overflows.load(Ordering::Relaxed)
    );
    check.disk.stop();
    check.session.stop().map_err(|err| err.to_string())
}

impl RecordCheck<'_> {
    /// Builds the Arrangement of the Project as it is now, queues its readers on the disk thread and swaps it in.
    fn swap_in(&mut self) -> Result<(), String> {
        let (mut arrangement, warnings) = build_arrangement(
            &self.config,
            &self.project,
            &self.takes,
            self.session.built_sample_rate() as f32,
            self.session.max_frames(),
        )
        .map_err(|err| err.to_string())?;
        for warning in &warnings {
            warn!("{warning}");
        }
        let names = self.project.tracks.iter().map(|track| track.name.clone());
        self.disk
            .add_readers(names.zip(arrangement.take_readers()).collect());
        swap_arrangement(&mut self.queues, arrangement)
    }

    fn send(&mut self, command: Command) -> Result<(), String> {
        self.queues
            .commands
            .push(command)
            .map_err(|_| "engine busy".to_string())
    }

    /// Turns the Takes the disk thread finished into Takes, then swaps in an Arrangement that plays them; returns how many.
    fn absorb_finished(&mut self) -> Result<usize, String> {
        let finished = self.disk.finished();
        for take in &finished {
            let latency = self.session.track_latency(take.track);
            let position =
                add_finished_take(&mut self.project, self.dir, &mut self.takes, take, latency)
                    .map_err(|err| format!("cannot load take {}: {err}", take.file))?;
            println!(
                "take {} track {} frames {} record_start {} capture_latency {} playback_latency {} offset_ms {} placement {}",
                take.file,
                take.track,
                take.span.frames,
                take.span.start,
                latency.capture,
                latency.playback,
                self.project.latency_offset_ms,
                position.map_or_else(
                    || "none".to_string(),
                    |placed| format!("{} offset {}", placed.position, placed.offset)
                ),
            );
        }
        if !finished.is_empty() {
            self.swap_in()?;
        }
        Ok(finished.len())
    }
}
