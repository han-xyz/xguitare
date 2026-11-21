use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tracing::warn;
use xguitare_core::take_capture::{FinishedTake, TrackReader};
use xguitare_core::take_writer::TakeWriter;

const POLL_INTERVAL: Duration = Duration::from_millis(10);

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
                    let drained = writer.drain();
                    for warning in drained.warnings {
                        warn!("{warning}");
                    }
                    for take in drained.finished {
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
