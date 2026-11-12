use std::ops::Range;

use serde_json::Value;
use xguitare_client::ClientError;
use xguitare_core::control::{Request, Response, SILENCE_DB, TransportAction};
use xguitare_core::project::LoopRegion;
use xguitare_core::project_control::STILL_CLOSING;
use xguitare_core::take_peaks::FRAMES_PER_PEAK;

use crate::model::{Command, KeyEvent, KeyModifiers, shortcuts_active};
use crate::theme::{Rgb, TRACK_COLOURS};

pub fn track_colour(index: usize) -> Rgb {
    TRACK_COLOURS[index % TRACK_COLOURS.len()]
}

/// Grid lines closer than this are thinned: beats first, then every other bar, and so on.
pub const MIN_GRID_PX: f64 = 8.0;
pub const MIN_FRAMES_PER_PX: f64 = 1.0;
/// One second per pixel at 48 kHz.
pub const MAX_FRAMES_PER_PX: f64 = 48_000.0;
/// The loop region the loop key sets when there is none.
pub const DEFAULT_LOOP_BARS: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f64,
    pub beats_per_bar: u32,
    /// The note value of one beat; the Daemon counts `bpm` in these beats.
    pub note: u32,
}

impl Tempo {
    pub fn frames_per_beat(self, sample_rate: f64) -> f64 {
        sample_rate * 60.0 / self.bpm
    }

    pub fn frames_per_bar(self, sample_rate: f64) -> f64 {
        self.frames_per_beat(sample_rate) * f64::from(self.beats_per_bar)
    }
}

/// The open Project's name from a `status` payload.
pub fn project_name(status: Option<&Value>) -> Option<&str> {
    status.and_then(|status| status["project"].as_str())
}

pub fn project_tempo(status: &Value) -> Option<Tempo> {
    let tempo = &status["tempo"];
    let bpm = tempo["bpm"].as_f64().filter(|bpm| *bpm > 0.0)?;
    let beats = u32::try_from(tempo["meter"][0].as_u64()?).ok()?;
    let note = u32::try_from(tempo["meter"][1].as_u64()?).ok()?;
    Some(Tempo {
        bpm,
        beats_per_bar: beats.max(1),
        note,
    })
}

pub fn sample_rate(status: &Value) -> f64 {
    status["sample_rate"].as_f64().unwrap_or(48_000.0)
}

/// `<bar>.<beat>.<hundredths of a beat>`, bars and beats counted from 1.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a non-negative beat count far below u64::MAX"
)]
pub fn bars_beats_text(frames: u64, sample_rate: f64, tempo: Tempo) -> String {
    let beats = frames as f64 / tempo.frames_per_beat(sample_rate);
    let hundredths = (beats * 100.0).round() as u64;
    let beat = hundredths / 100;
    let per_bar = u64::from(tempo.beats_per_bar);
    format!(
        "{}.{}.{:02}",
        beat / per_bar + 1,
        beat % per_bar + 1,
        hundredths % 100
    )
}

/// `m:ss.mmm`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a non-negative millisecond count far below u64::MAX"
)]
pub fn time_text(frames: u64, sample_rate: f64) -> String {
    let millis = (frames as f64 * 1000.0 / sample_rate).round() as u64;
    format!(
        "{}:{:02}.{:03}",
        millis / 60_000,
        millis / 1000 % 60,
        millis % 1000
    )
}

/// The visible part of the timeline: the frame at its left edge and the zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineView {
    pub first_frame: f64,
    pub frames_per_px: f64,
}

impl TimelineView {
    /// Ten seconds per thousand pixels.
    pub fn new(sample_rate: f64) -> TimelineView {
        TimelineView {
            first_frame: 0.0,
            frames_per_px: sample_rate / 100.0,
        }
    }

    /// Pixels from the timeline's left edge.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "screen coordinates fit an f32"
    )]
    pub fn x_of(self, frame: f64) -> f32 {
        ((frame - self.first_frame) / self.frames_per_px) as f32
    }

    /// Never before the Project start.
    pub fn frame_at(self, x: f32) -> f64 {
        (self.first_frame + f64::from(x) * self.frames_per_px).max(0.0)
    }

    /// `factor` above 1 zooms in; the frame at `anchor_x` stays where it is.
    #[must_use]
    pub fn zoomed(self, factor: f64, anchor_x: f32) -> TimelineView {
        let anchor = self.first_frame + f64::from(anchor_x) * self.frames_per_px;
        let frames_per_px =
            (self.frames_per_px / factor).clamp(MIN_FRAMES_PER_PX, MAX_FRAMES_PER_PX);
        TimelineView {
            first_frame: (anchor - f64::from(anchor_x) * frames_per_px).max(0.0),
            frames_per_px,
        }
    }

    #[must_use]
    pub fn scrolled(self, dx_px: f32) -> TimelineView {
        TimelineView {
            first_frame: (self.first_frame + f64::from(dx_px) * self.frames_per_px).max(0.0),
            ..self
        }
    }
}

