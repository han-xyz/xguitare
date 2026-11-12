use std::collections::BTreeSet;
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
    let step = grid_step_beats(view, sample_rate, tempo);
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

/// Beats between two lines of [`grid_lines`].
#[allow(clippy::cast_precision_loss, reason = "a beat count far below 2^52")]
fn grid_step_beats(view: TimelineView, sample_rate: f64, tempo: Tempo) -> u64 {
    let beat_px = tempo.frames_per_beat(sample_rate) / view.frames_per_px;
    if beat_px >= MIN_GRID_PX {
        return 1;
    }
    let mut step = u64::from(tempo.beats_per_bar);
    while beat_px * (step as f64) < MIN_GRID_PX {
        step *= 2;
    }
    step
}

/// Frames between two lines of [`grid_lines`].
#[allow(clippy::cast_precision_loss, reason = "a beat count far below 2^52")]
pub fn grid_step(view: TimelineView, sample_rate: f64, tempo: Tempo) -> f64 {
    grid_step_beats(view, sample_rate, tempo) as f64 * tempo.frames_per_beat(sample_rate)
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
    /// The active Take's frames; a Take recorded again under the same file has others.
    pub take_frames: u64,
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
    Some(ItemBlock {
        id: item["id"].as_u64()?,
        track_id,
        position: item["position"].as_u64()?,
        offset: item["offset"].as_u64().unwrap_or_default(),
        length: item["length"].as_u64()?,
        file: path.to_string(),
        take_frames: item["take_frames"].as_u64().unwrap_or_default(),
        take_name: take_name(path),
        take,
        takes: takes.len(),
    })
}

/// The names of the Item `id`'s Takes in an `items` payload, in order: each file name without folder and `.wav`.
pub fn take_names(items_payload: &Value, id: u64) -> Vec<String> {
    let tracks = items_payload["tracks"].as_array().map(Vec::as_slice);
    let item = tracks
        .unwrap_or_default()
        .iter()
        .filter_map(|track| track["items"].as_array())
        .flatten()
        .find(|item| item["id"].as_u64() == Some(id));
    let takes = item.and_then(|item| item["takes"].as_array());
    takes
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(take_name)
        .collect()
}

fn take_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".wav").unwrap_or(file).to_string()
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

/// What has the keyboard focus: nothing, an Item on the timeline, another widget, or a text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFocus {
    Nothing,
    Item,
    Widget,
    TextField,
}

/// Plain keys act only while no widget has focus, so that a focused knob keeps Home, End and the arrows and a focused button its Space; a focused Item keeps only the arrows, which it takes itself. With modifiers, and M, T, S and Delete, keys act unless a text field takes typing.
pub fn key_active(key: KeyEvent, focus: KeyFocus) -> bool {
    match key {
        KeyEvent::Space | KeyEvent::R | KeyEvent::Home | KeyEvent::End => {
            matches!(focus, KeyFocus::Nothing | KeyFocus::Item)
        }
        KeyEvent::Left | KeyEvent::Right | KeyEvent::AltUp | KeyEvent::AltDown => {
            focus == KeyFocus::Nothing
        }
        KeyEvent::Digit(..)
        | KeyEvent::M
        | KeyEvent::T
        | KeyEvent::S
        | KeyEvent::Delete
        | KeyEvent::CtrlZ
        | KeyEvent::CtrlShiftZ
        | KeyEvent::CtrlD
        | KeyEvent::CtrlC
        | KeyEvent::CtrlV
        | KeyEvent::CtrlS
        | KeyEvent::CtrlR
        | KeyEvent::CtrlM
        | KeyEvent::CtrlAltR => shortcuts_active(focus == KeyFocus::TextField),
    }
}

/// A key a focused Item takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKey {
    Enter,
    Left,
    Right,
    Up,
    Down,
    F10,
    /// The context menu key.
    Menu,
}

/// An [`ItemKey`] with the modifiers it was pressed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemKeyPress {
    pub key: ItemKey,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// What a key does on a focused Item; steps are -1 (left, up) or 1 (right, down).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemNodeAction {
    /// Only this Item is selected.
    Select,
    /// This Item goes in or out of the selection.
    Toggle,
    /// The selected Items move by one grid step.
    Nudge(isize),
    /// The selected Items move to the next Track.
    TrackStep(isize),
    /// This Item's edge moves by one grid step.
    Trim(ItemEdge, isize),
    /// This Item's menu opens.
    Menu,
}

