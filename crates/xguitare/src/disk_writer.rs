use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hound::{SampleFormat, WavSpec, WavWriter};
use tracing::warn;
use xguitare_core::take_capture::{
    FinishedTake, ReadStop, TrackReader, WritingTakes, next_take_file,
};

const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Writes each Track's Takes as mono f32 WAV files under `<project>/takes/`, keeping `writing` current.
pub struct TakeWriter {
    project_dir: PathBuf,
    spec: WavSpec,
    tracks: Vec<TrackWriter>,
    buffer: Vec<f32>,
    writing: WritingTakes,
}

struct TrackWriter {
    readers: VecDeque<TrackReader>,
    output: Output,
    written: u64,
}

enum Output {
    Closed,
    Writing {
        file: String,
        wav: WavWriter<BufWriter<File>>,
    },
    /// The Take's file could not be written; its frames are read and dropped until the Take ends.
    Failed,
}

impl TakeWriter {
    pub fn new(project_dir: PathBuf, sample_rate: u32, writing: WritingTakes) -> TakeWriter {
        TakeWriter {
            project_dir,
            spec: WavSpec {
                channels: 1,
                sample_rate,
                bits_per_sample: 32,
                sample_format: SampleFormat::Float,
            },
            tracks: Vec::new(),
            buffer: Vec::new(),
            writing,
        }
    }

    /// Queues one reader per Track, in Track order, behind the readers already queued at that position: a Take that a replaced Arrangement handed over continues in the same file. A Take file is named after the Track of the reader that starts it, and a finished Take carries the id of the reader that ends it.
    pub fn add_readers(&mut self, readers: Vec<TrackReader>) {
        for (idx, reader) in readers.into_iter().enumerate() {
            if let Some(track) = self.tracks.get_mut(idx) {
                track.readers.push_back(reader);
            } else {
                self.tracks.push(TrackWriter {
                    readers: VecDeque::from([reader]),
                    output: Output::Closed,
                    written: 0,
                });
            }
        }
    }

    /// Writes what the rings hold and returns the Takes that ended, each closed in its own file.
    pub fn drain(&mut self) -> Vec<FinishedTake> {
        let mut finished = Vec::new();
        for track in &mut self.tracks {
            while let Some(front) = track.readers.front_mut() {
                let stop = front.reader.read(track.written, &mut self.buffer);
                let (track_id, name) = (front.track_id, front.name.clone());
                track.write(
                    &self.buffer,
                    &self.project_dir,
                    &name,
                    self.spec,
                    &self.writing,
                );
                match stop {
                    ReadStop::Empty => break,
                    ReadStop::HandedOver => {
                        track.readers.pop_front();
                    }
                    ReadStop::Abandoned => {
                        track.readers.pop_front();
                        if let Some(file) = track.close(&self.writing) {
                            warn!("take {file} lost its end mark; its file is kept but not placed");
                            unmark_writing(&self.writing, &file);
                        }
                    }
                    ReadStop::TakeEnd(span) => {
                        if let Some(file) = track.close(&self.writing) {
                            finished.push(FinishedTake {
                                track_id,
                                file,
                                span,
                            });
                        }
                    }
                }
            }
        }
        finished
    }
}

impl TrackWriter {
    fn write(
        &mut self,
        samples: &[f32],
        project_dir: &Path,
        name: &str,
        spec: WavSpec,
        writing: &WritingTakes,
    ) {
        if samples.is_empty() {
            return;
        }
        self.written += samples.len() as u64;
        if matches!(self.output, Output::Closed) {
            let file = next_take_file(project_dir, name);
            writing
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(file.clone());
            let created = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(project_dir.join(&file))
                .map_err(hound::Error::from)
                .and_then(|created| WavWriter::new(BufWriter::new(created), spec));
            self.output = match created {
                Ok(wav) => Output::Writing { file, wav },
                Err(err) => {
                    warn!("cannot create take {file}: {err}; dropping this take");
                    unmark_writing(writing, &file);
                    Output::Failed
                }
            };
        }
        let Output::Writing { file, wav } = &mut self.output else {
            return;
        };
        if let Err(err) = samples
            .iter()
            .try_for_each(|&sample| wav.write_sample(sample))
        {
            warn!("cannot write take {file}: {err}; dropping this take");
            unmark_writing(writing, file);
            self.output = Output::Failed;
        }
    }

    /// Ends the Take: finalizes its file and returns its name, or `None` when it had no frames or failed.
    fn close(&mut self, writing: &WritingTakes) -> Option<String> {
        self.written = 0;
        match std::mem::replace(&mut self.output, Output::Closed) {
            Output::Writing { file, wav } => match wav.finalize() {
                Ok(()) => Some(file),
                Err(err) => {
                    warn!("cannot finish take {file}: {err}; dropping this take");
                    unmark_writing(writing, &file);
                    None
                }
            },
            Output::Closed | Output::Failed => None,
        }
    }
}

fn unmark_writing(writing: &WritingTakes, file: &str) {
    writing
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(file);
}

/// The disk thread: runs a [`TakeWriter`] every few milliseconds and hands back the Takes it finished.
pub struct DiskThread {
    readers: Sender<Vec<TrackReader>>,
    finished: Receiver<FinishedTake>,
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl DiskThread {
    /// # Errors
    ///
    /// The error from spawning the thread.
    pub fn spawn(mut writer: TakeWriter) -> io::Result<DiskThread> {
        let (readers_tx, readers_rx) = mpsc::channel::<Vec<TrackReader>>();
        let (finished_tx, finished_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("disk".to_string())
            .spawn(move || {
                loop {
                    let last = stopping.load(Ordering::Relaxed);
                    for readers in readers_rx.try_iter() {
                        writer.add_readers(readers);
                    }
                    for take in writer.drain() {
                        let _ = finished_tx.send(take);
                    }
                    if last {
                        break;
                    }
                    thread::sleep(POLL_INTERVAL);
                }
            })?;
        Ok(DiskThread {
            readers: readers_tx,
            finished: finished_rx,
            stop,
            handle,
        })
    }

    /// Queues the readers of a newly built Arrangement; see [`TakeWriter::add_readers`].
    pub fn add_readers(&self, readers: Vec<TrackReader>) {
        let _ = self.readers.send(readers);
    }

    /// The Takes finished since the last call.
    pub fn finished(&self) -> Vec<FinishedTake> {
        self.finished.try_iter().collect()
    }

    /// Writes what the rings still hold, then ends the thread and returns the Takes finished since the last [`DiskThread::finished`]; a Take still open is left finalized at its length so far.
    pub fn stop(self) -> Vec<FinishedTake> {
        self.stop.store(true, Ordering::Relaxed);
        if self.handle.join().is_err() {
            warn!("disk thread panicked");
        }
        self.finished.try_iter().collect()
    }
}
