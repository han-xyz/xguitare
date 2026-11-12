use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::Path;
use std::sync::Arc;

use hound::WavReader;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::control::slug;
use crate::project::{Item, Project, TAKES_DIR, Take, Track};

/// A Take ring holds at least this many seconds, so the disk thread may fall that far behind before frames drop.
pub const RING_SECONDS: u32 = 2;
const MARK_CAPACITY: usize = 64;
const UNNAMED_TRACK: &str = "track";

/// Where one Take of a Track starts and how long it is, as captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakeSpan {
    /// The playhead at the Take's first recorded frame.
    pub start: u64,
    /// The frames that reached the ring; frames dropped on overflow are not counted.
    pub frames: u64,
    /// True when the Take began at a loop wrap while Recording went on.
    pub loop_take: bool,
}

#[derive(Debug, Clone, Copy)]
enum Mark {
    TakeEnd(TakeSpan),
    HandedOver,
}

/// Where [`TakeReader::read`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStop {
    /// The ring holds nothing more for now.
    Empty,
    /// The samples read were the last of this Take.
    TakeEnd(TakeSpan),
    /// The recorder handed its open Take to the one that replaced it: this reader gets nothing more and the Take goes on in the next.
    HandedOver,
    /// The recorder was dropped and its last mark was lost: this reader gets nothing more and a Take still open has no end.
    Abandoned,
}

/// The audio thread's end of one Track's Take ring. Allocates nothing after [`take_ring`].
pub struct TakeRecorder {
    samples: Producer<f32>,
    marks: Producer<Mark>,
    open: Option<TakeSpan>,
    loop_next: bool,
    overflows: u64,
    handed_over: bool,
}

/// The disk thread's end of one Track's Take ring.
pub struct TakeReader {
    samples: Consumer<f32>,
    marks: Consumer<Mark>,
}

/// One Track's Take reader, queued on the disk thread with the Track's name (for its Take files) and an id that stays the Track's while other Tracks are added or removed.
pub struct TrackReader {
    pub track_id: u64,
    pub name: String,
    pub reader: TakeReader,
}

/// A Take the disk thread wrote to `file` (relative to the Project folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedTake {
    /// The `track_id` of the [`TrackReader`] that read it.
    pub track_id: u64,
    pub file: String,
    pub span: TakeSpan,
}

/// JACK latencies in frames: of the capture port feeding the Track, and of the playback port the output feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Latency {
    pub capture: u32,
    pub playback: u32,
}

/// A Take ring of `capacity` samples.
pub fn take_ring(capacity: usize) -> (TakeRecorder, TakeReader) {
    let (samples_tx, samples_rx) = RingBuffer::new(capacity);
    let (marks_tx, marks_rx) = RingBuffer::new(MARK_CAPACITY);
    (
        TakeRecorder {
            samples: samples_tx,
            marks: marks_tx,
            open: None,
            loop_next: false,
            overflows: 0,
            handed_over: false,
        },
        TakeReader {
            samples: samples_rx,
            marks: marks_rx,
        },
    )
}

impl TakeRecorder {
    /// Appends `samples`, recorded from playhead frame `at`, to the open Take, opening one at `at` when none is open; what does not fit is dropped and counted as one overflow. Does nothing once this recorder handed over to another.
    pub fn push(&mut self, at: u64, samples: &[f32]) {
        if self.handed_over {
            return;
        }
        let loop_take = self.loop_next;
        let span = self.open.get_or_insert(TakeSpan {
            start: at,
            frames: 0,
            loop_take,
        });
        let (pushed, dropped) = self.samples.push_partial_slice(samples);
        span.frames += pushed.len() as u64;
        if !dropped.is_empty() {
            self.overflows += 1;
        }
    }

    /// Ends the open Take, if any; `wrapped` makes the next Take a loop Take.
    pub fn end_take(&mut self, wrapped: bool) {
        let Some(span) = self.open.take() else {
            self.loop_next = false;
            return;
        };
        self.loop_next = wrapped;
        self.mark(Mark::TakeEnd(span));
    }

    /// Takes over the open Take of the recorder this one replaces; `previous` records nothing more.
    pub fn continue_from(&mut self, previous: &mut TakeRecorder) {
        self.open = previous.open.take();
        self.loop_next = previous.loop_next;
        self.overflows += previous.take_overflows();
        previous.handed_over = true;
        previous.mark(Mark::HandedOver);
    }

    /// The overflows since the last call.
    pub fn take_overflows(&mut self) -> u64 {
        std::mem::take(&mut self.overflows)
    }

    fn mark(&mut self, mark: Mark) {
        if self.marks.push(mark).is_err() {
            self.overflows += 1;
        }
    }
}

impl Drop for TakeRecorder {
    /// A recorder dropped with a Take open (its Arrangement closed or retired while Recording) ends that Take, so the disk thread finishes its file.
    fn drop(&mut self) {
        self.end_take(false);
    }
}

