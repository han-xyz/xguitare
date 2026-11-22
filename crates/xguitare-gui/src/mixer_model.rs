use std::time::{Duration, Instant};

use serde_json::Value;
use xguitare_protocol::{BusRef, SILENCE_DB, SendLevel, TrackRef};

use crate::model::{Band, Command, LADDER_RED_DB, LADDER_YELLOW_DB, Segment, acts_on_rack};
use crate::project_model::{project_name_payload, track_rows};

/// The fader position of 0 dB; the top is +6 dB.
pub const FADER_UNITY: f32 = 0.75;
pub const FADER_TOP_DB: f64 = 6.0;
/// The lowest volume and Send level the Daemon takes; a Send below it is off.
pub const FADER_FLOOR_DB: f64 = -60.0;
pub const MIXER_PEAK_HOLD_FOR: Duration = Duration::from_secs(1);
pub const MASTER: &str = "Master";
/// The LEDs in each of a strip's two ladders.
pub const STRIP_LADDER_SEGMENTS: usize = 20;

/// A Track, a Bus or Master: a column of the mixer panel and the Rack it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MixerTarget {
    Track(usize),
    Bus(usize),
    Master,
}

/// Solo in place: `SoloInPlace` sounds only because a soloed Track sends to it or it sends to a soloed Bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audibility {
    Audible,
    SoloInPlace,
    Silent,
}

/// A Track's Send to the Bus at index `bus`; `level_db` is `None` while the Send is off.
#[derive(Debug, Clone, PartialEq)]
pub struct SendView {
    pub bus: usize,
    pub level_db: Option<f64>,
    pub pre: bool,
}

/// `pan` is `None` for Master; `to_master` is `Some` and `sends` has one entry per Bus only for a Track; `volume_db` is `-inf` for a Master at -inf.
#[derive(Debug, Clone, PartialEq)]
pub struct MixerColumn {
    pub target: MixerTarget,
    pub name: String,
    pub volume_db: f64,
    pub pan: Option<f64>,
    pub mute: bool,
    pub solo: bool,
    pub peak_db: [f64; 2],
    pub clip: bool,
    pub to_master: Option<bool>,
    pub sends: Vec<SendView>,
    pub audibility: Audibility,
}

/// The Tracks, then the Buses, then Master, from a `status` payload; none while no Project is open.
pub fn mixer_columns(status: &Value) -> Vec<MixerColumn> {
    if project_name_payload(status).is_none() {
        return Vec::new();
    }
    let tracks = entries(&status["tracks"]);
    let buses = entries(&status["buses"]);
    let sends: Vec<Vec<SendView>> = tracks
        .iter()
        .map(|track| track_sends(track, buses.len()))
        .collect();
    let any_solo = tracks.iter().chain(&buses).any(|entry| flag(entry, "solo"));
    let mut columns = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let sends_to_solo = sends[index]
            .iter()
            .any(|send| send.level_db.is_some() && flag(&buses[send.bus], "solo"));
        columns.push(MixerColumn {
            to_master: Some(track["to_master"].as_bool().unwrap_or(true)),
            sends: sends[index].clone(),
            audibility: audibility(track, any_solo, sends_to_solo),
            ..column(MixerTarget::Track(index), track)
        });
    }
    for (index, bus) in buses.iter().enumerate() {
        let sent_to_by_solo = tracks
            .iter()
            .zip(&sends)
            .any(|(track, sends)| flag(track, "solo") && sends[index].level_db.is_some());
        columns.push(MixerColumn {
            audibility: audibility(bus, any_solo, sent_to_by_solo),
            ..column(MixerTarget::Bus(index), bus)
        });
    }
    let master = &status["master"];
    columns.push(MixerColumn {
        name: MASTER.to_string(),
        pan: None,
        volume_db: master["volume_db"].as_f64().unwrap_or(f64::NEG_INFINITY),
        ..column(MixerTarget::Master, master)
    });
    columns
}

fn entries(list: &Value) -> Vec<Value> {
    list.as_array().cloned().unwrap_or_default()
}

fn flag(entry: &Value, key: &str) -> bool {
    entry[key].as_bool().unwrap_or_default()
}

fn audibility(entry: &Value, any_solo: bool, brought_in: bool) -> Audibility {
    if flag(entry, "mute") || (any_solo && !flag(entry, "solo") && !brought_in) {
        Audibility::Silent
    } else if any_solo && !flag(entry, "solo") {
        Audibility::SoloInPlace
    } else {
        Audibility::Audible
    }
}