/// A vertical line of the bar/beat grid; `beat` 1 is a bar line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLine {
    pub frame: f64,
    pub bar: u64,
    pub beat: u32,
}

/// The grid lines inside `width_px` of `view`: every beat when beats are [`MIN_GRID_PX`] apart, else every bar, every second bar, every fourth, as far as needed.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "non-negative beat indices far below u64::MAX"
)]
pub fn grid_lines(
    view: TimelineView,
    width_px: f32,
    sample_rate: f64,
    tempo: Tempo,
) -> Vec<GridLine> {
    let frames_per_beat = tempo.frames_per_beat(sample_rate);
    let per_bar = u64::from(tempo.beats_per_bar);
    let beat_px = frames_per_beat / view.frames_per_px;
    let mut step = 1;
    if beat_px < MIN_GRID_PX {
        step = per_bar;
        while beat_px * (step as f64) < MIN_GRID_PX {
            step *= 2;
        }
    }
    let last_frame = view.first_frame + f64::from(width_px) * view.frames_per_px;
    let first_beat = (view.first_frame / frames_per_beat).ceil() as u64;
    let first = first_beat.div_ceil(step) * step;
    (first..)
        .step_by(usize::try_from(step).unwrap_or(usize::MAX))
        .map(|beat| (beat, beat as f64 * frames_per_beat))
        .take_while(|(_, frame)| *frame <= last_frame)
        .map(|(beat, frame)| GridLine {
            frame,
            bar: beat / per_bar + 1,
            beat: u32::try_from(beat % per_bar).unwrap_or(0) + 1,
        })
        .collect()
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a non-negative frame far below u64::MAX"
)]
pub fn snap_to_beat(frame: f64, sample_rate: f64, tempo: Tempo) -> u64 {
    let frames_per_beat = tempo.frames_per_beat(sample_rate);
    ((frame.max(0.0) / frames_per_beat).round() * frames_per_beat).round() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    Stopped,
    Playing,
    Recording,
}

/// The transport bar's state from a `status` payload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportView {
    pub state: TransportState,
    pub frames: u64,
    pub loop_region: LoopRegion,
    pub click: bool,
    pub count_in: u32,
    /// Beats left before recording starts; 0 when no count-in runs.
    pub count_in_remaining: u64,
    pub disk_overflows: u64,
    pub loaded_audio_bytes: u64,
}

impl TransportView {
    pub fn play_lit(&self) -> bool {
        self.state != TransportState::Stopped
    }

    pub fn record_lit(&self) -> bool {
        self.state == TransportState::Recording
    }

    pub fn count_in_text(&self) -> Option<String> {
        (self.count_in_remaining > 0).then(|| format!("count-in: {}", self.count_in_remaining))
    }

    /// Megabytes (10⁶ bytes) of Take audio held in memory, one decimal.
    #[allow(
        clippy::cast_precision_loss,
        reason = "a byte count shown to one decimal"
    )]
    pub fn loaded_audio_text(&self) -> String {
        format!("audio {:.1} MB", self.loaded_audio_bytes as f64 / 1e6)
    }

    pub fn disk_warning(&self) -> Option<String> {
        (self.disk_overflows > 0).then(|| {
            format!(
                "disk too slow: frames dropped {} times",
                self.disk_overflows
            )
        })
    }
}

/// `None` while no Project is open.
pub fn transport_view(status: &Value) -> Option<TransportView> {
    project_name(Some(status))?;
    let state = match status["transport"].as_str()? {
        "playing" => TransportState::Playing,
        "recording" => TransportState::Recording,
        _ => TransportState::Stopped,
    };
    Some(TransportView {
        state,
        frames: status["position"]["frames"].as_u64().unwrap_or_default(),
        loop_region: loop_region(status),
        click: status["click"]["on"].as_bool().unwrap_or_default(),
        count_in: status["click"]["count_in"]
            .as_u64()
            .and_then(|bars| u32::try_from(bars).ok())
            .unwrap_or_default(),
        count_in_remaining: status["count_in_remaining"].as_u64().unwrap_or_default(),
        disk_overflows: status["disk_overflows"].as_u64().unwrap_or_default(),
        loaded_audio_bytes: status["loaded_audio_bytes"].as_u64().unwrap_or_default(),
    })
}

