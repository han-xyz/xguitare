use std::collections::VecDeque;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::take_capture::{FinishedTake, ReadStop, TrackReader, WritingTakes, next_take_file};

/// Writes each Track's Takes as mono f32 WAV files under `<project>/takes/`, keeping `writing` current.
pub struct TakeWriter {
    project_dir: PathBuf,
    spec: WavSpec,
    tracks: Vec<TrackWriter>,
    buffer: Vec<f32>,
    writing: WritingTakes,
}

/// What one [`TakeWriter::drain`] produced: the Takes that ended and the problems met on the way.
#[derive(Debug, Default, PartialEq)]
pub struct Drained {
    pub finished: Vec<FinishedTake>,
    pub warnings: Vec<TakeWarning>,
}

/// A Take the writer dropped or could not place; `Display` gives the log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeWarning {
    Create { file: String, error: String },
    Write { file: String, error: String },
    Finish { file: String, error: String },
    Flush { file: String, error: String },
    Abandoned { file: String },
}

impl fmt::Display for TakeWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TakeWarning::Create { file, error } => {
                write!(f, "cannot create take {file}: {error}; dropping this take")
            }
            TakeWarning::Write { file, error } => {
                write!(f, "cannot write take {file}: {error}; dropping this take")
            }
            TakeWarning::Finish { file, error } => {
                write!(f, "cannot finish take {file}: {error}; dropping this take")
            }
            TakeWarning::Flush { file, error } => {
                write!(f, "cannot flush take {file}: {error}; dropping this take")
            }
            TakeWarning::Abandoned { file } => write!(
                f,
                "take {file} lost its end mark; its file is kept but not placed"
            ),
        }
    }
}

struct TrackWriter {
    readers: VecDeque<TrackReader>,
    output: Output,
    written: u64,
    unflushed: u64,
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
                    unflushed: 0,
                });
            }
        }
    }

    /// Writes what the rings hold, flushing each Take's file when it is created and once per second of audio after, so a copy or a crash leaves it readable up to the last flush, and returns the Takes that ended, each closed in its own file, with the warnings for the Takes dropped on the way.
    pub fn drain(&mut self) -> Drained {
        let mut drained = Drained::default();
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
                    &mut drained.warnings,
                );
                match stop {
                    ReadStop::Empty => break,
                    ReadStop::HandedOver => {
                        track.readers.pop_front();
                    }
                    ReadStop::Abandoned => {
                        track.readers.pop_front();
                        if let Some(file) = track.close(&self.writing, &mut drained.warnings) {
                            unmark_writing(&self.writing, &file);
                            drained.warnings.push(TakeWarning::Abandoned { file });
                        }
                    }
                    ReadStop::TakeEnd(span) => {
                        if let Some(file) = track.close(&self.writing, &mut drained.warnings) {
                            drained.finished.push(FinishedTake {
                                track_id,
                                file,
                                span,
                            });
                        }
                    }
                }
            }
        }
        drained
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
        warnings: &mut Vec<TakeWarning>,
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
                    unmark_writing(writing, &file);
                    warnings.push(TakeWarning::Create {
                        file,
                        error: err.to_string(),
                    });
                    Output::Failed
                }
            };
            self.flush(writing, warnings);
        }
        let Output::Writing { file, wav } = &mut self.output else {
            return;
        };
        if let Err(err) = samples
            .iter()
            .try_for_each(|&sample| wav.write_sample(sample))
        {
            unmark_writing(writing, file);
            warnings.push(TakeWarning::Write {
                file: file.clone(),
                error: err.to_string(),
            });
            self.output = Output::Failed;
            return;
        }
        self.unflushed += samples.len() as u64;
        if self.unflushed < u64::from(spec.sample_rate) {
            return;
        }
        self.unflushed = 0;
        self.flush(writing, warnings);
    }

    fn flush(&mut self, writing: &WritingTakes, warnings: &mut Vec<TakeWarning>) {
        let Output::Writing { file, wav } = &mut self.output else {
            return;
        };
        if let Err(err) = wav.flush() {
            unmark_writing(writing, file);
            warnings.push(TakeWarning::Flush {
                file: file.clone(),
                error: err.to_string(),
            });
            self.output = Output::Failed;
        }
    }

    /// Ends the Take: finalizes its file and returns its name, or `None` when it had no frames or failed.
    fn close(&mut self, writing: &WritingTakes, warnings: &mut Vec<TakeWarning>) -> Option<String> {
        self.written = 0;
        self.unflushed = 0;
        match std::mem::replace(&mut self.output, Output::Closed) {
            Output::Writing { file, wav } => match wav.finalize() {
                Ok(()) => Some(file),
                Err(err) => {
                    unmark_writing(writing, &file);
                    warnings.push(TakeWarning::Finish {
                        file,
                        error: err.to_string(),
                    });
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