/// Enter selects, Ctrl+Enter toggles, ← and → nudge, Shift+← and Shift+→ trim the end, Ctrl+Shift+← and Ctrl+Shift+→ the start, Alt+↑ and Alt+↓ change the Track, Shift+F10 and the menu key open the Item's menu; under any other modifiers these keys do nothing.
pub fn item_node_action(press: ItemKeyPress) -> Option<ItemNodeAction> {
    let modifiers = (press.ctrl, press.shift, press.alt);
    match (press.key, modifiers) {
        (ItemKey::Enter, (false, false, false)) => Some(ItemNodeAction::Select),
        (ItemKey::Enter, (true, false, false)) => Some(ItemNodeAction::Toggle),
        (ItemKey::Left, (false, false, false)) => Some(ItemNodeAction::Nudge(-1)),
        (ItemKey::Right, (false, false, false)) => Some(ItemNodeAction::Nudge(1)),
        (ItemKey::Left, (false, true, false)) => Some(ItemNodeAction::Trim(ItemEdge::End, -1)),
        (ItemKey::Right, (false, true, false)) => Some(ItemNodeAction::Trim(ItemEdge::End, 1)),
        (ItemKey::Left, (true, true, false)) => Some(ItemNodeAction::Trim(ItemEdge::Start, -1)),
        (ItemKey::Right, (true, true, false)) => Some(ItemNodeAction::Trim(ItemEdge::Start, 1)),
        (ItemKey::Up, (false, false, true)) => Some(ItemNodeAction::TrackStep(-1)),
        (ItemKey::Down, (false, false, true)) => Some(ItemNodeAction::TrackStep(1)),
        (ItemKey::F10, (false, true, false)) | (ItemKey::Menu, (false, false, false)) => {
            Some(ItemNodeAction::Menu)
        }
        _ => None,
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
        KeyEvent::CtrlZ => Some(Command::ProjectUndo),
        KeyEvent::CtrlShiftZ => Some(Command::ProjectRedo),
        KeyEvent::Digit(
            digit,
            KeyModifiers {
                ctrl: true,
                alt: false,
            },
        ) => MainView::of_digit(digit).map(Command::ShowView),
        KeyEvent::Digit(..)
        | KeyEvent::M
        | KeyEvent::T
        | KeyEvent::S
        | KeyEvent::Delete
        | KeyEvent::CtrlD
        | KeyEvent::CtrlC
        | KeyEvent::CtrlV
        | KeyEvent::Left
        | KeyEvent::Right
        | KeyEvent::AltUp
        | KeyEvent::AltDown => None,
    }
}

/// The Arrange view's Item keys: S splits at the playhead the selected Items (none: the selected Track's), Delete deletes the selected Items, T plays the next Take of the one selected Item (after the last, the first), Ctrl+D duplicates and Ctrl+C copies the selected Items, Ctrl+V pastes a filled clipboard at the playhead on the selected Track, ← and → move the selected Items by one step of the grid drawn for `timeline` (a beat without one), Alt+↑ and Alt+↓ to the Track above or below.
pub fn item_key_to_command(
    key: KeyEvent,
    status: &Value,
    items: &[ItemBlock],
    selection: &ItemSelection,
    timeline: Option<TimelineView>,
) -> Option<Command> {
    let transport = transport_view(status)?;
    match key {
        KeyEvent::S => Some(Command::ItemSplit {
            ids: (!selection.is_empty()).then(|| selection.ids()),
            at: transport.frames,
        }),
        KeyEvent::Delete if !selection.is_empty() => Some(Command::ItemDelete(selection.ids())),
        KeyEvent::CtrlD if !selection.is_empty() => Some(Command::ItemDuplicate(selection.ids())),
        KeyEvent::CtrlC if !selection.is_empty() => Some(Command::ItemCopy(selection.ids())),
        KeyEvent::CtrlV if status["clipboard"].as_u64().unwrap_or_default() > 0 => {
            Some(Command::ItemPaste {
                track: usize::try_from(status["selected"].as_u64()?).ok()?,
                at: transport.frames,
            })
        }
        KeyEvent::T => {
            let [id] = selection.ids()[..] else {
                return None;
            };
            let item = items.iter().find(|item| item.id == id)?;
            (item.takes > 1).then(|| Command::ItemTake {
                id,
                take: (item.take + 1) % item.takes,
            })
        }
        KeyEvent::AltUp | KeyEvent::AltDown => {
            let track_ids: Vec<u64> = track_rows(status).iter().map(|row| row.id).collect();
            let step = if key == KeyEvent::AltUp { -1 } else { 1 };
            track_step_command(items, selection, &track_ids, step)
        }
        KeyEvent::Left | KeyEvent::Right => {
            let rate = sample_rate(status);
            let tempo = project_tempo(status)?;
            let step = timeline.map_or(tempo.frames_per_beat(rate), |view| {
                grid_step(view, rate, tempo)
            });
            let step = if key == KeyEvent::Left { -step } else { step };
            nudge_command(items, selection, step)
        }
        _ => None,
    }
}