fn loop_region(status: &Value) -> LoopRegion {
    let region = &status["loop"];
    LoopRegion {
        start: region["start"].as_u64().unwrap_or_default(),
        end: region["end"].as_u64().unwrap_or_default(),
        on: region["on"].as_bool().unwrap_or_default(),
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "mute, solo, arm and selection are independent"
)]
pub struct TrackRow {
    pub index: usize,
    /// The id `items` names the Track by.
    pub id: u64,
    pub name: String,
    pub input: Option<String>,
    pub volume_db: f64,
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    pub armed: bool,
    pub input_peak_db: f64,
    pub selected: bool,
}

pub fn track_rows(status: &Value) -> Vec<TrackRow> {
    let selected = status["selected"].as_u64();
    status["tracks"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, track)| TrackRow {
            index,
            id: track["id"].as_u64().unwrap_or_default(),
            name: track["name"].as_str().unwrap_or_default().to_string(),
            input: track["input"].as_str().map(str::to_string),
            volume_db: track["volume_db"].as_f64().unwrap_or_default(),
            pan: track["pan"].as_f64().unwrap_or_default(),
            mute: track["mute"].as_bool().unwrap_or_default(),
            solo: track["solo"].as_bool().unwrap_or_default(),
            armed: track["armed"].as_bool().unwrap_or_default(),
            input_peak_db: track["input_peak_db"].as_f64().unwrap_or(SILENCE_DB),
            selected: selected == Some(index as u64),
        })
        .collect()
}

/// One Item on the timeline: its id, its Track's id, where it sits, the part of its active Take it plays from `offset`, and that Take's file and name, the file name without folder and `.wav`.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemBlock {
    pub id: u64,
    pub track_id: u64,
    pub position: u64,
    pub offset: u64,
    pub length: u64,
    /// The active Take's file, as `take_peaks` names it.
    pub file: String,
    pub take_name: String,
    /// The active Take's index among `takes`.
    pub take: usize,
    pub takes: usize,
}

/// The part of an Item's Take that a lane shows: from `x` pixels right of the timeline's left edge, `width` whole pixels wide, the Take's `frames`.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleTake {
    pub x: f32,
    pub width: usize,
    pub frames: Range<u64>,
}

impl ItemBlock {
    /// The Take name, with `(active/count)` when the Item has more than one Take.
    pub fn label(&self) -> String {
        if self.takes > 1 {
            format!("{} ({}/{})", self.take_name, self.take + 1, self.takes)
        } else {
            self.take_name.clone()
        }
    }

    pub fn end(&self) -> u64 {
        self.position.saturating_add(self.length)
    }

    /// The Take frames it plays that lie inside `0..width_px` of `view`, on whole pixels; `None` when none do.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "non-negative pixel and frame counts far below the limits of each type"
    )]
    pub fn visible_take(&self, view: TimelineView, width_px: f32) -> Option<VisibleTake> {
        let left = view.x_of(self.position as f64).max(0.0).round();
        let right = view.x_of(self.end() as f64).min(width_px).round();
        if right <= left {
            return None;
        }
        let played = |x: f32| {
            let frame = view.first_frame + f64::from(x) * view.frames_per_px - self.position as f64;
            self.offset + (frame.max(0.0).round() as u64).min(self.length)
        };
        Some(VisibleTake {
            x: left,
            width: (right - left) as usize,
            frames: played(left)..played(right),
        })
    }
}