fn column(target: MixerTarget, entry: &Value) -> MixerColumn {
    let peak = |side: usize| entry["peak_db"][side].as_f64().unwrap_or(SILENCE_DB);
    MixerColumn {
        target,
        name: entry["name"].as_str().unwrap_or_default().to_string(),
        volume_db: entry["volume_db"].as_f64().unwrap_or_default(),
        pan: Some(entry["pan"].as_f64().unwrap_or_default()),
        mute: flag(entry, "mute"),
        solo: flag(entry, "solo"),
        peak_db: [peak(0), peak(1)],
        clip: flag(entry, "clip"),
        to_master: None,
        sends: Vec::new(),
        audibility: Audibility::Audible,
    }
}

/// One Send per Bus; status lists only the Sends that are not off.
fn track_sends(track: &Value, bus_count: usize) -> Vec<SendView> {
    let listed = entries(&track["sends"]);
    (0..bus_count)
        .map(|bus| {
            let on = listed
                .iter()
                .find(|send| send["bus"].as_u64() == Some(bus as u64));
            SendView {
                bus,
                level_db: on.and_then(|send| send["level_db"].as_f64()),
                pre: on.is_some_and(|send| flag(send, "pre")),
            }
        })
        .collect()
}

/// The exponent that puts +6 dB at the top when 0 dB sits at [`FADER_UNITY`]: position = unity · gain^exponent.
fn fader_exponent() -> f64 {
    (1.0 / f64::from(FADER_UNITY)).ln() / (FADER_TOP_DB / 20.0 * std::f64::consts::LN_10)
}

/// 0 at -inf, [`FADER_UNITY`] at 0 dB, 1 at +6 dB and above.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a position in 0..=1 fits an f32"
)]
pub fn fader_position(db: f64) -> f32 {
    let gain = 10f64.powf(db.min(FADER_TOP_DB) / 20.0);
    (f64::from(FADER_UNITY) * gain.powf(fader_exponent())).clamp(0.0, 1.0) as f32
}

/// The inverse of [`fader_position`].
pub fn fader_db(position: f32) -> f64 {
    if position <= 0.0 {
        return f64::NEG_INFINITY;
    }
    let gain = (f64::from(position.min(1.0)) / f64::from(FADER_UNITY)).powf(1.0 / fader_exponent());
    (20.0 * gain.log10()).min(FADER_TOP_DB)
}

/// Bottom LED first, on the fader's own scale so that the dB marks beside the fader read the ladder too: each LED covers an equal stretch of fader travel, lights once `db` is above its bottom edge (the bottom one from [`FADER_FLOOR_DB`]) and takes its band from its top edge.
pub fn strip_ladder(db: f64) -> [Segment; STRIP_LADDER_SEGMENTS] {
    let edge_db = |index: usize| fader_db(index as f32 / STRIP_LADDER_SEGMENTS as f32);
    std::array::from_fn(|index| {
        let top_db = edge_db(index + 1);
        let band = if top_db < LADDER_YELLOW_DB {
            Band::Green
        } else if top_db < LADDER_RED_DB {
            Band::Yellow
        } else {
            Band::Red
        };
        Segment {
            lit: db > edge_db(index).max(FADER_FLOOR_DB),
            band,
        }
    })
}

/// Each side's largest peak, held for [`MIXER_PEAK_HOLD_FOR`], then the current one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoPeakHold {
    held_db: [f64; 2],
    since: [Option<Instant>; 2],
}

impl Default for StereoPeakHold {
    fn default() -> StereoPeakHold {
        StereoPeakHold {
            held_db: [SILENCE_DB; 2],
            since: [None; 2],
        }
    }
}

impl StereoPeakHold {
    pub fn observe(&mut self, peak_db: [f64; 2], now: Instant) {
        let sides = self.held_db.iter_mut().zip(&mut self.since).zip(peak_db);
        for ((held, since), peak) in sides {
            let expired = since
                .is_none_or(|since| now.saturating_duration_since(since) >= MIXER_PEAK_HOLD_FOR);
            if expired || peak >= *held {
                *held = peak;
                *since = Some(now);
            }
        }
    }

    pub fn held_db(&self) -> [f64; 2] {
        self.held_db
    }
}

/// A Send knob's position: 0 when off, else as the fader.
pub fn send_fraction(level_db: Option<f64>) -> f32 {
    level_db.map_or(0.0, fader_position)
}

/// The Send level a knob position sends: off below [`FADER_FLOOR_DB`].
#[allow(
    clippy::cast_possible_truncation,
    reason = "a level between -60 and 6 dB fits an f32"
)]
pub fn send_level(fraction: f32) -> SendLevel {
    let db = fader_db(fraction);
    if db < FADER_FLOOR_DB {
        SendLevel::Off
    } else {
        SendLevel::Db(db as f32)
    }
}

pub fn send_text(level_db: Option<f64>) -> String {
    level_db.map_or_else(|| "off".to_string(), |db| format!("{db:.1} dB"))
}