/// The selected Items moved by `delta` frames, cut so that the earliest starts no earlier than frame 0; `None` when that leaves nothing to move.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "frame counts far below i64::MAX"
)]
pub fn nudge_command(
    items: &[ItemBlock],
    selection: &ItemSelection,
    delta: f64,
) -> Option<Command> {
    let earliest = items
        .iter()
        .filter(|item| selection.contains(item.id))
        .map(|item| item.position)
        .min()?;
    let delta = (delta.round() as i64).max(-(earliest as i64));
    (delta != 0).then(|| Command::ItemMove {
        ids: selection.ids(),
        delta,
        track: None,
    })
}

/// The selected Items moved `step` Tracks down (negative: up) among the Tracks `track_ids`, in row order; `None` when one of them would leave the Tracks.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "row indices far below isize::MAX"
)]
pub fn track_step_command(
    items: &[ItemBlock],
    selection: &ItemSelection,
    track_ids: &[u64],
    step: isize,
) -> Option<Command> {
    let row_of = |id: u64| {
        let item = items.iter().find(|item| item.id == id)?;
        let row = track_ids.iter().position(|track| *track == item.track_id)?;
        Some(row as isize)
    };
    let ids: Vec<u64> = selection
        .ids()
        .into_iter()
        .filter(|id| row_of(*id).is_some())
        .collect();
    let rows: Vec<isize> = ids.iter().filter_map(|id| row_of(*id)).collect();
    let last = track_ids.len() as isize - 1;
    let fits = rows.iter().all(|row| (0..=last).contains(&(row + step)));
    let first = *rows.first()?;
    fits.then(|| Command::ItemMove {
        ids,
        delta: 0,
        track: Some((first + step) as usize),
    })
}

/// Esc clears the Item selection when it is not taken by a focused widget, a menu or a dialog, before it would close the error toast.
pub fn esc_clears_selection(
    (widget_focused, popup_open): (bool, bool),
    dialog_open: bool,
    has_selection: bool,
) -> bool {
    has_selection && !widget_focused && !popup_open && !dialog_open
}

/// The footer's `Undo: <label>` for the edit `status` names next for `project_undo`.
pub fn undo_text(status: &Value) -> Option<String> {
    status["undo"]
        .as_str()
        .map(|label| format!("Undo: {label}"))
}

/// An Item's accessible name: `Item <take name>, <Track>, bar <n>`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a non-negative bar count far below u64::MAX"
)]
pub fn item_name(item: &ItemBlock, track: &str, sample_rate: f64, tempo: Tempo) -> String {
    let bar = (item.position as f64 / tempo.frames_per_bar(sample_rate)).floor() as u64 + 1;
    format!("Item {}, {track}, bar {bar}", item.take_name)
}

/// Pixels inside an Item's start and end that grab the edge rather than the body; at most a third of the Item each.
pub const EDGE_GRAB_PX: f32 = 6.0;

/// What a pointer on a lane is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemHit {
    Body(u64),
    StartEdge(u64),
    EndEdge(u64),
    Lane,
}

impl ItemHit {
    pub fn item(self) -> Option<u64> {
        match self {
            ItemHit::Body(id) | ItemHit::StartEdge(id) | ItemHit::EndEdge(id) => Some(id),
            ItemHit::Lane => None,
        }
    }
}