/// One `(min, max)` per pixel for `width` pixels spanning the Take `frames`, from a peak table of `[min, max, …]` per [`FRAMES_PER_PEAK`] frames: the lowest min and highest max of the buckets a pixel covers, or the one bucket it lies in. Columns end where the table does.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "non-negative bucket indices far below usize::MAX"
)]
pub fn waveform_columns(peaks: &[i8], frames: Range<u64>, width: usize) -> Vec<(i8, i8)> {
    let buckets = peaks.len() / 2;
    if width == 0 || frames.is_empty() {
        return Vec::new();
    }
    let bucket_frames = FRAMES_PER_PEAK as f64;
    let per_px = (frames.end - frames.start) as f64 / width as f64;
    (0..width)
        .map_while(|px| {
            let from = frames.start as f64 + px as f64 * per_px;
            let first = (from / bucket_frames).floor() as usize;
            if first >= buckets {
                return None;
            }
            let last = (((from + per_px) / bucket_frames).ceil() as usize)
                .max(first + 1)
                .min(buckets);
            Some(
                peaks[first * 2..last * 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .fold((i8::MAX, i8::MIN), |(min, max), &[low, high]| {
                        (min.min(low), max.max(high))
                    }),
            )
        })
        .collect()
}

/// The Items of every Track in an `items` payload. Items name their Track by id, so that a reply older than the latest `status` never draws on a row that moved.
pub fn item_blocks(items_payload: &Value) -> Vec<ItemBlock> {
    let tracks = items_payload["tracks"].as_array().map(Vec::as_slice);
    tracks
        .unwrap_or_default()
        .iter()
        .flat_map(|entry| {
            let track_id = entry["id"].as_u64();
            entry["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |item| item_block(track_id?, item))
        })
        .collect()
}

fn item_block(track_id: u64, item: &Value) -> Option<ItemBlock> {
    let takes = item["takes"].as_array()?;
    let take = usize::try_from(item["take"].as_u64()?).ok()?;
    let path = takes.get(take)?.as_str()?;
    let file = path.rsplit('/').next().unwrap_or(path);
    Some(ItemBlock {
        id: item["id"].as_u64()?,
        track_id,
        position: item["position"].as_u64()?,
        offset: item["offset"].as_u64().unwrap_or_default(),
        length: item["length"].as_u64()?,
        file: path.to_string(),
        take_name: file.strip_suffix(".wav").unwrap_or(file).to_string(),
        take,
        takes: takes.len(),
    })
}

/// Where the Take being recorded began: the playhead the window first saw recording at once the count-in ended, kept while recording goes on; status names no such frame.
pub fn recording_from(previous: Option<u64>, transport: &TransportView) -> Option<u64> {
    let taking = transport.record_lit() && transport.count_in_remaining == 0;
    taking.then(|| previous.unwrap_or(transport.frames))
}

/// The frames the Take being recorded covers so far, from `from` to the playhead; from the loop start once the playhead has wrapped back past `from`.
pub fn recording_span(from: u64, transport: &TransportView) -> (u64, u64) {
    let start = if transport.frames < from {
        transport.loop_region.start
    } else {
        from
    };
    (start, transport.frames)
}

/// The end of the last Item or of the loop region, whichever is later.
pub fn project_end(status: &Value, items: &[ItemBlock]) -> u64 {
    items
        .iter()
        .map(ItemBlock::end)
        .chain([loop_region(status).end])
        .max()
        .unwrap_or_default()
}

/// What the main area shows while a Project is open: the shown Rack alone, the Tracks and timeline over it, or the mixer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainView {
    Rack,
    Arrange,
    Mixer,
}

impl MainView {
    /// Its name in eframe storage.
    pub fn stored(self) -> &'static str {
        match self {
            MainView::Rack => "rack",
            MainView::Arrange => "arrange",
            MainView::Mixer => "mixer",
        }
    }

    /// The view for Ctrl+1, Ctrl+2 and Ctrl+3.
    fn of_digit(digit: u8) -> Option<MainView> {
        match digit {
            1 => Some(MainView::Rack),
            2 => Some(MainView::Arrange),
            3 => Some(MainView::Mixer),
            _ => None,
        }
    }
}

/// The view stored by [`MainView::stored`]; Arrange when none or another name is stored.
pub fn restored_view(stored: Option<&str>) -> MainView {
    [MainView::Rack, MainView::Arrange, MainView::Mixer]
        .into_iter()
        .find(|view| Some(view.stored()) == stored)
        .unwrap_or(MainView::Arrange)
}

/// The view shown and the one before the mixer, which [`Command::ToggleMixer`] goes back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewSwitch {
    pub shown: MainView,
    previous: MainView,
}

impl ViewSwitch {
    pub fn new(shown: MainView) -> ViewSwitch {
        let previous = if shown == MainView::Mixer {
            MainView::Arrange
        } else {
            shown
        };
        ViewSwitch { shown, previous }
    }

    pub fn follow(&mut self, command: &Command) {
        let view = match command {
            Command::ShowView(view) => *view,
            Command::ToggleMixer if self.shown == MainView::Mixer => self.previous,
            Command::ToggleMixer => MainView::Mixer,
            _ => return,
        };
        if self.shown != MainView::Mixer {
            self.previous = self.shown;
        }
        self.shown = view;
    }
}

/// Plain keys act only while no widget has focus, so that a focused knob keeps Home and End and a focused button its Space; with modifiers, and M and T, they act unless a text field takes typing.
pub fn key_active(key: KeyEvent, text_edit_focused: bool, widget_focused: bool) -> bool {
    match key {
        KeyEvent::Space | KeyEvent::R | KeyEvent::Home | KeyEvent::End => !widget_focused,
        KeyEvent::Digit(..)
        | KeyEvent::M
        | KeyEvent::T
        | KeyEvent::CtrlS
        | KeyEvent::CtrlR
        | KeyEvent::CtrlM
        | KeyEvent::CtrlAltR => shortcuts_active(text_edit_focused),
    }
}