pub const FADER_MIN_HEIGHT: f32 = 134.0;
pub const FADER_MAX_HEIGHT: f32 = 320.0;
pub const SEND_ROW_HEIGHT: f32 = 34.0;
/// The Send rows a strip shows however short the mixer is; past them the Sends scroll inside the strip.
pub const SEND_ROWS_MIN: usize = 2;

/// A strip's fader height and the Send rows it shows before they scroll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripFit {
    pub fader_height: f32,
    pub send_rows: usize,
}

/// How a strip spends `height`, what its fixed rows leave: the fader [`FADER_MIN_HEIGHT`] and up to [`SEND_ROWS_MIN`] Send rows first, then a Send row per Bus, then the fader the rest up to [`FADER_MAX_HEIGHT`]. The fader stays at its minimum until every Send row shows, so that a growing mixer never shrinks it. Below that minimum the strip keeps it, and the mixer scrolls.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a whole count of rows, floored at 0, fits a usize"
)]
pub fn strip_fit(height: f32, buses: usize) -> StripFit {
    let rows_room = ((height - FADER_MIN_HEIGHT) / SEND_ROW_HEIGHT)
        .floor()
        .max(0.0) as usize;
    let send_rows = rows_room.max(SEND_ROWS_MIN).min(buses);
    let fader_height = if send_rows < buses {
        FADER_MIN_HEIGHT
    } else {
        (height - send_rows as f32 * SEND_ROW_HEIGHT).clamp(FADER_MIN_HEIGHT, FADER_MAX_HEIGHT)
    };
    StripFit {
        fader_height,
        send_rows,
    }
}

/// A strip's outer width, edge included, at the least and at the most.
pub const STRIP_MIN_WIDTH: f32 = 125.0;
pub const STRIP_MAX_WIDTH: f32 = 160.0;

/// The strips' outer width and how many show whole beside Master.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripRow {
    pub width: f32,
    pub shown: usize,
}

/// Strips as wide as fills `width`, the row of strips and Master less the rule between them, with whole strips each followed by `gap`: `strips` of them, the Add Bus slot counted, or as many as fit at least [`STRIP_MIN_WIDTH`] wide, at most [`STRIP_MAX_WIDTH`]; one strip beside Master however narrow.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a whole count of strips, floored at 2, fits a usize and an f32"
)]
pub fn strip_row(width: f32, gap: f32, strips: usize) -> StripRow {
    let most = (width / (STRIP_MIN_WIDTH + gap))
        .floor()
        .min(strips as f32 + 1.0)
        .max(2.0);
    let filling = width / most - gap;
    let strip_width = filling.clamp(STRIP_MIN_WIDTH, STRIP_MAX_WIDTH);
    let slots = if (STRIP_MIN_WIDTH..=STRIP_MAX_WIDTH).contains(&filling) {
        most
    } else {
        (width / (strip_width + gap))
            .floor()
            .min(strips as f32 + 1.0)
            .max(2.0)
    } as usize;
    StripRow {
        width: strip_width,
        shown: slots - 1,
    }
}

/// The selectors of a Rack request for the Rack of `rack`; none for the Daemon's own choice (the selected Track, or the one Rack with no Project open).
pub fn rack_selectors(rack: Option<MixerTarget>) -> (Option<TrackRef>, Option<BusRef>, bool) {
    match rack {
        None => (None, None, false),
        Some(MixerTarget::Track(index)) => (Some(TrackRef::Index(index)), None, false),
        Some(MixerTarget::Bus(index)) => (None, Some(BusRef::Index(index)), false),
        Some(MixerTarget::Master) => (None, None, true),
    }
}

/// The Rack the window shows, by what stays the same while Tracks and Buses come and go: the selected Track's (a Track's column selects it), a Bus by its status `id`, or Master's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShownRack {
    Track,
    Bus(u64),
    Master,
}

impl ShownRack {
    /// The Rack of a mixer column of `status`.
    pub fn of(status: &Value, target: MixerTarget) -> Option<ShownRack> {
        match target {
            MixerTarget::Track(_) => Some(ShownRack::Track),
            MixerTarget::Bus(index) => status["buses"][index]["id"].as_u64().map(ShownRack::Bus),
            MixerTarget::Master => Some(ShownRack::Master),
        }
    }

    /// Its column in `status` now; `None` once its Bus is gone, or with no Track selected.
    pub fn column(self, status: &Value) -> Option<MixerTarget> {
        match self {
            ShownRack::Track => selected_track(status).map(MixerTarget::Track),
            ShownRack::Bus(id) => entries(&status["buses"])
                .iter()
                .position(|bus| bus["id"].as_u64() == Some(id))
                .map(MixerTarget::Bus),
            ShownRack::Master => Some(MixerTarget::Master),
        }
    }
}

fn selected_track(status: &Value) -> Option<usize> {
    track_rows(status)
        .into_iter()
        .find(|row| row.selected)
        .map(|row| row.index)
}