/// What lies under `x` pixels from the timeline's left edge on the lane of the Track `track_id`: the topmost Item there (the last in `items`), or the lane.
#[allow(clippy::cast_precision_loss, reason = "frames far below 2^52")]
pub fn hit_test(items: &[ItemBlock], track_id: u64, view: TimelineView, x: f32) -> ItemHit {
    let span = |item: &ItemBlock| {
        (
            view.x_of(item.position as f64),
            view.x_of(item.end() as f64),
        )
    };
    let Some((item, (left, right))) = items
        .iter()
        .rev()
        .filter(|item| item.track_id == track_id)
        .map(|item| (item, span(item)))
        .find(|(_, (left, right))| (*left..*right).contains(&x))
    else {
        return ItemHit::Lane;
    };
    let grab = EDGE_GRAB_PX.min((right - left) / 3.0);
    if x < left + grab {
        ItemHit::StartEdge(item.id)
    } else if x >= right - grab {
        ItemHit::EndEdge(item.id)
    } else {
        ItemHit::Body(item.id)
    }
}

/// The ids of the Items on the Tracks `track_ids` that overlap the timeline `frames`, in `items` order.
#[allow(clippy::cast_precision_loss, reason = "frames far below 2^52")]
pub fn items_in_box(items: &[ItemBlock], track_ids: &[u64], frames: Range<f64>) -> Vec<u64> {
    items
        .iter()
        .filter(|item| track_ids.contains(&item.track_id))
        .filter(|item| (item.position as f64) < frames.end && frames.start < item.end() as f64)
        .map(|item| item.id)
        .collect()
}

/// The selected Items, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemSelection(BTreeSet<u64>);

impl ItemSelection {
    pub fn contains(&self, id: u64) -> bool {
        self.0.contains(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// In ascending order.
    pub fn ids(&self) -> Vec<u64> {
        self.0.iter().copied().collect()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// A click: only `id` is selected, or with `toggle` (Ctrl) `id` goes in or out.
    pub fn pick(&mut self, id: u64, toggle: bool) {
        if !toggle {
            self.0.clear();
        }
        if !self.0.remove(&id) || !toggle {
            self.0.insert(id);
        }
    }

    /// A drag starting on `id`: the selection stays when it holds `id`, else as [`ItemSelection::pick`].
    pub fn grab(&mut self, id: u64, toggle: bool) {
        if !self.contains(id) {
            self.pick(id, toggle);
        }
    }

    /// The selection becomes `ids`, or with `add` takes them in as well.
    pub fn select_box(&mut self, ids: &[u64], add: bool) {
        if !add {
            self.0.clear();
        }
        self.0.extend(ids);
    }

    /// After a duplicate or paste: the Items new in `after` since `before` become the selection, when there are any.
    pub fn select_new(&mut self, before: &[u64], after: &[ItemBlock]) {
        let new: BTreeSet<u64> = after
            .iter()
            .map(|item| item.id)
            .filter(|id| !before.contains(id))
            .collect();
        if !new.is_empty() {
            self.0 = new;
        }
    }

    /// Keeps the ids still in `after`. With `split_at`, a split there was sent: each Item new since `before` that starts at `split_at` is selected with the Item ending there on its Track, when that one was selected or nothing was.
    pub fn follow(&mut self, before: &[u64], after: &[ItemBlock], split_at: Option<u64>) {
        let had_none = self.is_empty();
        self.0.retain(|id| after.iter().any(|item| item.id == *id));
        let Some(at) = split_at else {
            return;
        };
        for right in after
            .iter()
            .filter(|item| item.position == at && !before.contains(&item.id))
        {
            let left = after.iter().find(|left| {
                left.track_id == right.track_id && left.end() == at && before.contains(&left.id)
            });
            if let Some(left) = left
                && (had_none || self.contains(left.id))
            {
                self.0.extend([left.id, right.id]);
            }
        }
    }
}

/// A drag of the selected Items: the Item grabbed, where the pointer was pressed (a timeline frame and a Track row), and the Items moved, the grabbed one first.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveDrag {
    grabbed: u64,
    ids: Vec<u64>,
    grab_frame: f64,
    grab_row: usize,
}

impl MoveDrag {
    pub fn new(
        grabbed: u64,
        selection: &ItemSelection,
        grab_frame: f64,
        grab_row: usize,
    ) -> MoveDrag {
        let ids = [grabbed]
            .into_iter()
            .chain(selection.ids().into_iter().filter(|id| *id != grabbed))
            .collect();
        MoveDrag {
            grabbed,
            ids,
            grab_frame,
            grab_row,
        }
    }

    pub fn moves(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }
}

/// The pointer during a [`MoveDrag`]: its timeline frame, the Track row under it, whether to snap (the Snap switch, inverted while Alt is held) and whether Shift locks the Track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePointer {
    pub frame: f64,
    pub row: usize,
    pub snap: bool,
    pub lock_track: bool,
}

/// Where a [`MoveDrag`] would put its Items: `delta` frames and `rows` Tracks away, and the grid line the grabbed Item's start snapped to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MovePreview {
    pub delta: i64,
    pub rows: isize,
    pub snap_line: Option<u64>,
}