impl TakeReader {
    /// Replaces `out` with the ring's samples of the current Take, of which `take_written` were read before (through this reader or the ones it follows), stopping at that Take's end.
    pub fn read(&mut self, take_written: u64, out: &mut Vec<f32>) -> ReadStop {
        out.clear();
        let abandoned = self.samples.is_abandoned();
        let mark = self.marks.peek().ok().copied();
        let available = self.samples.slots();
        let wanted = match mark {
            Some(Mark::TakeEnd(span)) => available.min(
                usize::try_from(span.frames.saturating_sub(take_written)).unwrap_or(usize::MAX),
            ),
            _ => available,
        };
        if let Ok(chunk) = self.samples.read_chunk(wanted) {
            out.extend(chunk);
        }
        match mark {
            Some(Mark::TakeEnd(span)) if take_written + out.len() as u64 >= span.frames => {
                let _ = self.marks.pop();
                ReadStop::TakeEnd(span)
            }
            Some(Mark::HandedOver) if self.samples.is_empty() => {
                let _ = self.marks.pop();
                ReadStop::HandedOver
            }
            None if abandoned && self.samples.is_empty() => ReadStop::Abandoned,
            _ => ReadStop::Empty,
        }
    }
}

/// Where an Item for a Take starts on the timeline, and how many of the Take's frames it skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakePlacement {
    pub position: u64,
    pub offset: u64,
}

/// Where a Take recorded from playhead `record_start` belongs: earlier by the capture and playback latency and by `offset_ms` of the Project's latency offset. A place before frame 0 becomes frame 0 with the Take's first frames skipped, so the rest still lands where it was heard.
pub fn take_position(
    record_start: u64,
    capture_latency: u32,
    playback_latency: u32,
    offset_ms: f64,
    rate: u32,
) -> TakePlacement {
    let offset = (offset_ms * f64::from(rate) / 1000.0).round() as i128;
    let compensation = i128::from(capture_latency) + i128::from(playback_latency) + offset;
    let position = i128::from(record_start) - compensation;
    TakePlacement {
        position: u64::try_from(position.max(0)).unwrap_or(u64::MAX),
        offset: u64::try_from(-position.min(0)).unwrap_or(u64::MAX),
    }
}

/// Adds `take` at `placement`: a loop Take placed like the Track's last Item becomes that Item's newest, active Take; anything else becomes a new Item.
pub fn add_take(track: &mut Track, placement: TakePlacement, take: Take, loop_take: bool) {
    if loop_take
        && let Some(item) = track.items.last_mut()
        && (item.position, item.offset) == (placement.position, placement.offset)
    {
        item.takes.push(take);
        item.take = item.takes.len() - 1;
        return;
    }
    track.items.push(Item {
        position: placement.position,
        offset: placement.offset,
        take: 0,
        takes: vec![take],
    });
}

/// The start of a Track's Take file names: its name as a slug, `track` when that is empty.
pub fn take_stem(track_name: &str) -> String {
    let stem = slug(track_name);
    if stem.is_empty() {
        UNNAMED_TRACK.to_string()
    } else {
        stem
    }
}

/// The first free `takes/<track slug>-<n>.wav` under `project_dir`, counting from 1.
pub fn next_take_file(project_dir: &Path, track_name: &str) -> String {
    let stem = take_stem(track_name);
    let mut n = 1u64;
    loop {
        let file = format!("{TAKES_DIR}/{stem}-{n}.wav");
        if !project_dir.join(&file).exists() {
            return file;
        }
        n += 1;
    }
}

/// Reads the mono f32 Take `file` under `project_dir` into memory.
///
/// # Errors
///
/// The `hound` error when the file cannot be read, and `Unsupported` when it is not mono.
pub fn load_take(project_dir: &Path, file: &str) -> Result<Arc<[f32]>, hound::Error> {
    let reader = WavReader::open(project_dir.join(file))?;
    if reader.spec().channels != 1 {
        return Err(hound::Error::Unsupported);
    }
    let samples = reader
        .into_samples::<f32>()
        .collect::<Result<Vec<_>, _>>()?;
    Ok(samples.into())
}

/// Loads a finished Take into `takes` and adds it as a Take on Track `track` at its latency-compensated placement; `None` when that Track does not exist.
///
/// # Errors
///
/// The `hound` error from [`load_take`]; the Project is then unchanged.
pub fn add_finished_take<S: BuildHasher>(
    project: &mut Project,
    project_dir: &Path,
    takes: &mut HashMap<String, Arc<[f32]>, S>,
    track: usize,
    finished: &FinishedTake,
    latency: Latency,
) -> Result<Option<TakePlacement>, hound::Error> {
    let placement = take_position(
        finished.span.start,
        latency.capture,
        latency.playback,
        project.latency_offset_ms,
        project.sample_rate,
    );
    let Some(track) = project.tracks.get_mut(track) else {
        return Ok(None);
    };
    let samples = load_take(project_dir, &finished.file)?;
    let take = Take {
        file: finished.file.clone(),
        frames: samples.len() as u64,
    };
    takes.insert(finished.file.clone(), samples);
    add_take(track, placement, take, finished.span.loop_take);
    Ok(Some(placement))
}