/// The column whose Rack the window shows: the one `shown` names while it is in `status`, else the selected Track's.
pub fn shown_rack_column(shown: Option<ShownRack>, status: &Value) -> Option<MixerTarget> {
    shown
        .and_then(|shown| shown.column(status))
        .or_else(|| ShownRack::Track.column(status))
}

/// What the window knows of the Rack it shows: the Rack a mixer strip named (`None` for the selected Track's), the selected Track (`None` with no Project open), and the Bus ids in column order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RackFocus {
    pub shown: Option<ShownRack>,
    pub selected: Option<usize>,
    pub bus_ids: Vec<u64>,
}

impl RackFocus {
    /// The focus over `status`; a shown Rack whose Bus is gone from it is closed.
    pub fn new(shown: Option<ShownRack>, status: Option<&Value>) -> RackFocus {
        let Some(status) = status else {
            return RackFocus::default();
        };
        RackFocus {
            shown: shown.filter(|shown| shown.column(status).is_some()),
            selected: selected_track(status),
            bus_ids: entries(&status["buses"])
                .iter()
                .filter_map(|bus| bus["id"].as_u64())
                .collect(),
        }
    }

    /// The Rack that Rack requests act on: the one a strip named, else the selected Track's.
    pub fn target(&self) -> Option<ShownRack> {
        self.shown.or(self.selected.map(|_| ShownRack::Track))
    }

    fn follow(&mut self, command: &Command) {
        match command {
            Command::ShowRack(target) => self.shown = Some(*target),
            Command::TrackSelect(index) => {
                self.selected = Some(*index);
                if self.shown.is_some() {
                    self.shown = Some(ShownRack::Track);
                }
            }
            Command::BusRemove(removed) => {
                if *removed < self.bus_ids.len() {
                    let id = self.bus_ids.remove(*removed);
                    if self.shown == Some(ShownRack::Bus(id)) {
                        self.shown = None;
                    }
                }
            }
            Command::ProjectNew(_)
            | Command::ProjectOpen(_)
            | Command::ProjectClose
            | Command::RestoreAutosave(_) => self.shown = None,
            _ => {}
        }
    }
}

/// `commands` with a [`Command::ShowRack`] of the window's Rack before each Rack request and wherever a command moves it, so that the client thread aims at the Rack the window shows; and the focus after them.
pub fn aim_rack_commands(
    mut focus: RackFocus,
    commands: Vec<Command>,
) -> (Vec<Command>, RackFocus) {
    let mut aimed = Vec::new();
    let mut named = None;
    for command in commands {
        let target = focus.target();
        if acts_on_rack(&command)
            && let Some(target) = target
            && named != Some(target)
        {
            aimed.push(Command::ShowRack(target));
            named = Some(target);
        }
        focus.follow(&command);
        let shows = matches!(command, Command::ShowRack(_));
        aimed.push(command);
        if shows {
            named = focus.target();
        } else if focus.target() != target
            && let Some(moved) = focus.target()
        {
            aimed.push(Command::ShowRack(moved));
            named = Some(moved);
        }
    }
    (aimed, focus)
}

/// The Render dialog's state from `status` `render`: none yet, running at a progress from 0 to 1, or how the last one ended.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderView {
    Idle,
    Running(f64),
    Done(String),
    Failed(String),
    Cancelled,
}

/// A Render the dialog sent: when, and the status `render` it is to replace.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderSent {
    pub at: Instant,
    pub previous: Value,
}

/// `refused` is the last `render` request the Daemon refused, which never reaches status; after `sent` it is shown, and the Render from before `sent` is not.
pub fn render_view(
    status: &Value,
    refused: Option<&(String, Instant)>,
    sent: Option<&RenderSent>,
) -> RenderView {
    let render = &status["render"];
    if let Some(sent) = sent {
        if let Some((error, at)) = refused
            && *at >= sent.at
        {
            return RenderView::Failed(error.clone());
        }
        if *render == sent.previous {
            return RenderView::Idle;
        }
    }
    let text = |key: &str| render[key].as_str().unwrap_or_default().to_string();
    match render["state"].as_str() {
        Some("running") => RenderView::Running(render["progress"].as_f64().unwrap_or_default()),
        Some("done") => RenderView::Done(text("file")),
        Some("failed") => RenderView::Failed(text("error")),
        Some("cancelled") => RenderView::Cancelled,
        _ => RenderView::Idle,
    }
}

/// The open Project's folder name from a `project_list` payload, which a Render names its file after by default.
pub fn default_render_name(projects: Option<&Value>, status: &Value) -> Option<String> {
    let name = project_name_payload(status)?;
    entries(&projects?["projects"])
        .iter()
        .find(|project| project["name"].as_str() == Some(name))
        .and_then(|project| project["folder"].as_str())
        .map(str::to_string)
}