impl MovePreview {
    /// An Item's preview start and Track row, from its start and `row`.
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        reason = "the preview keeps every moved Item at frame 0 or later and on a Track"
    )]
    pub fn place(&self, item: &ItemBlock, row: usize) -> (u64, usize) {
        (
            (item.position as i64 + self.delta) as u64,
            (row as isize + self.rows) as usize,
        )
    }

    /// The `item_move` to send on release; `None` when nothing moves.
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        reason = "the preview keeps the grabbed Item on a Track"
    )]
    pub fn command(&self, drag: &MoveDrag) -> Option<Command> {
        if self.delta == 0 && self.rows == 0 {
            return None;
        }
        Some(Command::ItemMove {
            ids: drag.ids.clone(),
            delta: self.delta,
            track: (self.rows != 0).then(|| (drag.grab_row as isize + self.rows) as usize),
        })
    }
}

/// Where `drag` puts its Items with the pointer at `pointer`: all by one delta, the grabbed Item's start snapped to the nearest of the `grid` lines drawn when snapping, cut so that the earliest starts at frame 0 at the earliest; on the Track row under the pointer unless the Track is locked, cut so that every Item stays on one of the Tracks `track_ids` (in row order).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    reason = "frames and rows far below the limits of each type"
)]
pub fn move_preview(
    drag: &MoveDrag,
    items: &[ItemBlock],
    track_ids: &[u64],
    grid: &[GridLine],
    pointer: MovePointer,
) -> MovePreview {
    let moved: Vec<&ItemBlock> = items.iter().filter(|item| drag.moves(item.id)).collect();
    let Some(grabbed) = moved.iter().find(|item| item.id == drag.grabbed) else {
        return MovePreview::default();
    };
    let start = grabbed.position as f64 + pointer.frame - drag.grab_frame;
    let line = nearest_line(grid, start, pointer.snap);
    let wanted = line.unwrap_or(start).round() as i64 - grabbed.position as i64;
    let earliest = moved.iter().map(|item| item.position).min().unwrap_or(0) as i64;
    let delta = wanted.max(-earliest);
    let snap_line = line
        .map(|line| line as i64)
        .filter(|line| *line == grabbed.position as i64 + delta)
        .map(|line| line as u64);
    let row_of = |item: &&ItemBlock| {
        track_ids
            .iter()
            .position(|id| *id == item.track_id)
            .unwrap_or(0) as isize
    };
    let lowest = moved.iter().map(row_of).min().unwrap_or(0);
    let highest = moved.iter().map(row_of).max().unwrap_or(0);
    let last = track_ids.len() as isize - 1;
    let rows = if pointer.lock_track {
        0
    } else {
        (pointer.row as isize - drag.grab_row as isize).clamp(-lowest, last - highest)
    };
    MovePreview {
        delta,
        rows,
        snap_line,
    }
}

/// The whole frame of the `grid` line nearest `frame` when `snap`.
pub(crate) fn nearest_line(grid: &[GridLine], frame: f64, snap: bool) -> Option<f64> {
    grid.iter()
        .map(|line| line.frame.round())
        .filter(|_| snap)
        .min_by(|a, b| (a - frame).abs().total_cmp(&(b - frame).abs()))
}