/// Reaper's keys while a Project is open, Ctrl+1, Ctrl+2 and Ctrl+3 (the views), Ctrl+M (mixer) and Ctrl+Alt+R (Render dialog) among them; the digit keys load no Preset then.
pub fn project_key_to_command(
    key: KeyEvent,
    status: &Value,
    items: &[ItemBlock],
) -> Option<Command> {
    let transport = transport_view(status)?;
    match key {
        KeyEvent::Space if transport.play_lit() => Some(Command::Transport(TransportAction::Stop)),
        KeyEvent::Space => Some(Command::Transport(TransportAction::Play)),
        KeyEvent::CtrlR => Some(Command::Transport(TransportAction::Record)),
        KeyEvent::R => Some(Command::Loop(toggled_loop(status, transport.loop_region))),
        KeyEvent::Home => Some(Command::Locate(0)),
        KeyEvent::End => Some(Command::Locate(project_end(status, items))),
        KeyEvent::CtrlS => Some(Command::ProjectSave),
        KeyEvent::CtrlM => Some(Command::ToggleMixer),
        KeyEvent::CtrlAltR => Some(Command::OpenRenderDialog),
        KeyEvent::Digit(
            digit,
            KeyModifiers {
                ctrl: true,
                alt: false,
            },
        ) => MainView::of_digit(digit).map(Command::ShowView),
        KeyEvent::Digit(..) | KeyEvent::M | KeyEvent::T => None,
    }
}

/// An empty region turns on as the first [`DEFAULT_LOOP_BARS`] bars.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a non-negative frame far below u64::MAX"
)]
fn toggled_loop(status: &Value, region: LoopRegion) -> LoopRegion {
    if region.start < region.end {
        return LoopRegion {
            on: !region.on,
            ..region
        };
    }
    let bar = project_tempo(status).map_or(0.0, |tempo| tempo.frames_per_bar(sample_rate(status)));
    LoopRegion {
        start: 0,
        end: (bar * DEFAULT_LOOP_BARS as f64).round() as u64,
        on: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopEdge {
    Start,
    End,
}

/// The region with `edge` moved to `frame`; an edge dragged past the other one swaps them.
pub fn dragged_loop(region: LoopRegion, edge: LoopEdge, frame: u64) -> LoopRegion {
    let other = match edge {
        LoopEdge::Start => region.end,
        LoopEdge::End => region.start,
    };
    LoopRegion {
        start: frame.min(other),
        end: frame.max(other),
        on: region.on,
    }
}

/// `<beats>/<note>`, e.g. `3/4`; the Daemon checks the range.
pub fn parse_meter(text: &str) -> Option<(u32, u32)> {
    let (beats, note) = text.split_once('/')?;
    Some((beats.trim().parse().ok()?, note.trim().parse().ok()?))
}

/// A Project whose autosave is newer than its `project.toml`: the `folder` (or name) it was opened by, and its display `name`.
#[derive(Debug, Clone, PartialEq)]
pub struct AutosaveOffer {
    pub folder: String,
    pub name: String,
}

/// The Project a `project_open` answered with a newer autosave it did not load.
pub fn autosave_offer(
    req: &Request,
    response: &Result<Response, ClientError>,
) -> Option<AutosaveOffer> {
    let (Request::ProjectOpen { name: folder, .. }, Ok(Response::Ok(payload))) = (req, response)
    else {
        return None;
    };
    let offered = payload["autosave_newer"].as_bool() == Some(true)
        && payload["restored"].as_bool() == Some(false);
    offered.then(|| AutosaveOffer {
        folder: folder.clone(),
        name: payload["name"].as_str().unwrap_or(folder).to_string(),
    })
}

pub fn still_closing(response: &Result<Response, ClientError>) -> bool {
    matches!(response, Ok(Response::Err { error }) if error == STILL_CLOSING)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRow {
    pub folder: String,
    pub name: String,
    /// Why `project.toml` does not load; `None` for a valid Project.
    pub error: Option<String>,
}

pub fn project_rows(project_list_payload: &Value) -> Vec<ProjectRow> {
    project_list_payload["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|project| ProjectRow {
            folder: project["folder"].as_str().unwrap_or_default().to_string(),
            name: project["name"].as_str().unwrap_or_default().to_string(),
            error: (project["valid"].as_bool() != Some(true)).then(|| {
                project["error"]
                    .as_str()
                    .unwrap_or("invalid project")
                    .to_string()
            }),
        })
        .collect()
}