/// An Item's start or end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemEdge {
    Start,
    End,
}

/// The edge of `item` moved to `frame`, kept on the Item's active Take and at least one frame from the other edge.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "frames far below 2^52; negative frames clamp to the Take's start"
)]
fn trimmed_edge(item: &ItemBlock, edge: ItemEdge, frame: f64) -> u64 {
    let take_start = item.position.saturating_sub(item.offset);
    let take_end = take_start.saturating_add(item.take_frames);
    let frame = frame.max(0.0).round() as u64;
    match edge {
        ItemEdge::Start => frame.max(take_start).min(item.end() - 1),
        ItemEdge::End => frame.min(take_end).max(item.position + 1),
    }
}

/// A drag of an Item's edge: the Item, the edge and the timeline frame the pointer was pressed at.
#[derive(Debug, Clone, PartialEq)]
pub struct TrimDrag {
    pub id: u64,
    pub edge: ItemEdge,
    grab_frame: f64,
}

impl TrimDrag {
    pub fn new(id: u64, edge: ItemEdge, grab_frame: f64) -> TrimDrag {
        TrimDrag {
            id,
            edge,
            grab_frame,
        }
    }
}

/// The edges a [`TrimDrag`] would give its Item, and the grid line the moved edge snapped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrimPreview {
    pub id: u64,
    pub start: u64,
    pub end: u64,
    pub snap_line: Option<u64>,
}

impl TrimPreview {
    /// The `item_trim` to send on release, naming only the edges that moved; `None` when none did.
    pub fn command(&self, item: &ItemBlock) -> Option<Command> {
        let start = (self.start != item.position).then_some(self.start);
        let end = (self.end != item.end()).then_some(self.end);
        (start.is_some() || end.is_some()).then_some(Command::ItemTrim {
            id: self.id,
            start,
            end,
        })
    }

    /// `item` as it would be after the trim: from `start` to `end`, playing its Take from where `start` falls on it.
    pub fn apply(&self, item: &ItemBlock) -> ItemBlock {
        ItemBlock {
            position: self.start,
            offset: (item.offset + self.start).saturating_sub(item.position),
            length: self.end - self.start,
            ..item.clone()
        }
    }
}

/// Where `drag` puts its Item's edge with the pointer at the timeline `frame`: moved as far as the pointer, onto the nearest of the `grid` lines when `snap`, then kept on the Item's Take and at least one frame long; `None` when the Item is not in `items`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "grid lines lie on whole non-negative frames far below 2^52"
)]
pub fn trim_preview(
    drag: &TrimDrag,
    items: &[ItemBlock],
    grid: &[GridLine],
    frame: f64,
    snap: bool,
) -> Option<TrimPreview> {
    let item = items.iter().find(|item| item.id == drag.id)?;
    let from = match drag.edge {
        ItemEdge::Start => item.position,
        ItemEdge::End => item.end(),
    };
    let wanted = from as f64 + frame - drag.grab_frame;
    let line = nearest_line(grid, wanted, snap);
    let moved = trimmed_edge(item, drag.edge, line.unwrap_or(wanted));
    let snap_line = line.map(|line| line as u64).filter(|line| *line == moved);
    let (start, end) = match drag.edge {
        ItemEdge::Start => (moved, item.end()),
        ItemEdge::End => (item.position, moved),
    };
    Some(TrimPreview {
        id: item.id,
        start,
        end,
        snap_line,
    })
}

/// The `item_trim` that moves `item`'s `edge` by `step` frames, kept as [`trim_preview`] keeps it; `None` when the edge stays.
#[allow(clippy::cast_precision_loss, reason = "frames far below 2^52")]
pub fn trim_step_command(item: &ItemBlock, edge: ItemEdge, step: f64) -> Option<Command> {
    let from = match edge {
        ItemEdge::Start => item.position,
        ItemEdge::End => item.end(),
    };
    let moved = trimmed_edge(item, edge, from as f64 + step);
    (moved != from).then(|| Command::ItemTrim {
        id: item.id,
        start: (edge == ItemEdge::Start).then_some(moved),
        end: (edge == ItemEdge::End).then_some(moved),
    })
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
