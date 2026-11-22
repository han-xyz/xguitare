use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use xguitare_client::{ClientError, TakesCleanStopped};
use xguitare_protocol::{
    BankKind, BusRef, GetReply, ItemsReply, LoopRegion, Position, RenderBits, RenderRange, Request,
    Response, SILENCE_DB, SendLevel, StatusReply, TrackRef, TransportAction, is_hidden_char,
    shown_file_name, to_payload,
};

use crate::mixer_model::{MixerTarget, ShownRack, rack_selectors};
use crate::project_model::{AutosaveOffer, MainView};
use crate::theme::{self, FOCUS_RING_GAP, FOCUS_RING_WIDTH, Rgb, palette};

pub const SPAWN_AFTER_FAILURES: u32 = 3;
pub const SPAWN_BACKOFF: Duration = Duration::from_secs(5);
pub const RESPAWN_FIRST_DELAY: Duration = Duration::from_secs(5);
pub const RESPAWN_MAX_DELAY: Duration = Duration::from_secs(60);
pub const METER_FLOOR_DB: f64 = -60.0;
pub const SET_INTERVAL: Duration = Duration::from_millis(30);
pub const FEEDBACK_FOR: Duration = Duration::from_secs(3);
pub const INVALID_PRESET: &str = "invalid preset";
pub const LOAD_ONLY_INTO_TRACK: &str = "a Preset loads only into a Track";
pub const KNOB_DRAG_PX: f32 = 220.0;
pub const KNOB_FINE_DRAG_PX: f32 = 2200.0;
pub const KNOB_ARC_START_DEG: f32 = 135.0;
pub const KNOB_ARC_SWEEP_DEG: f32 = 270.0;
pub const LADDER_SEGMENTS: usize = 24;
pub const LADDER_YELLOW_DB: f64 = -12.0;
pub const LADDER_RED_DB: f64 = -3.0;
pub const EAR_WIDTH: f32 = 30.0;
pub const EAR_HOLE_RADIUS: f32 = 4.0;
pub const EAR_HOLE_INSET: f32 = 10.0;
pub const CLIP_DB: f64 = 0.0;
pub const PEAK_HOLD_FOR: Duration = Duration::from_secs(2);
pub const CENTS_SPAN: i64 = 50;
pub const IN_TUNE_CENTS: i64 = 3;
pub const NO_SIGNAL: &str = "no signal";
pub const KNOB_KEY_STEP: f64 = 0.01;
pub const KNOB_PAGE_STEP: f64 = 0.1;
pub const AUTOSCROLL_EDGE_PX: f32 = 40.0;
pub const AUTOSCROLL_MAX_PX: f32 = 12.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Connected,
    Disconnected {
        since: Instant,
    },
    Starting {
        since: Instant,
    },
    /// A Daemon this window started exited before it ever answered: its exit status, its last non-empty stderr line as written, and when the window starts it again.
    Exited {
        status: String,
        last_line: Option<String>,
        retry_at: Instant,
    },
}

/// What the client thread last saw; the payloads are the Control API's `status`, `list`, `get`, `tuner`, `ports`, `settings`, `project_list` and `items` replies as sent, `module_types` the `modules` reply of the current connection, `irs` the last `ir_list` reply's names, `autosave_offer` the Project whose `project_open` answered that its autosave is newer.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub link: Link,
    pub status: Option<StatusReply>,
    pub bank: Option<Value>,
    pub params: Option<GetReply>,
    pub tuner: Option<Value>,
    pub ports: Option<Value>,
    pub settings: Option<Value>,
    pub projects: Option<Value>,
    pub items: Option<ItemsReply>,
    /// The peak table of each active Take of the open Project's Items fetched so far, keyed by Take file, as `take_peaks` pages it.
    pub peaks: HashMap<String, Arc<[i8]>>,
    pub module_types: Vec<ModuleType>,
    pub irs: Vec<String>,
    pub last_message: Option<(Message, Instant)>,
    pub failed_set: Option<(String, Instant)>,
    pub refused_remove: Option<(String, Instant)>,
    /// The error of the last `render` the Daemon refused.
    pub refused_render: Option<(String, Instant)>,
    pub autosave_offer: Option<(AutosaveOffer, Instant)>,
    /// The last `takes_unused` reply for the open Project: its payload or its error.
    pub unused_takes: Option<(Result<Value, String>, Instant)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Load(String),
    Set {
        param: String,
        value: f64,
    },
    Save,
    /// Saves the live Rack as a new Preset with this name.
    SaveAs(String),
    RackAdd {
        type_name: String,
        after: Option<String>,
        at: Option<usize>,
    },
    RackRemove {
        id: String,
    },
    RackMove {
        id: String,
        to: usize,
    },
    Mute(bool),
    Bypass {
        id: String,
        on: bool,
    },
    RackIr {
        id: String,
        ir: String,
    },
    /// Undo of a Remove: see [`Outgoing::Restore`].
    Restore(Restore),
    /// Sends nothing itself: the client thread fetches `ir_list` again on its next tick.
    RefreshIrs,
    SetPorts {
        input: String,
        output_left: String,
        output_right: String,
    },
    /// Sends nothing itself: the client thread fetches `ports` again on its next tick.
    RefreshPorts,
    SetSettings {
        bank_kind: Option<BankKind>,
        ir_dir: Option<PathBuf>,
        projects_dir: Option<PathBuf>,
    },
    ProjectNew(String),
    ProjectOpen(String),
    /// Closes the open Project and opens it again from its autosave: see [`Outgoing::RestoreAutosave`].
    RestoreAutosave(String),
    ProjectSave,
    ProjectClose,
    ProjectUndo,
    ProjectRedo,
    /// Moves the Items `ids` by `delta` frames; with `track`, the first of `ids` goes to that Track index and the others keep their distance in Tracks to it.
    ItemMove {
        ids: Vec<u64>,
        delta: i64,
        track: Option<usize>,
    },
    /// Splits at the frame `at` the Items `ids`, or without them those under `at` on the selected Track.
    ItemSplit {
        ids: Option<Vec<u64>>,
        at: u64,
    },
    ItemDelete(Vec<u64>),
    /// Makes the Take at index `take` the one the Item `id` plays.
    ItemTake {
        id: u64,
        take: usize,
    },
    ItemDuplicate(Vec<u64>),
    ItemCopy(Vec<u64>),
    /// Pastes the Daemon's clipboard with its first Item on the Track at index `track`, starting at the frame `at`.
    ItemPaste {
        track: usize,
        at: u64,
    },
    /// Moves the Item `id`'s start or end, or both, to those timeline frames.
    ItemTrim {
        id: u64,
        start: Option<u64>,
        end: Option<u64>,
    },
    /// Sends nothing itself: the client thread fetches `project_list` again on its next tick.
    RefreshProjects,
    TrackAdd,
    TrackSet {
        track: usize,
        edit: TrackEdit,
    },
    /// Also the Track that later Rack commands in the same batch act on.
    TrackSelect(usize),
    Transport(TransportAction),
    Locate(u64),
    Loop(LoopRegion),
    Tempo {
        bpm: Option<f64>,
        meter: Option<(u32, u32)>,
    },
    Click {
        on: Option<bool>,
        count_in: Option<u32>,
    },
    /// The `metronome` keys to change; `None` keeps the value.
    Metronome {
        on: Option<bool>,
        bpm: Option<f64>,
        meter: Option<(u32, u32)>,
        volume: Option<f64>,
    },
    MetronomeTap,
    BusAdd,
    BusRemove(usize),
    /// The `bus_set` keys to change; `None` keeps the value.
    BusSet {
        bus: usize,
        volume: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
    },
    SendSet {
        track: usize,
        bus: usize,
        level: Option<SendLevel>,
        pre: Option<bool>,
    },
    MasterSet(f32),
    ClipReset(MixerTarget),
    /// Sends nothing itself: the Rack requests after it, and the client thread's `get`, act on this Rack; a Track stands for the selected one.
    ShowRack(ShownRack),
    Render {
        range: RenderRange,
        bits: RenderBits,
        name: Option<String>,
    },
    RenderCancel,
    /// Kept by the window: shows one of the main views.
    ShowView(MainView),
    /// Kept by the window: shows the mixer, or the view before it while it shows.
    ToggleMixer,
    /// Kept by the window: opens the Render dialog.
    OpenRenderDialog,
    /// Kept by the window: opens the Clean up Takes dialog, which asks for [`Command::RefreshUnusedTakes`].
    OpenCleanTakesDialog,
    /// Sends nothing itself: the client thread reads `takes_unused` into [`Snapshot::unused_takes`].
    RefreshUnusedTakes,
    /// Deletes these unused Take files.
    CleanTakes(Vec<String>),
    Quit,
}

/// The `track_set` keys to change; `None` keeps the value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackEdit {
    pub input: Option<String>,
    pub volume: Option<f32>,
    pub pan: Option<f32>,
    pub mute: Option<bool>,
    pub solo: Option<bool>,
    pub arm: Option<bool>,
    pub name: Option<String>,
    pub to_master: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleType {
    pub type_name: String,
    pub channels: String,
}

impl ModuleType {
    pub fn label(&self) -> String {
        format!("{} ({})", self.type_name, self.channels)
    }
}

pub fn ir_names(ir_list_payload: &Value) -> Vec<String> {
    ir_list_payload["irs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect()
}

/// The Cabinet IR name of Module `id` in a `get` reply, read from its first entry.
pub fn cabinet_ir(get: &GetReply, id: &str) -> Option<String> {
    get.params
        .iter()
        .find(|entry| entry.module == id)?
        .ir
        .clone()
        .flatten()
}

/// Whether Module `id` in a `get` reply is bypassed, read from its first entry.
pub fn bypassed(get: &GetReply, id: &str) -> bool {
    get.params
        .iter()
        .find(|entry| entry.module == id)
        .is_some_and(|entry| entry.bypass)
}

pub fn module_types(modules_payload: &Value) -> Vec<ModuleType> {
    modules_payload["modules"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|module| ModuleType {
            type_name: module["type"].as_str().unwrap_or_default().to_string(),
            channels: module["channels"].as_str().unwrap_or_default().to_string(),
        })
        .collect()
}

/// `dy_px` is the pointer's vertical motion in screen pixels, positive downward as egui reports it, so dragging up raises the value; [`KNOB_DRAG_PX`] crosses the whole range, [`KNOB_FINE_DRAG_PX`] when `fine`.
pub fn knob_value_from_drag(
    value: f32,
    min: f32,
    max: f32,
    dy_px: f32,
    fine: bool,
    integer: bool,
) -> f32 {
    let px_per_range = if fine {
        KNOB_FINE_DRAG_PX
    } else {
        KNOB_DRAG_PX
    };
    let moved = (value - dy_px * (max - min) / px_per_range).clamp(min, max);
    if integer { moved.round() } else { moved }
}

/// Where `value` sits in `min..=max`, clamped to 0..=1; 0 for an empty range.
pub fn knob_fraction(value: f32, min: f32, max: f32) -> f32 {
    let range = max - min;
    if range <= 0.0 {
        return 0.0;
    }
    ((value - min) / range).clamp(0.0, 1.0)
}

/// Clockwise degrees in egui's y-down screen space: 135° is lower left, 270° the top, 405° lower right.
pub fn knob_angle_deg(fraction: f32) -> f32 {
    KNOB_ARC_START_DEG + KNOB_ARC_SWEEP_DEG * fraction
}

/// Offsets from the knob's centre in egui's y-down space: the pointer tip at `radius`.
#[derive(Debug, Clone, PartialEq)]
pub struct KnobPoints {
    pub pointer: (f32, f32),
}

pub fn knob_points(fraction: f32, radius: f32) -> KnobPoints {
    KnobPoints {
        pointer: point_at_deg(radius, knob_angle_deg(fraction)),
    }
}

/// A knob's two arcs as start and end angles in [`knob_angle_deg`]'s degrees: the track over the whole sweep, and the value arc from `origin` (a fraction, see [`knob_origin`]) to the value, `None` when the value sits at the origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnobArcs {
    pub track: (f32, f32),
    pub value: Option<(f32, f32)>,
}

#[allow(
    clippy::float_cmp,
    reason = "a knob at its origin is shown the exact origin fraction"
)]
pub fn knob_arcs(fraction: f32, origin: f32) -> KnobArcs {
    let fraction = fraction.clamp(0.0, 1.0);
    KnobArcs {
        track: (knob_angle_deg(0.0), knob_angle_deg(1.0)),
        value: (fraction != origin).then(|| (knob_angle_deg(origin), knob_angle_deg(fraction))),
    }
}

/// Where a knob's value arc starts, as a fraction of its sweep: the middle for a bipolar Parameter (a range symmetric around a 0 default, such as pan), else the start.
#[allow(clippy::float_cmp, reason = "ranges and defaults are exact constants")]
pub fn knob_origin(min: f64, max: f64, default: f64) -> f32 {
    if min == -max && default == 0.0 {
        0.5
    } else {
        0.0
    }
}

/// The centre lines of the dark gap band around a focused widget and of the focus ring outside it.
pub fn focus_ring_rects(widget: ScreenRect) -> (ScreenRect, ScreenRect) {
    let grown = |by: f32| ScreenRect {
        left: widget.left - by,
        top: widget.top - by,
        right: widget.right + by,
        bottom: widget.bottom + by,
    };
    (
        grown(FOCUS_RING_GAP / 2.0),
        grown(FOCUS_RING_GAP + FOCUS_RING_WIDTH / 2.0),
    )
}

fn point_at_deg(radius: f32, deg: f32) -> (f32, f32) {
    let rad = deg.to_radians();
    (radius * rad.cos(), radius * rad.sin())
}

/// The value a [`ParamRow`]'s knob at `fraction` stands for: the Parameter default on `reset`, whole numbers for an integer Parameter.
pub fn knob_param_value(row: &ParamRow, fraction: f32, reset: bool) -> f64 {
    if reset {
        return row.default;
    }
    let value = row.min + f64::from(fraction) * (row.max - row.min);
    if row.integer { value.round() } else { value }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnobKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// The value a focused knob's `key` moves `value` to: ↑/↓ [`KNOB_KEY_STEP`] of the range, PgUp/PgDn [`KNOB_PAGE_STEP`], an integer Parameter at least 1; Home/End the ends.
pub fn key_step(spec: &ParamRow, value: f64, key: KnobKey) -> f64 {
    let range = spec.max - spec.min;
    let step = |fraction: f64| {
        if spec.integer {
            (range * fraction).round().max(1.0)
        } else {
            range * fraction
        }
    };
    let next = match key {
        KnobKey::Up => value + step(KNOB_KEY_STEP),
        KnobKey::Down => value - step(KNOB_KEY_STEP),
        KnobKey::PageUp => value + step(KNOB_PAGE_STEP),
        KnobKey::PageDown => value - step(KNOB_PAGE_STEP),
        KnobKey::Home => spec.min,
        KnobKey::End => spec.max,
    };
    next.clamp(spec.min, spec.max)
}

/// A number typed into a knob's field: a Unicode minus reads as `-`, a trailing `k` or `kHz` (any case) multiplies by 1000, another trailing unit such as `dB` or `%` is ignored.
pub fn parse_typed_value(text: &str) -> Option<f64> {
    let text = text.replace('\u{2212}', "-");
    let text = text.trim();
    let number = text.trim_end_matches(|c: char| c.is_alphabetic() || c == '%');
    let unit = text[number.len()..].to_lowercase();
    let scale = if unit == "k" || unit == "khz" {
        1000.0
    } else {
        1.0
    };
    let value = number.trim_end().parse::<f64>().ok()?;
    Some(value * scale).filter(|value| value.is_finite())
}

/// How far to scroll the Rack while a unit or a Module tile is dragged with the pointer at `pointer_y` in a scroll area from `top` to `bottom`: negative up, positive down, growing to [`AUTOSCROLL_MAX_PX`] as the pointer reaches an edge within [`AUTOSCROLL_EDGE_PX`]; 0 elsewhere.
pub fn autoscroll_delta(pointer_y: f32, top: f32, bottom: f32) -> f32 {
    let speed = |depth: f32| AUTOSCROLL_MAX_PX * (depth / AUTOSCROLL_EDGE_PX).clamp(0.0, 1.0);
    let from_top = pointer_y - top;
    let from_bottom = bottom - pointer_y;
    if from_top < AUTOSCROLL_EDGE_PX {
        -speed(AUTOSCROLL_EDGE_PX - from_top)
    } else if from_bottom < AUTOSCROLL_EDGE_PX {
        speed(AUTOSCROLL_EDGE_PX - from_bottom)
    } else {
        0.0
    }
}

/// Latches once a peak reaches [`CLIP_DB`] and stays lit until [`ClipLatch::reset`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClipLatch {
    clipped: bool,
}

impl ClipLatch {
    pub fn observe(&mut self, peak_db: f64) {
        self.clipped |= peak_db >= CLIP_DB;
    }

    pub fn is_clipped(&self) -> bool {
        self.clipped
    }

    pub fn reset(&mut self) {
        self.clipped = false;
    }
}

/// The gap a dragged unit or Module tile is dropped into, 0 the front to `unit_edges.len() - 1` the end: the number of units whose middle is above `pointer_y`. `unit_edges` holds each unit's top edge in Rack order and, last, the last unit's bottom edge.
pub fn drop_slot(pointer_y: f32, unit_edges: &[f32]) -> usize {
    unit_edges
        .windows(2)
        .filter(|edges| f32::midpoint(edges[0], edges[1]) < pointer_y)
        .count()
}

/// Where a Module tile dropped into gap `slot` of the Rack `ids` goes.
pub fn palette_place(slot: usize, ids: &[String]) -> AddPlace {
    slot.checked_sub(1)
        .and_then(|above| ids.get(above))
        .map_or(AddPlace::Front, |id| AddPlace::After(id.clone()))
}

/// What is being dragged: the Module unit at a Rack position, a Module tile of the left panel by its type, a fixed unit in the Rack, or a fixed unit's chip of the left panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Dragged {
    Unit(usize),
    Tile(String),
    Fixed(FixedUnit),
    FixedChip(FixedUnit),
}

/// A rectangle in screen pixels: the window, or the part of it where a released unit stays in the Rack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl ScreenRect {
    pub fn contains(&self, (x, y): (f32, f32)) -> bool {
        (self.left..=self.right).contains(&x) && (self.top..=self.bottom).contains(&y)
    }
}

/// The command a drag released at `pointer` sends: inside the Rack `area` a unit moves to the [`drop_slot`] and a tile is added there; elsewhere in the `window` a unit is removed and a tile does nothing. A release outside the window, or with no pointer position (`None`), or beside the unit's own place sends nothing.
pub fn drag_outcome(
    dragged: &Dragged,
    pointer: Option<(f32, f32)>,
    window: &ScreenRect,
    area: &ScreenRect,
    unit_edges: &[f32],
    order: &[RackItem],
) -> Option<Command> {
    let pointer = pointer.filter(|pointer| window.contains(*pointer))?;
    let ids: Vec<String> = order.iter().filter_map(RackItem::module_id).collect();
    let modules_above = |slot: usize| {
        order[..slot.min(order.len())]
            .iter()
            .filter(|item| item.module_id().is_some())
            .count()
    };
    let slot = area
        .contains(pointer)
        .then(|| modules_above(drop_slot(pointer.1, unit_edges)));
    match (dragged, slot) {
        (Dragged::Unit(from), None) => Some(Command::RackRemove {
            id: ids[*from].clone(),
        }),
        (Dragged::Unit(from), Some(slot)) => {
            let to = if slot > *from { slot - 1 } else { slot };
            (to != *from).then(|| Command::RackMove {
                id: ids[*from].clone(),
                to,
            })
        }
        (Dragged::Tile(type_name), Some(slot)) => {
            Some(add_command(type_name, &palette_place(slot, &ids)))
        }
        (Dragged::Tile(_), None) | (Dragged::Fixed(_) | Dragged::FixedChip(_), _) => None,
    }
}

/// A Rack unit that is not a Module: its place is window layout only, in no Preset and not in the Control API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedUnit {
    Meters,
    Tuner,
}

impl FixedUnit {
    /// In the order of their chips in the left panel, as the Rack draws them by default.
    pub const ALL: [FixedUnit; 2] = [FixedUnit::Meters, FixedUnit::Tuner];

    pub fn label(self) -> &'static str {
        match self {
            FixedUnit::Meters => "Meters",
            FixedUnit::Tuner => "Tuner",
        }
    }

    /// As the footer's sentence names it.
    pub fn in_prose(self) -> &'static str {
        match self {
            FixedUnit::Meters => "meters",
            FixedUnit::Tuner => "Tuner",
        }
    }

    fn stored(self) -> &'static str {
        match self {
            FixedUnit::Meters => "meters",
            FixedUnit::Tuner => "tuner",
        }
    }
}

/// One unit of the Rack as drawn, top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RackItem {
    Fixed(FixedUnit),
    Module(String),
}

impl RackItem {
    pub fn module_id(&self) -> Option<String> {
        match self {
            RackItem::Fixed(_) => None,
            RackItem::Module(id) => Some(id.clone()),
        }
    }
}

/// Where the fixed units sit among the Modules of every Rack the window shows; a hidden fixed unit is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RackLayout {
    /// Each shown unit with its slot, the number of Modules drawn above it; a slot past the Rack's end draws at the end. The order of the entries orders fixed units drawn in the same gap.
    shown: Vec<(FixedUnit, usize)>,
}

impl Default for RackLayout {
    fn default() -> RackLayout {
        RackLayout {
            shown: vec![(FixedUnit::Meters, 0), (FixedUnit::Tuner, 0)],
        }
    }
}

impl RackLayout {
    /// The units drawn for the Module `ids`.
    pub fn order(&self, ids: &[String]) -> Vec<RackItem> {
        let fixed_at = |gap: usize| {
            self.shown
                .iter()
                .filter(move |(_, slot)| (*slot).min(ids.len()) == gap)
                .map(|(unit, _)| RackItem::Fixed(*unit))
        };
        let mut order = Vec::with_capacity(ids.len() + self.shown.len());
        for (gap, id) in ids.iter().enumerate() {
            order.extend(fixed_at(gap));
            order.push(RackItem::Module(id.clone()));
        }
        order.extend(fixed_at(ids.len()));
        order
    }

    pub fn is_shown(&self, unit: FixedUnit) -> bool {
        self.shown.iter().any(|(shown, _)| *shown == unit)
    }

    /// Moves `unit`, or shows it when hidden, into gap `gap` of the units drawn for `ids`: 0 the front, the unit count the end. The other fixed unit keeps its slot.
    pub fn place(&mut self, unit: FixedUnit, gap: usize, ids: &[String]) {
        let mut order = self.order(ids);
        let item = RackItem::Fixed(unit);
        let gap = match order.iter().position(|drawn| *drawn == item) {
            Some(from) => {
                order.remove(from);
                if gap > from { gap - 1 } else { gap }
            }
            None => gap,
        };
        let above = &order[..gap.min(order.len())];
        let slot = above
            .iter()
            .filter(|drawn| drawn.module_id().is_some())
            .count();
        let fixed_above = above.len() - slot;
        self.shown.retain(|(shown, _)| *shown != unit);
        let index = fixed_above.min(self.shown.len());
        self.shown.insert(index, (unit, slot));
    }

    /// Moves a shown `unit` one unit up or down past whatever is drawn next to it.
    pub fn step(&mut self, unit: FixedUnit, direction: Direction, ids: &[String]) {
        let order = self.order(ids);
        let Some(index) = order
            .iter()
            .position(|drawn| *drawn == RackItem::Fixed(unit))
        else {
            return;
        };
        let Some(to) = move_target(index, direction, order.len()) else {
            return;
        };
        let gap = match direction {
            Direction::Up => to,
            Direction::Down => to + 1,
        };
        self.place(unit, gap, ids);
    }

    /// `None` when `unit` is already hidden.
    pub fn hide(&mut self, unit: FixedUnit, now: Instant) -> Option<HiddenFixed> {
        let index = self.shown.iter().position(|(shown, _)| *shown == unit)?;
        let (_, slot) = self.shown.remove(index);
        Some(HiddenFixed {
            unit,
            slot,
            index,
            since: now,
        })
    }

    pub fn undo_hide(&mut self, hidden: &HiddenFixed) {
        if self.is_shown(hidden.unit) {
            return;
        }
        let index = hidden.index.min(self.shown.len());
        self.shown.insert(index, (hidden.unit, hidden.slot));
    }

    /// After the Rack's Module ids change from `previous` to `ids`: one Module added, removed or moved shifts each slot so that its unit keeps its neighbours (a removed Module's unit goes before the next one); any other change, another Preset or another Rack, keeps the slots.
    pub fn follow(&mut self, previous: &[String], ids: &[String]) {
        if let Some(edit) = rack_edit(previous, ids) {
            for (_, slot) in &mut self.shown {
                *slot = shifted(*slot, edit);
            }
        }
    }

    /// The layout as kept in eframe storage.
    pub fn to_stored(&self) -> String {
        let entries: Vec<Value> = self
            .shown
            .iter()
            .map(|(unit, slot)| serde_json::json!({"unit": unit.stored(), "slot": slot}))
            .collect();
        Value::Array(entries).to_string()
    }

    /// A stored string that does not parse, from an older or edited file, reads as the default layout.
    pub fn from_stored(stored: &str) -> RackLayout {
        RackLayout::parse_stored(stored).unwrap_or_default()
    }

    fn parse_stored(stored: &str) -> Option<RackLayout> {
        let entries: Vec<Value> = serde_json::from_str(stored).ok()?;
        let mut layout = RackLayout { shown: Vec::new() };
        for entry in &entries {
            let unit = FixedUnit::ALL
                .into_iter()
                .find(|unit| entry["unit"] == unit.stored())?;
            let slot = usize::try_from(entry["slot"].as_u64()?).ok()?;
            if layout.is_shown(unit) {
                return None;
            }
            layout.shown.push((unit, slot));
        }
        Some(layout)
    }
}

/// One Module added at, removed from, or moved between Rack positions.
#[derive(Debug, Clone, Copy)]
enum RackEdit {
    Insert(usize),
    Remove(usize),
    Move(usize, usize),
}

/// The single edit that turns `previous` into `ids`; `None` for no change or anything more.
fn rack_edit(previous: &[String], ids: &[String]) -> Option<RackEdit> {
    let first_difference = previous
        .iter()
        .zip(ids)
        .position(|(was, is)| was != is)
        .unwrap_or(previous.len().min(ids.len()));
    if ids.len() == previous.len() + 1 {
        let mut added = previous.to_vec();
        added.insert(first_difference, ids[first_difference].clone());
        return (added == ids).then_some(RackEdit::Insert(first_difference));
    }
    if previous.len() == ids.len() + 1 {
        let mut removed = previous.to_vec();
        removed.remove(first_difference);
        return (removed == ids).then_some(RackEdit::Remove(first_difference));
    }
    if previous.len() != ids.len() || previous == ids {
        return None;
    }
    (0..previous.len()).find_map(|from| {
        let mut moved = previous.to_vec();
        let id = moved.remove(from);
        let to = ids.iter().position(|is| *is == id)?;
        moved.insert(to, id);
        (moved == ids).then_some(RackEdit::Move(from, to))
    })
}

/// A slot after `edit`: a unit keeps the Module below it, and the front stays the front.
fn shifted(slot: usize, edit: RackEdit) -> usize {
    match edit {
        RackEdit::Remove(at) if slot > at => slot - 1,
        RackEdit::Insert(at) if slot >= at && slot > 0 => slot + 1,
        RackEdit::Move(from, to) => {
            shifted(shifted(slot, RackEdit::Remove(from)), RackEdit::Insert(to))
        }
        RackEdit::Remove(_) | RackEdit::Insert(_) => slot,
    }
}

/// A fixed unit the GUI hid, offered for [`UNDO_FOR`] to go back where it was.
#[derive(Debug, Clone, PartialEq)]
pub struct HiddenFixed {
    pub unit: FixedUnit,
    slot: usize,
    index: usize,
    since: Instant,
}

impl HiddenFixed {
    /// As [`RackLayout::follow`], so that Undo puts the unit back between the same Modules.
    pub fn follow(&mut self, previous: &[String], ids: &[String]) {
        if let Some(edit) = rack_edit(previous, ids) {
            self.slot = shifted(self.slot, edit);
        }
    }

    pub fn is_shown(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.since) < UNDO_FOR
    }
}

/// What a fixed unit's drag does to the [`RackLayout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDrop {
    /// Into a gap of the units drawn, as [`RackLayout::place`] takes it.
    Place(FixedUnit, usize),
    Hide(FixedUnit),
}

/// A fixed unit or its chip released inside the Rack `area` goes to the [`drop_slot`]; a fixed unit released elsewhere in the `window` hides. A Module unit or tile, a chip outside the Rack, and a release outside the window do nothing here.
pub fn layout_drop(
    dragged: &Dragged,
    pointer: Option<(f32, f32)>,
    window: &ScreenRect,
    area: &ScreenRect,
    unit_edges: &[f32],
) -> Option<LayoutDrop> {
    let pointer = pointer.filter(|pointer| window.contains(*pointer))?;
    let slot = area
        .contains(pointer)
        .then(|| drop_slot(pointer.1, unit_edges));
    match (dragged, slot) {
        (Dragged::Fixed(unit) | Dragged::FixedChip(unit), Some(slot)) => {
            Some(LayoutDrop::Place(*unit, slot))
        }
        (Dragged::Fixed(unit), None) => Some(LayoutDrop::Hide(*unit)),
        _ => None,
    }
}

/// What Undo puts back of a removed Module, from the `get` payload shown before its Remove: its type, the Module above it (`None` at the front), its position `at`, its Parameter values, its bypass and its Cabinet IR.
#[derive(Debug, Clone, PartialEq)]
pub struct Restore {
    pub type_name: String,
    pub after: Option<String>,
    pub at: usize,
    pub values: Vec<(String, f64)>,
    pub bypass: bool,
    pub ir: Option<String>,
    /// The Track the Module was removed from; `None` for the Preset's Rack or a Bus or Master Rack.
    pub track: Option<TrackRef>,
    /// The Bus the Module was removed from.
    pub bus: Option<BusRef>,
    /// Whether the Module was removed from the Master Rack.
    pub master: bool,
}

/// The [`Restore`] of Module `id`, for the Rack `get` shows; `None` when the reply has no such Module.
pub fn restore(get: &GetReply, id: &str) -> Option<Restore> {
    let modules = param_rows(get);
    let index = modules.iter().position(|(module, _)| module == id)?;
    let rows = &modules[index].1;
    Some(Restore {
        type_name: rows
            .first()
            .map_or("", |row| row.module_type.as_str())
            .to_string(),
        after: index.checked_sub(1).map(|above| modules[above].0.clone()),
        at: index,
        values: rows
            .iter()
            .map(|row| (row.param.clone(), row.value))
            .collect(),
        bypass: bypassed(get, id),
        ir: cabinet_ir(get, id),
        track: None,
        bus: None,
        master: false,
    })
}

impl Restore {
    /// The `rack_add` that puts the Module back after the Module that was above it, or at its old position `at` when that one is not among the `present` Module ids.
    pub fn add_request(&self, present: &[String]) -> Request {
        let after = self.after.clone().filter(|after| present.contains(after));
        let at = after.is_none().then_some(self.at);
        Request::RackAdd {
            r#type: self.type_name.clone(),
            after,
            at,
            track: self.track.clone(),
            bus: self.bus.clone(),
            master: self.master,
        }
    }

    /// What gives the Module the `rack_add` answered with `id` its values, its bypass and its Cabinet IR.
    pub fn follow_up_requests(&self, id: &str) -> Vec<Request> {
        let sets = self.values.iter().map(|(param, value)| Request::Set {
            param: format!("{id}.{param}"),
            value: *value,
            track: self.track.clone(),
            bus: self.bus.clone(),
            master: self.master,
        });
        let bypass = self.bypass.then(|| Request::Bypass {
            id: id.to_string(),
            on: true,
            track: self.track.clone(),
            bus: self.bus.clone(),
            master: self.master,
        });
        let ir = self.ir.clone().map(|ir| Request::RackIr {
            ir,
            id: Some(id.to_string()),
            track: self.track.clone(),
            bus: self.bus.clone(),
            master: self.master,
        });
        sets.chain(bypass).chain(ir).collect()
    }
}

pub const UNDO_FOR: Duration = Duration::from_secs(10);

/// A Module the GUI removed, with its [`Restore`], offered for [`UNDO_FOR`] while the Preset it was removed from stays active.
#[derive(Debug, Clone, PartialEq)]
pub struct RemovedUnit {
    pub id: String,
    pub restore: Restore,
    preset: (Option<String>, Option<usize>),
    since: Instant,
}

impl RemovedUnit {
    /// `None` when `get` has no Module `id`, so that there is nothing to undo. While a Project is open the Restore names the selected Track, so that Undo goes back to it whichever Track is selected then.
    pub fn new(
        get: &GetReply,
        status: Option<&StatusReply>,
        id: &str,
        now: Instant,
    ) -> Option<RemovedUnit> {
        Some(RemovedUnit {
            id: id.to_string(),
            restore: Restore {
                track: selected_track(status),
                ..restore(get, id)?
            },
            preset: preset_of(status),
            since: now,
        })
    }

    pub fn is_shown(&self, status: Option<&StatusReply>, now: Instant) -> bool {
        now.saturating_duration_since(self.since) < UNDO_FOR && preset_of(status) == self.preset
    }
}

/// The Undo offer to show now; an offer not shown, because its time ran out or the active Preset changed, is dropped for good, so that it never comes back when that Preset does.
pub fn shown_undo<'a>(
    removed: &'a mut Option<RemovedUnit>,
    status: Option<&StatusReply>,
    now: Instant,
) -> Option<&'a RemovedUnit> {
    if removed
        .as_ref()
        .is_some_and(|removed| !removed.is_shown(status, now))
    {
        *removed = None;
    }
    removed.as_ref()
}

/// The `status` payload's Preset name and program; nulls without a status.
fn selected_track(status: Option<&StatusReply>) -> Option<TrackRef> {
    let status = status.filter(|status| status.project.is_some())?;
    let name = &status.tracks.get(status.selected?)?.name;
    Some(TrackRef::Name(name.clone()))
}

fn preset_of(status: Option<&StatusReply>) -> (Option<String>, Option<usize>) {
    status.map_or((None, None), |status| {
        (status.preset.clone(), status.program)
    })
}

/// The peak text's value: the largest peak seen, held for [`PEAK_HOLD_FOR`], then the current one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakHold {
    held_db: f64,
    since: Option<Instant>,
}

impl Default for PeakHold {
    fn default() -> PeakHold {
        PeakHold {
            held_db: SILENCE_DB,
            since: None,
        }
    }
}

impl PeakHold {
    pub fn observe(&mut self, peak_db: f64, now: Instant) {
        let expired = self
            .since
            .is_none_or(|since| now.saturating_duration_since(since) >= PEAK_HOLD_FOR);
        if expired || peak_db >= self.held_db {
            self.held_db = peak_db;
            self.since = Some(now);
        }
    }

    pub fn held_db(&self) -> f64 {
        self.held_db
    }
}

/// The open Module ids as kept in eframe storage.
pub fn open_units_to_string(open: &BTreeSet<String>) -> String {
    serde_json::json!(open).to_string()
}

/// Notes the id of every [`Command::RackRemove`] among `commands` as waiting for the Daemon.
pub fn remember_removes(removing: &mut BTreeSet<String>, commands: &[Command]) {
    for command in commands {
        if let Command::RackRemove { id } = command {
            removing.insert(id.clone());
        }
    }
}

/// Settles every id in `removing` against the first `get` reply after its Remove: the open state of an id the reply no longer has is dropped, a Module still there means the Remove was refused and keeps its unit open. Another Preset lacking an id that was never removed keeps it.
pub fn forget_removed_units(
    open: &mut BTreeSet<String>,
    removing: &mut BTreeSet<String>,
    get: &GetReply,
) {
    let present: BTreeSet<String> = param_rows(get).into_iter().map(|(id, _)| id).collect();
    for id in std::mem::take(removing) {
        if !present.contains(&id) {
            open.remove(&id);
        }
    }
}

/// A stored string that does not parse, from an older or edited file, reads as no unit open.
pub fn open_units_from_string(stored: &str) -> BTreeSet<String> {
    serde_json::from_str(stored).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Band {
    Green,
    Yellow,
    Red,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub lit: bool,
    pub band: Band,
}

/// Bottom segment first; each covers an equal dB step of [`METER_FLOOR_DB`]..0, lights once `db` is above its bottom edge and takes its band from its top edge. `clipped` lights the top segment whatever `db` is.
pub fn ladder_segments(db: f64, clipped: bool) -> [Segment; LADDER_SEGMENTS] {
    let step_db = -METER_FLOOR_DB / LADDER_SEGMENTS as f64;
    let lit_count = ((db - METER_FLOOR_DB) / step_db)
        .ceil()
        .clamp(0.0, LADDER_SEGMENTS as f64) as usize;
    std::array::from_fn(|index| {
        let top_db = METER_FLOOR_DB + (index + 1) as f64 * step_db;
        let band = if top_db < LADDER_YELLOW_DB {
            Band::Green
        } else if top_db < LADDER_RED_DB {
            Band::Yellow
        } else {
            Band::Red
        };
        let is_top = index == LADDER_SEGMENTS - 1;
        Segment {
            lit: index < lit_count || (clipped && is_top),
            band,
        }
    })
}

/// Screw hole centres, top then bottom, as offsets from the top-left of an ear [`EAR_WIDTH`] wide and `height` tall.
pub fn ear_holes(height: f32) -> [(f32, f32); 2] {
    let inset = EAR_HOLE_INSET.min(height / 3.0);
    let x = EAR_WIDTH / 2.0;
    [(x, inset), (x, height - inset)]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Up,
    Down,
}

/// The Rack position each unit of `order` shows on its ears: Modules count 1-based, as `rack move` counts; a fixed unit shows none.
pub fn rack_ear_labels(order: &[RackItem]) -> Vec<Option<String>> {
    let mut position = 0;
    order
        .iter()
        .map(|item| {
            item.module_id().map(|_| {
                position += 1;
                position.to_string()
            })
        })
        .collect()
}

/// The Rack position a Module at `index` moves to; `None` when it is already at that end.
pub fn move_target(index: usize, direction: Direction, len: usize) -> Option<usize> {
    match direction {
        Direction::Up => index.checked_sub(1),
        Direction::Down => Some(index + 1).filter(|&to| to < len),
    }
}

pub const MONO: &str = "mono";
pub const STEREO: &str = "stereo";

/// Each Module id of the Rack with its type's `channels`, empty when `modules` does not list the type.
pub fn rack_channels(
    modules: &[(String, Vec<ParamRow>)],
    module_types: &[ModuleType],
) -> Vec<(String, String)> {
    modules
        .iter()
        .map(|(id, rows)| {
            let channels = rows
                .first()
                .and_then(|row| {
                    module_types
                        .iter()
                        .find(|module_type| module_type.type_name == row.module_type)
                })
                .map(|module_type| module_type.channels.clone())
                .unwrap_or_default();
            (id.clone(), channels)
        })
        .collect()
}

/// The id a new Module of `new_channels` is inserted after: a stereo one after the last Module, a mono one or the cabinet after the last mono Module; `None` is the front.
pub fn add_after(rack: &[(String, String)], new_channels: &str) -> Option<String> {
    let mut candidates = rack.iter().rev();
    let after = if new_channels == STEREO {
        candidates.next()
    } else {
        candidates.find(|(_, channels)| channels == MONO)
    };
    after.map(|(id, _)| id.clone())
}

/// Where the Add Module picker inserts a new Module.
#[derive(Debug, Clone, PartialEq)]
pub enum AddPlace {
    Front,
    After(String),
}

impl AddPlace {
    pub fn label(&self) -> String {
        match self {
            AddPlace::Front => "at the front".to_string(),
            AddPlace::After(id) => format!("after {id}"),
        }
    }
}

/// The front, then after each Module in Rack order.
pub fn add_places(rack: &[(String, String)]) -> Vec<AddPlace> {
    std::iter::once(AddPlace::Front)
        .chain(rack.iter().map(|(id, _)| AddPlace::After(id.clone())))
        .collect()
}

/// The `chosen` place while the Rack still offers it, else where [`add_after`] puts a Module of `new_channels`.
pub fn add_place(
    rack: &[(String, String)],
    new_channels: &str,
    chosen: Option<&AddPlace>,
) -> AddPlace {
    if let Some(chosen) = chosen.filter(|chosen| add_places(rack).contains(chosen)) {
        return chosen.clone();
    }
    add_after(rack, new_channels).map_or(AddPlace::Front, AddPlace::After)
}

/// The front is position 0, as `after: null` would append.
pub fn add_command(type_name: &str, place: &AddPlace) -> Command {
    let (after, at) = match place {
        AddPlace::Front => (None, Some(0)),
        AddPlace::After(id) => (Some(id.clone()), None),
    };
    Command::RackAdd {
        type_name: type_name.to_string(),
        after,
        at,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BankRow {
    pub program: usize,
    pub name: String,
    pub active: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyEvent {
    Digit(u8, KeyModifiers),
    CtrlS,
    CtrlR,
    CtrlM,
    CtrlAltR,
    Space,
    R,
    Home,
    End,
    M,
    T,
    S,
    /// Delete or Backspace.
    Delete,
    CtrlZ,
    CtrlShiftZ,
    CtrlD,
    /// Ctrl+C, or the copy event it becomes.
    CtrlC,
    /// Ctrl+V, or the paste event it becomes.
    CtrlV,
    Left,
    Right,
    AltUp,
    AltDown,
}

/// Digits and Ctrl+S act unless a text field has focus; a focused knob consumes only its own arrow, page and Home/End keys.
pub fn shortcuts_active(text_edit_focused: bool) -> bool {
    !text_edit_focused
}

/// The Bank rows of a `list` payload, and how many entries were skipped for lacking an integer `program`.
pub fn bank_rows(list_payload: &Value) -> (Vec<BankRow>, usize) {
    let active = list_payload["active"].as_u64();
    let entries = list_payload["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let rows: Vec<BankRow> = entries
        .iter()
        .filter_map(|entry| {
            let program = entry["program"].as_u64()?;
            let valid = entry["valid"].as_bool().unwrap_or_default();
            Some(BankRow {
                program: program as usize,
                name: entry["name"].as_str().unwrap_or_default().to_string(),
                active: active == Some(program),
                error: (!valid).then(|| {
                    entry["error"]
                        .as_str()
                        .unwrap_or(INVALID_PRESET)
                        .to_string()
                }),
            })
        })
        .collect();
    let skipped = entries.len() - rows.len();
    (rows, skipped)
}

/// `rows` as the shown Rack takes them: on a Bus or Master Rack (`stereo_rack`) each row is refused, as the Daemon refuses `load` there.
pub fn bank_rows_for_rack(rows: Vec<BankRow>, stereo_rack: bool) -> Vec<BankRow> {
    if !stereo_rack {
        return rows;
    }
    rows.into_iter()
        .map(|row| BankRow {
            error: row.error.or_else(|| Some(LOAD_ONLY_INTO_TRACK.to_string())),
            ..row
        })
        .collect()
}

/// The rows whose name contains `query` in any case or whose program number is `query`; every row for a blank query.
pub fn filter_bank(rows: &[BankRow], query: &str) -> Vec<BankRow> {
    let query = query.trim().to_lowercase();
    rows.iter()
        .filter(|row| {
            query.is_empty()
                || row.name.to_lowercase().contains(&query)
                || row.program.to_string() == query
        })
        .cloned()
        .collect()
}

pub fn key_to_command(key: KeyEvent, rows: &[BankRow]) -> Option<Command> {
    match key {
        KeyEvent::CtrlS => Some(Command::Save),
        KeyEvent::Digit(_, KeyModifiers { ctrl: true, .. } | KeyModifiers { alt: true, .. }) => {
            None
        }
        KeyEvent::Digit(digit @ 1..=9, _) => {
            let program = usize::from(digit - 1);
            rows.iter()
                .find(|row| row.program == program && row.error.is_none())
                .map(|row| Command::Load(row.program.to_string()))
        }
        KeyEvent::Digit(..)
        | KeyEvent::CtrlR
        | KeyEvent::CtrlM
        | KeyEvent::CtrlAltR
        | KeyEvent::Space
        | KeyEvent::R
        | KeyEvent::Home
        | KeyEvent::End
        | KeyEvent::M
        | KeyEvent::T
        | KeyEvent::S
        | KeyEvent::Delete
        | KeyEvent::CtrlZ
        | KeyEvent::CtrlShiftZ
        | KeyEvent::CtrlD
        | KeyEvent::CtrlC
        | KeyEvent::CtrlV
        | KeyEvent::Left
        | KeyEvent::Right
        | KeyEvent::AltUp
        | KeyEvent::AltDown => None,
    }
}

/// After a Daemon this window started exited before answering, it is started again only at the `retry_at` [`respawn_delay`] set.
pub fn may_start_daemon(link: &Link, now: Instant) -> bool {
    match link {
        Link::Exited { retry_at, .. } => now >= *retry_at,
        Link::Connected | Link::Disconnected { .. } | Link::Starting { .. } => true,
    }
}

/// The pause before starting the Daemon again after its `early_exits`-th exit in a row before answering: doubling from [`RESPAWN_FIRST_DELAY`] up to [`RESPAWN_MAX_DELAY`].
pub fn respawn_delay(early_exits: u32) -> Duration {
    RESPAWN_FIRST_DELAY
        .saturating_mul(1 << early_exits.saturating_sub(1).min(4))
        .min(RESPAWN_MAX_DELAY)
}

pub fn spawn_policy(failures: u32, last_spawn: Option<Instant>, now: Instant) -> bool {
    failures >= SPAWN_AFTER_FAILURES
        && last_spawn.is_none_or(|spawned| now.duration_since(spawned) >= SPAWN_BACKOFF)
}

/// Only a missing socket counts: a Daemon that does not answer may still be alive, and an insecure socket directory fails a spawned Daemon too.
pub fn counts_toward_spawn(err: &ClientError) -> bool {
    matches!(err, ClientError::NotRunning(_))
}

/// What the client thread sends for a [`Command`]: one request, or a [`Restore`]: its `rack_add`, then, only once that succeeds, its follow-ups to the id the Daemon answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    Request(Request),
    Restore(Restore),
    /// `project_close`, then `project_open` with `restore`, sent again while the Daemon answers [`STILL_CLOSING`](xguitare_protocol::STILL_CLOSING).
    RestoreAutosave(String),
}

/// `rack` is the Rack the Rack requests act on while a Project is open, `None` without one; a [`Command::TrackSelect`] or [`Command::ShowRack`] moves it for the commands after it, a shown Bus found in `status` by its id (the Rack requests for one that is gone are dropped) and a shown Track's Rack left to the Daemon's selected Track.
#[allow(clippy::too_many_lines, reason = "one match arm per queued command")]
pub fn requests_until_quit(
    queued: Vec<Command>,
    rack: Option<MixerTarget>,
    status: Option<&StatusReply>,
) -> (Vec<Outgoing>, bool) {
    let mut requests = Vec::new();
    let mut rack = rack;
    let mut shown_bus_gone = false;
    for command in queued {
        if shown_bus_gone && acts_on_rack(&command) {
            continue;
        }
        let (track, bus, master) = rack_selectors(rack);
        let req = match command {
            Command::Load(preset) => Request::Load {
                preset,
                track,
                bus,
                master,
            },
            Command::Set { param, value } => Request::Set {
                param,
                value,
                track,
                bus,
                master,
            },
            Command::Save => Request::Save { name: None },
            Command::SaveAs(name) => Request::Save { name: Some(name) },
            Command::RackAdd {
                type_name,
                after,
                at,
            } => Request::RackAdd {
                r#type: type_name,
                after,
                at,
                track,
                bus,
                master,
            },
            Command::RackRemove { id } => Request::RackRemove {
                id,
                track,
                bus,
                master,
            },
            Command::RackMove { id, to } => Request::RackMove {
                id,
                to,
                track,
                bus,
                master,
            },
            Command::Mute(on) => Request::Mute { on },
            Command::Bypass { id, on } => Request::Bypass {
                id,
                on,
                track,
                bus,
                master,
            },
            Command::RackIr { id, ir } => Request::RackIr {
                ir,
                id: Some(id),
                track,
                bus,
                master,
            },
            Command::SetPorts {
                input,
                output_left,
                output_right,
            } => Request::SetPorts {
                input,
                output_left,
                output_right,
            },
            Command::SetSettings {
                bank_kind,
                ir_dir,
                projects_dir,
            } => Request::SetSettings {
                bank_kind,
                ir_dir,
                projects_dir,
            },
            Command::ProjectNew(name) => Request::ProjectNew { name },
            Command::ProjectOpen(name) => Request::ProjectOpen {
                name,
                restore: false,
            },
            Command::ProjectSave => Request::ProjectSave,
            Command::ProjectClose => Request::ProjectClose,
            Command::ProjectUndo => Request::ProjectUndo,
            Command::ProjectRedo => Request::ProjectRedo,
            Command::CleanTakes(files) => Request::TakesClean { files },
            Command::ItemMove { ids, delta, track } => Request::ItemMove {
                ids,
                delta,
                track: track.map(TrackRef::Index),
            },
            Command::ItemSplit { ids, at } => Request::ItemSplit { ids, at },
            Command::ItemDelete(ids) => Request::ItemDelete { ids },
            Command::ItemTrim { id, start, end } => Request::ItemTrim { id, start, end },
            Command::ItemTake { id, take } => Request::ItemTake { id, take },
            Command::ItemDuplicate(ids) => Request::ItemDuplicate { ids },
            Command::ItemCopy(ids) => Request::ItemCopy { ids },
            Command::ItemPaste { track, at } => Request::ItemPaste {
                track: TrackRef::Index(track),
                at,
            },
            Command::TrackAdd => Request::TrackAdd { name: None },
            Command::TrackSet { track, edit } => track_set_request(track, edit),
            Command::TrackSelect(index) => {
                rack = Some(MixerTarget::Track(index));
                shown_bus_gone = false;
                Request::TrackSelect {
                    track: TrackRef::Index(index),
                }
            }
            Command::Transport(action) => Request::Transport {
                action,
                position: None,
            },
            Command::Locate(frames) => Request::Transport {
                action: TransportAction::Locate,
                position: Some(Position::Frames(frames)),
            },
            Command::Loop(region) => Request::Loop {
                start: region.start,
                end: region.end,
                on: region.on,
            },
            Command::Tempo { bpm, meter } => Request::Tempo { bpm, meter },
            Command::Click { on, count_in } => Request::Click { on, count_in },
            Command::Metronome {
                on,
                bpm,
                meter,
                volume,
            } => Request::Metronome {
                on,
                bpm,
                meter,
                volume,
            },
            Command::MetronomeTap => Request::MetronomeTap,
            Command::BusAdd => Request::BusAdd { name: None },
            Command::BusRemove(index) => Request::BusRemove {
                bus: BusRef::Index(index),
            },
            Command::BusSet {
                bus,
                volume,
                pan,
                mute,
                solo,
            } => Request::BusSet {
                bus: BusRef::Index(bus),
                name: None,
                volume,
                pan,
                mute,
                solo,
            },
            Command::SendSet {
                track,
                bus,
                level,
                pre,
            } => Request::SendSet {
                track: TrackRef::Index(track),
                bus: BusRef::Index(bus),
                level,
                pre,
            },
            Command::MasterSet(volume) => Request::MasterSet {
                volume: SendLevel::Db(volume),
            },
            Command::ClipReset(target) => {
                let (track, bus, master) = rack_selectors(Some(target));
                Request::ClipReset { track, bus, master }
            }
            Command::Render { range, bits, name } => Request::Render {
                range: Some(range),
                bits: Some(bits),
                name,
            },
            Command::RenderCancel => Request::RenderCancel,
            Command::ShowRack(shown) => {
                let column = status.and_then(|status| shown.column(&to_payload(status)));
                shown_bus_gone = matches!(shown, ShownRack::Bus(_)) && column.is_none();
                rack = column.filter(|target| !matches!(target, MixerTarget::Track(_)));
                continue;
            }
            Command::Restore(restore) => {
                requests.push(Outgoing::Restore(restore));
                continue;
            }
            Command::RestoreAutosave(name) => {
                requests.push(Outgoing::RestoreAutosave(name));
                continue;
            }
            Command::RefreshIrs
            | Command::RefreshPorts
            | Command::RefreshProjects
            | Command::ShowView(_)
            | Command::ToggleMixer
            | Command::OpenRenderDialog
            | Command::OpenCleanTakesDialog
            | Command::RefreshUnusedTakes => continue,
            Command::Quit => return (requests, true),
        };
        requests.push(Outgoing::Request(req));
    }
    (requests, false)
}

/// Whether `command` is a request on the shown Rack.
pub(crate) fn acts_on_rack(command: &Command) -> bool {
    matches!(
        command,
        Command::Load(_)
            | Command::Set { .. }
            | Command::RackAdd { .. }
            | Command::RackRemove { .. }
            | Command::RackMove { .. }
            | Command::Bypass { .. }
            | Command::RackIr { .. }
    )
}

fn track_set_request(track: usize, edit: TrackEdit) -> Request {
    Request::TrackSet {
        track: TrackRef::Index(track),
        input: edit.input,
        volume: edit.volume,
        pan: edit.pan,
        mute: edit.mute,
        solo: edit.solo,
        arm: edit.arm,
        name: edit.name,
        to_master: edit.to_master,
    }
}

/// Leaving with the Tuner open undoes the Mute it turned on before the client thread stops.
pub fn quit_commands(tuner_open: bool, muted_by_us: bool) -> Vec<Command> {
    if tuner_open && muted_by_us {
        vec![Command::Mute(false), Command::Quit]
    } else {
        vec![Command::Quit]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TunerReading {
    pub note: String,
    pub hz: f64,
    pub cents: i64,
}

/// The reading of a `tuner` payload; `None` unless it is confident.
pub fn tuner_reading(payload: &Value) -> Option<TunerReading> {
    if payload["confident"].as_bool() != Some(true) {
        return None;
    }
    Some(TunerReading {
        note: payload["note"].as_str()?.to_string(),
        hz: payload["hz"].as_f64()?,
        cents: payload["cents"].as_i64()?,
    })
}

pub fn cents_fraction(cents: i64) -> f32 {
    (cents as f32 / CENTS_SPAN as f32).clamp(-1.0, 1.0)
}

pub fn cents_text(cents: i64) -> String {
    format!("{cents:+} ct").replace('-', "\u{2212}")
}

pub fn tuner_header_text(reading: Option<&TunerReading>) -> String {
    match reading {
        Some(reading) => format!("{}  {}", reading.note, cents_text(reading.cents)),
        None => NO_SIGNAL.to_string(),
    }
}

pub fn in_tune(cents: i64) -> bool {
    cents.abs() <= IN_TUNE_CENTS
}

/// Opens or closes the Tuner view: (open after, command to send, whether the Mute is ours after). Opening mutes when `mute_default` and Mute is off (`muted_now` false); closing unmutes only a Mute this view turned on.
#[allow(
    clippy::fn_params_excessive_bools,
    reason = "four independent states, matched as one truth table"
)]
pub fn tuner_toggle(
    open_now: bool,
    mute_default: bool,
    muted_by_us: bool,
    muted_now: bool,
) -> (bool, Option<Command>, bool) {
    match (open_now, mute_default && !muted_now, muted_by_us) {
        (false, true, _) => (true, Some(Command::Mute(true)), true),
        (false, false, _) => (true, None, false),
        (true, _, true) => (false, Some(Command::Mute(false)), false),
        (true, _, false) => (false, None, false),
    }
}

pub const BYPASSED_TAG: &str = "Bypassed";

/// `arc` replaces the Module accent on the unit's knobs and lit choices when `Some`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitStyle {
    pub panel: Rgb,
    pub text: Rgb,
    pub arc: Option<Rgb>,
    pub lamp_lit: bool,
    pub tag: Option<&'static str>,
}

/// A unit while the Daemon is not connected: on the bypass panel in bypass text with grey arcs, its lamp and tag still telling its Bypass.
pub fn paused(style: UnitStyle) -> UnitStyle {
    let colours = palette();
    UnitStyle {
        panel: colours.bypass_panel,
        text: colours.bypass_text,
        arc: Some(colours.bypass_arc),
        ..style
    }
}

/// How a unit shows its Module's Bypass: a bypassed unit, open or closed, sits on the bypass panel in bypass text with grey arcs, an unlit lamp and a `Bypassed` tag.
pub fn unit_style(bypassed: bool, open: bool) -> UnitStyle {
    let colours = palette();
    if bypassed {
        return UnitStyle {
            panel: colours.bypass_panel,
            text: colours.bypass_text,
            arc: Some(colours.bypass_arc),
            lamp_lit: false,
            tag: Some(BYPASSED_TAG),
        };
    }
    UnitStyle {
        panel: if open {
            colours.panel_open
        } else {
            colours.panel
        },
        text: colours.text,
        arc: None,
        lamp_lit: true,
        tag: None,
    }
}

/// The banner text while the `status` payload says Mute is on; `muted_by_tuner` when the Tuner view turned it on, so that closing the view turns it off.
pub fn mute_banner(status: Option<&StatusReply>, muted_by_tuner: bool) -> Option<String> {
    let muted = status.is_some_and(|status| status.muted);
    let how = if muted_by_tuner {
        "close the Tuner"
    } else {
        "press Mute"
    };
    muted.then(|| format!("Output muted \u{2014} {how} to unmute"))
}

/// `held_db` is `None` while no status arrives, so the text shows no value.
pub fn peak_text(held_db: Option<f64>, clipped: bool) -> String {
    let Some(db) = held_db else {
        return "—".to_string();
    };
    let text = if clipped {
        format!("{db:+.1} dB, clipped")
    } else {
        format!("{db:+.1} dB")
    };
    text.replace('-', "\u{2212}")
}

pub fn refresh_decision(last_seen: Option<u64>, revision: u64) -> bool {
    last_seen != Some(revision)
}

/// A peak the payload lacks or carries as `null` (a non-finite value serialises as `null`) reads as −120 dB.
pub fn peak_db(peak: Option<f64>) -> f64 {
    peak.unwrap_or(SILENCE_DB)
}

/// How long one buffer of `buffer` frames lasts at `rate` Hz.
pub fn latency_ms(buffer: u64, rate: u64) -> f64 {
    buffer as f64 * 1000.0 / rate as f64
}

/// The sidebar's label and value rows for a `status` payload; only the Daemon row without one.
pub fn engine_readout(status: Option<&StatusReply>) -> Vec<(String, String)> {
    let Some(status) = status else {
        return vec![("Daemon".to_string(), "disconnected".to_string())];
    };
    let rate = u64::from(status.sample_rate);
    let buffer = u64::from(status.buffer_size);
    [
        ("Rate", format!("{} kHz", rate as f64 / 1000.0)),
        ("Buffer", format!("{buffer} frames")),
        ("Latency", format!("{:.1} ms", latency_ms(buffer, rate))),
        ("Xruns", status.xruns.to_string()),
        ("Daemon", "connected".to_string()),
    ]
    .into_iter()
    .map(|(label, value)| (label.to_string(), value))
    .collect()
}

/// What the banner says while the Daemon is not connected; the client thread retries on its own.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkBanner {
    pub title: &'static str,
    pub detail: String,
    /// A third line, under the detail.
    pub hint: Option<String>,
    /// Drawn as an error rather than as a wait.
    pub is_error: bool,
}

pub fn link_banner(link: &Link, now: Instant) -> Option<LinkBanner> {
    const PAUSED: &str = "Controls are paused.";
    let waiting = |title, detail| LinkBanner {
        title,
        detail,
        hint: None,
        is_error: false,
    };
    match link {
        Link::Connected => None,
        Link::Disconnected { since } => Some(waiting(
            "Daemon disconnected",
            match seconds_since(*since, now) {
                0 => format!("Reconnecting\u{2026} {PAUSED}"),
                seconds => format!("Reconnecting for {seconds} s. {PAUSED}"),
            },
        )),
        Link::Starting { since } => Some(waiting(
            "Starting the Daemon",
            match seconds_since(*since, now) {
                0 => format!("Waiting for it\u{2026} {PAUSED}"),
                seconds => format!("Waited {seconds} s so far. {PAUSED}"),
            },
        )),
        Link::Exited {
            status,
            last_line,
            retry_at,
        } => Some(LinkBanner {
            title: "The Daemon exited",
            detail: last_line
                .as_deref()
                .map(without_escapes)
                .filter(|line| !line.is_empty())
                .unwrap_or_else(|| status.clone()),
            hint: Some(format!(
                "{PAUSED} Starting it again {}; a Daemon you start yourself is picked up at once.",
                match seconds_until(*retry_at, now) {
                    0 => "now".to_string(),
                    seconds => format!("in {seconds} s"),
                }
            )),
            is_error: true,
        }),
    }
}

/// `line` without ANSI escape sequences (CSI, OSC and two-byte ones), control characters and bidi marks, overrides and isolates, trimmed.
fn without_escapes(line: &str) -> String {
    let mut shown = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    chars.find(|c| ('\u{40}'..='\u{7e}').contains(c));
                }
                Some(']') => {
                    let mut previous = None;
                    for c in chars.by_ref() {
                        if c == '\u{7}' || (previous == Some('\u{1b}') && c == '\\') {
                            break;
                        }
                        previous = Some(c);
                    }
                }
                _ => {}
            },
            c if is_hidden_char(c) || matches!(c, '\u{200e}' | '\u{200f}') => {}
            c => shown.push(c),
        }
    }
    shown.trim().to_string()
}

/// `text` when `width_of` it fits `max_width`, else its longest prefix that fits with an ellipsis after it, empty when not even that fits.
pub fn elide_to_width(text: &str, max_width: f32, width_of: impl Fn(&str) -> f32) -> String {
    if width_of(text) <= max_width {
        return text.to_string();
    }
    text.char_indices()
        .map(|(end, _)| format!("{}\u{2026}", &text[..end]))
        .take_while(|elided| width_of(elided) <= max_width)
        .last()
        .filter(|elided| elided.chars().count() > 1)
        .unwrap_or_default()
}

fn seconds_since(since: Instant, now: Instant) -> u64 {
    now.saturating_duration_since(since).as_secs()
}

fn seconds_until(until: Instant, now: Instant) -> u64 {
    let left = until.saturating_duration_since(now);
    left.as_secs() + u64::from(left.subsec_nanos() > 0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamRow {
    pub address: String,
    pub module: String,
    pub module_type: String,
    pub param: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: String,
    pub integer: bool,
    /// A choice Parameter's names for its values 0, 1, …; empty for a plain number.
    pub labels: Vec<String>,
}

/// Groups the `get` reply's `params` per Module, keeping Rack order.
pub fn param_rows(get: &GetReply) -> Vec<(String, Vec<ParamRow>)> {
    let mut modules: Vec<(String, Vec<ParamRow>)> = Vec::new();
    for param in &get.params {
        let row = ParamRow {
            address: format!("{}.{}", param.module, param.param),
            module: param.module.clone(),
            module_type: param.type_name.clone(),
            param: param.param.clone(),
            value: param.value,
            min: param.min,
            max: param.max,
            default: param.default,
            unit: param.unit.clone(),
            integer: param.integer,
            labels: param.labels.clone(),
        };
        match modules.last_mut() {
            Some((id, rows)) if *id == param.module => rows.push(row),
            _ => modules.push((param.module.clone(), vec![row])),
        }
    }
    modules
}

pub fn unit_suffix(unit: &str) -> &'static str {
    match unit {
        "dB" => " dB",
        "ms" => " ms",
        "Hz" => " Hz",
        "%" => " %",
        _ => "",
    }
}

/// The width one small knob takes in a closed unit, with its label, its value field and the gap after it.
pub const CLOSED_KNOB_WIDTH: f32 = 114.0;

/// The gap between a unit's header items, and between its ears and its content.
pub const HEADER_GAP: f32 = 10.0;
pub const UNIT_NAME_WIDTH: f32 = 90.0;
pub const FOOTSWITCH_WIDTH: f32 = 42.0;
/// The open/close triangle: a whole pointer target.
pub const CHEVRON_WIDTH: f32 = theme::CONTROL_HEIGHT;
pub const RULE_WIDTH: f32 = 1.0;
pub const MOVE_BUTTON_SIDE: f32 = 30.0;
pub const MOVE_BUTTON_GAP: f32 = 8.0;
pub const MOVE_BUTTONS_WIDTH: f32 = 2.0 * MOVE_BUTTON_SIDE + MOVE_BUTTON_GAP;
/// The `Bypassed` tag's place after the chevron, kept whatever the tag's text measures.
pub const TAG_SLOT_WIDTH: f32 = 72.0;
/// The widths of a closed unit's header items other than its knobs and the `Bypassed` tag, left to right: On/Off switch with its lamp, name, chevron, rule, ↑/↓.
const HEADER_FIXED_ITEMS: [f32; 5] = [
    FOOTSWITCH_WIDTH,
    UNIT_NAME_WIDTH,
    CHEVRON_WIDTH,
    RULE_WIDTH,
    MOVE_BUTTONS_WIDTH,
];

/// What a closed unit `unit_width` wide leaves its knobs: less its ears and margins, the fixed header items, the `Bypassed` tag when `bypassed`, and one header gap per item.
pub fn closed_knobs_width(unit_width: f32, bypassed: bool) -> f32 {
    let tag = bypassed.then_some(TAG_SLOT_WIDTH);
    let fixed: f32 = HEADER_FIXED_ITEMS
        .iter()
        .chain(&tag)
        .map(|width| width + HEADER_GAP)
        .sum();
    (unit_width - 2.0 * (EAR_WIDTH + HEADER_GAP) - fixed).max(0.0)
}

/// What an open unit `unit_width` wide leaves its knob rows: less its ears and margins, and less the Cabinet IR column at its right when `has_ir`.
pub fn open_knobs_width(unit_width: f32, has_ir: bool) -> f32 {
    let side_column = if has_ir {
        MOVE_BUTTONS_WIDTH + HEADER_GAP
    } else {
        0.0
    };
    (unit_width - 2.0 * (EAR_WIDTH + HEADER_GAP) - side_column).max(0.0)
}

/// Rows of small knobs a closed unit of `knob_count` Parameters needs in `width`: as many per row as fit, at least one per row and one row.
pub fn closed_unit_rows(knob_count: usize, width: f32) -> usize {
    let per_row = ((width / CLOSED_KNOB_WIDTH).floor() as usize).max(1);
    knob_count.div_ceil(per_row).max(1)
}

/// The height of an open unit with one row of large knobs, and the least any open unit has.
pub const OPEN_UNIT_HEIGHT: f32 = 168.0;
/// One large knob column with its label and value field, and the gap after it.
pub const OPEN_KNOB_PITCH: f32 = 80.0;
const OPEN_ROW_HEIGHT: f32 = 104.0;
const OPEN_ROW_GAP: f32 = 8.0;
const CHOICE_HEIGHT: f32 = 32.0;
const CHOICE_GAP: f32 = 4.0;
/// Up to this many choices stack in the column; more sit side by side.
const STACKED_CHOICES: usize = 2;

/// The Parameter groups an open unit of `type_name` keeps on one row, as counts in spec order; a type not listed is one group.
fn param_groups(type_name: &str) -> &'static [usize] {
    match type_name {
        "autowah" => &[2, 3, 3, 2],
        "blend" => &[3, 4, 3],
        _ => &[],
    }
}

/// Large knobs per row of an open unit `width` wide: whole groups while they fit, a new row before a group that does not; a group wider than a row fills rows of its own.
pub fn open_knob_rows(type_name: &str, knob_count: usize, width: f32) -> Vec<usize> {
    let per_row = ((width / OPEN_KNOB_PITCH).floor() as usize).max(1);
    let listed: usize = param_groups(type_name).iter().sum();
    let groups: Vec<usize> = if listed == knob_count {
        param_groups(type_name).to_vec()
    } else {
        vec![knob_count]
    };
    let mut rows = Vec::new();
    let mut current = 0;
    for group in groups.into_iter().filter(|&group| group > 0) {
        if current + group > per_row && current > 0 {
            rows.push(current);
            current = 0;
        }
        let mut left = group;
        while current + left > per_row {
            rows.push(per_row);
            left -= per_row;
        }
        current += left;
    }
    if current > 0 {
        rows.push(current);
    }
    rows
}

pub fn open_unit_height(rows: usize) -> f32 {
    OPEN_UNIT_HEIGHT + rows.saturating_sub(1) as f32 * (OPEN_ROW_HEIGHT + OPEN_ROW_GAP)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceButton {
    pub text: String,
    pub accessible_name: String,
    pub lit: bool,
    pub value: f64,
    /// Left, top, width and height within the knob column.
    pub cell: (f32, f32, f32, f32),
}

/// One button per label, filling a column `column_width` wide: two stack, more sit side by side.
pub fn choice_buttons(row: &ParamRow, column_width: f32) -> Vec<ChoiceButton> {
    let count = row.labels.len();
    let lit = chosen_index(row);
    let param = param_label(&row.param);
    row.labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let text = choice_text(label);
            let cell = if count <= STACKED_CHOICES {
                let top = index as f32 * (CHOICE_HEIGHT + CHOICE_GAP);
                (0.0, top, column_width, CHOICE_HEIGHT)
            } else {
                let width = column_width / count as f32;
                (index as f32 * width, 0.0, width, CHOICE_HEIGHT)
            };
            ChoiceButton {
                accessible_name: format!("{param}, {text}"),
                text,
                lit: index == lit,
                value: index as f64,
                cell,
            }
        })
        .collect()
}

/// Labels that are abbreviations, shown in capitals.
const ABBREVIATED_LABELS: [&str; 3] = ["lp", "bp", "hp"];

/// A label as the GUI shows it: an abbreviation in capitals (`lp` reads `LP`), anything else with a capital first letter (`up` reads `Up`).
pub fn choice_text(label: &str) -> String {
    if ABBREVIATED_LABELS.contains(&label) {
        return label.to_uppercase();
    }
    param_label(label)
}

fn chosen_index(row: &ParamRow) -> usize {
    (row.value.round().max(0.0) as usize).min(row.labels.len().saturating_sub(1))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChoiceKey {
    Previous,
    Next,
}

/// The value an arrow key moves a choice to; it stops at the first and the last.
pub fn choice_step(row: &ParamRow, key: ChoiceKey) -> f64 {
    let index = chosen_index(row);
    let moved = match key {
        ChoiceKey::Previous => index.saturating_sub(1),
        ChoiceKey::Next => (index + 1).min(row.labels.len().saturating_sub(1)),
    };
    moved as f64
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChipClick {
    Set(f64),
    Open,
}

/// A click on a closed unit's chip: two choices switch to the other one, more open the unit.
pub fn chip_click(row: &ParamRow) -> ChipClick {
    if row.labels.len() == STACKED_CHOICES {
        return ChipClick::Set((1 - chosen_index(row).min(1)) as f64);
    }
    ChipClick::Open
}

const KILO: f64 = 1000.0;
const RATIO_PARAM: &str = "ratio";
const Q_PARAM: &str = "q";
const Q_SUFFIX: &str = "_q";

/// A Parameter name in sentence case: `low_cut` reads `Low cut`.
pub fn param_label(param: &str) -> String {
    let words = param.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Module ids shown other than in sentence case.
const DISPLAY_NAMES: [(&str, &str); 1] = [("eq", "EQ")];

/// A unit's name: the Module id in sentence case (`eq` reads `EQ`), and its type when the two differ.
pub fn module_title<'a>(module: &str, type_name: &'a str) -> (String, Option<&'a str>) {
    let differs = !type_name.is_empty() && type_name != module;
    let name = DISPLAY_NAMES
        .iter()
        .find(|(id, _)| *id == module)
        .map_or_else(|| param_label(module), |(_, name)| (*name).to_string());
    (name, differs.then_some(type_name))
}

/// The value as a knob shows it: one decimal at most (two for a filter Q), none on a whole value, a Unicode minus, `kHz` from 1000 Hz, a ratio as `6 : 1`.
pub fn format_value(spec: &ParamRow, value: f64) -> String {
    if spec.param == Q_PARAM || spec.param.ends_with(Q_SUFFIX) {
        return hundredths_text(value);
    }
    if spec.param == RATIO_PARAM {
        return format!("{} : 1", number_text(value));
    }
    if spec.unit == "Hz" && value.abs() >= KILO {
        return format!("{} kHz", number_text(value / KILO));
    }
    format!("{}{}", number_text(value), unit_suffix(&spec.unit))
}

/// The number a value field holds while it is edited, in the Parameter's own unit, so that [`parse_typed_value`] reads it back.
pub fn typed_text(spec: &ParamRow, value: f64) -> String {
    if spec.integer {
        return number_text(value.round());
    }
    number_text(value)
}

/// The value typed into a value field.
///
/// # Errors
///
/// The error toast's detail `'<text>' is not a number` when `text` does not parse.
pub fn typed_value(text: &str) -> Result<f64, String> {
    parse_typed_value(text).ok_or_else(|| format!("'{}' is not a number", text.trim()))
}

/// Two decimals at most, trailing zeros dropped: `0.71`, `2.5`, `3`.
fn hundredths_text(value: f64) -> String {
    let text = format!("{:.2}", (value * 100.0).round() / 100.0);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.replace('-', "\u{2212}")
}

fn number_text(value: f64) -> String {
    let tenths = (value * 10.0).round();
    let text = if tenths == 0.0 {
        "0".to_string()
    } else if tenths % 10.0 == 0.0 {
        format!("{}", tenths / 10.0)
    } else {
        format!("{:.1}", tenths / 10.0)
    };
    text.replace('-', "\u{2212}")
}

/// The gap between Add Module chips, across and down.
pub const CHIP_GAP: f32 = theme::SPACE_XS;
/// The narrowest chip that still shows `compressor`, the longest Module type, beside its accent tab.
const MIN_CHIP_WIDTH: f32 = 68.0;

/// Three columns of Add Module chips when three fit `width`, else two.
pub fn chip_columns(width: f32) -> usize {
    let three = (width - 2.0 * CHIP_GAP) / 3.0;
    if three >= MIN_CHIP_WIDTH { 3 } else { 2 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Project,
    Preset,
    Bank,
    Rack,
    Tuner,
    Metronome,
    Audio,
    Settings,
    Engine,
}

/// The left panel's sections, top to bottom.
pub const SECTIONS: [Section; 9] = [
    Section::Project,
    Section::Preset,
    Section::Bank,
    Section::Metronome,
    Section::Rack,
    Section::Tuner,
    Section::Audio,
    Section::Settings,
    Section::Engine,
];

impl Section {
    pub fn title(self, bank_kind: BankKind) -> &'static str {
        match (self, bank_kind) {
            (Section::Project, _) => "Project",
            (Section::Preset, _) => "Preset",
            (Section::Bank, BankKind::Guitar) => "Guitar Bank",
            (Section::Bank, BankKind::Bass) => "Bass Bank",
            (Section::Rack, _) => "Add Module",
            (Section::Tuner, _) => "Tuner and Mute",
            (Section::Metronome, _) => "Metronome",
            (Section::Audio, _) => "Audio",
            (Section::Settings, _) => "Settings",
            (Section::Engine, _) => "Engine",
        }
    }

    /// Audio and Settings are set once and stay closed until opened.
    pub fn open_by_default(self) -> bool {
        !matches!(self, Section::Audio | Section::Settings)
    }
}

/// The Bank kind of a `settings` payload, `configured` before one arrives.
pub fn bank_kind(settings_payload: Option<&Value>, configured: BankKind) -> BankKind {
    settings_payload
        .and_then(|settings| serde_json::from_value(settings["bank_kind"].clone()).ok())
        .unwrap_or(configured)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PortSelection {
    pub input: Option<String>,
    pub output_left: Option<String>,
    pub output_right: Option<String>,
}

/// What the Audio pickers offer and show, from a `ports` payload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PortChoices {
    pub capture: Vec<String>,
    pub playback: Vec<String>,
    pub connected: PortSelection,
}

pub fn port_choices(ports_payload: &Value) -> PortChoices {
    let names = |key: &str| -> Vec<String> {
        ports_payload[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str().map(str::to_string))
            .collect()
    };
    let connected = |key: &str| ports_payload["connected"][key].as_str().map(str::to_string);
    PortChoices {
        capture: names("capture"),
        playback: names("playback"),
        connected: PortSelection {
            input: connected("input"),
            output_left: connected("output_left"),
            output_right: connected("output_right"),
        },
    }
}

/// `None` until all three ports are chosen: `set_ports` takes no empty name.
pub fn set_ports_command(selection: &PortSelection) -> Option<Command> {
    Some(Command::SetPorts {
        input: selection.input.clone()?,
        output_left: selection.output_left.clone()?,
        output_right: selection.output_right.clone()?,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsDraft {
    pub bank_kind: BankKind,
    pub ir_dir: String,
    pub projects_dir: String,
}

/// The Settings section's values from a `settings` payload; `None` when it lacks them.
pub fn settings_draft(settings_payload: &Value) -> Option<SettingsDraft> {
    Some(SettingsDraft {
        bank_kind: serde_json::from_value(settings_payload["bank_kind"].clone()).ok()?,
        ir_dir: settings_payload["ir_dir"].as_str()?.to_string(),
        projects_dir: settings_payload["projects_dir"].as_str()?.to_string(),
    })
}

/// A `set_settings` with only the keys `draft` changes from `current`; `None` when it changes none.
pub fn set_settings_command(current: &SettingsDraft, draft: &SettingsDraft) -> Option<Command> {
    let bank_kind = (draft.bank_kind != current.bank_kind).then_some(draft.bank_kind);
    let ir_dir = draft.ir_dir.trim();
    let ir_dir = (ir_dir != current.ir_dir).then(|| PathBuf::from(ir_dir));
    let projects_dir = draft.projects_dir.trim();
    let projects_dir = (projects_dir != current.projects_dir).then(|| PathBuf::from(projects_dir));
    if bank_kind.is_none() && ir_dir.is_none() && projects_dir.is_none() {
        return None;
    }
    Some(Command::SetSettings {
        bank_kind,
        ir_dir,
        projects_dir,
    })
}

#[derive(Debug, Default)]
struct Drag {
    last_sent: Option<Instant>,
    pending: Option<f64>,
    released_at: Option<u64>,
}

impl Drag {
    fn is_due(&self, now: Instant) -> bool {
        self.last_sent
            .is_none_or(|sent| now.duration_since(sent) >= SET_INTERVAL)
    }
}

/// Per Parameter address: while dragged, a `set` at most every [`SET_INTERVAL`], a throttled value sent by [`Throttle::tick`] once the interval has passed; the knob keeps its local value until a snapshot newer than the release Revision arrives.
#[derive(Debug, Default)]
pub struct Throttle {
    drags: HashMap<String, Drag>,
}

impl Throttle {
    pub fn on_drag(&mut self, address: &str, value: f64, now: Instant) -> Option<Command> {
        let drag = self.drags.entry(address.to_string()).or_default();
        drag.released_at = None;
        if !drag.is_due(now) {
            drag.pending = Some(value);
            return None;
        }
        drag.last_sent = Some(now);
        drag.pending = None;
        Some(set_command(address, value))
    }

    pub fn tick(&mut self, now: Instant) -> Vec<Command> {
        let mut due = Vec::new();
        for (address, drag) in &mut self.drags {
            if !drag.is_due(now) {
                continue;
            }
            if let Some(value) = drag.pending.take() {
                drag.last_sent = Some(now);
                due.push(set_command(address, value));
            }
        }
        due
    }

    pub fn has_pending(&self) -> bool {
        self.drags.values().any(|drag| drag.pending.is_some())
    }

    pub fn on_release(&mut self, address: &str, value: f64, revision: u64) -> Command {
        let drag = self.drags.entry(address.to_string()).or_default();
        drag.last_sent = None;
        drag.pending = None;
        drag.released_at = Some(revision);
        set_command(address, value)
    }

    pub fn hold_local(&self, address: &str, revision_now: u64) -> bool {
        self.drags.get(address).is_some_and(|drag| {
            drag.released_at
                .is_none_or(|released| revision_now <= released)
        })
    }

    pub fn forget(&mut self, address: &str) {
        self.drags.remove(address);
    }

    pub fn retain(&mut self, is_present: impl Fn(&str) -> bool) {
        self.drags.retain(|address, _| is_present(address));
    }
}

fn set_command(address: &str, value: f64) -> Command {
    Command::Set {
        param: address.to_string(),
        value,
    }
}

/// What the window tells about a command it sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A result for the footer, such as a saved path.
    Notice(String),
    /// A failure for the error toast: the action that failed, and the reason.
    Error { title: &'static str, detail: String },
}

/// The footer's notice for [`FEEDBACK_FOR`], and the error toast's error until it is dismissed or the next error replaces it.
#[derive(Debug, Default)]
pub struct Feedback {
    notice: Option<(String, Instant)>,
    error: Option<(&'static str, String)>,
}

impl Feedback {
    pub fn show(&mut self, message: Message, at: Instant) {
        match message {
            Message::Notice(text) => self.notice = Some((text, at)),
            Message::Error { title, detail } => self.error = Some((title, detail)),
        }
    }

    pub fn notice(&self, now: Instant) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) < FEEDBACK_FOR)
            .map(|(text, _)| text.as_str())
    }

    /// The error toast's title and detail.
    pub fn error(&self) -> Option<(&str, &str)> {
        self.error
            .as_ref()
            .map(|(title, detail)| (*title, detail.as_str()))
    }

    pub fn dismiss(&mut self) {
        self.error = None;
    }
}

/// Esc closes the error toast unless it first releases a focused widget, closes an open menu or closes a dialog.
pub fn esc_dismisses_toast(widget_focused: bool, popup_open: bool, dialog_open: bool) -> bool {
    !widget_focused && !popup_open && !dialog_open
}

/// Where the Render progress percentage, `text_width` wide, starts from the bar's left, and whether it sits on the lit part (`lit_width` wide) or on the well after it: never across the lit edge.
pub fn progress_text_place(lit_width: f32, text_width: f32) -> (f32, bool) {
    if lit_width >= text_width + 2.0 * theme::SPACE_S {
        (theme::SPACE_S, true)
    } else {
        (lit_width + theme::SPACE_S, false)
    }
}

/// The id of a `rack remove` that the Daemon refused or that never reached it.
pub fn refused_remove(req: &Request, response: &Result<Response, ClientError>) -> Option<String> {
    match (req, response) {
        (_, Ok(Response::Ok(_))) => None,
        (Request::RackRemove { id, .. }, _) => Some(id.clone()),
        _ => None,
    }
}

/// The error of a `render` that the Daemon refused or that never reached it, verbatim.
pub fn refused_render(req: &Request, response: &Result<Response, ClientError>) -> Option<String> {
    match (req, response) {
        (Request::Render { .. }, Ok(Response::Err { error, .. })) => Some(error.clone()),
        (Request::Render { .. }, Err(err)) => Some(err.to_string()),
        _ => None,
    }
}

/// The message for a command's response: a saved path, or an error titled by [`failure_title`]; other successes are silent.
pub fn response_message(req: &Request, response: Result<Response, ClientError>) -> Option<Message> {
    let detail = match (req, response) {
        (Request::Save { .. } | Request::ProjectSave, Ok(Response::Ok(payload))) => {
            return Some(Message::Notice(format!(
                "saved {}",
                payload["path"].as_str().unwrap_or_default()
            )));
        }
        (Request::TakesClean { .. }, Ok(Response::Ok(payload))) => {
            let deleted = payload["deleted"].as_u64().unwrap_or(0);
            let takes = if deleted == 1 { "Take" } else { "Takes" };
            let freed = size_text(payload["bytes"].as_u64().unwrap_or(0));
            let notice = format!("Deleted {deleted} {takes}, {freed} freed");
            let failed = payload["failed"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            let Some(first) = failed.first() else {
                return Some(Message::Notice(notice));
            };
            let more = match failed.len() - 1 {
                0 => String::new(),
                more => format!(", and {more} more"),
            };
            format!(
                "{notice}; could not delete {}: {}{more}",
                shown_file_name(first["file"].as_str().unwrap_or_default()),
                first["error"].as_str().unwrap_or_default()
            )
        }
        (Request::ProjectOpen { .. }, Ok(Response::Ok(payload))) => {
            let warnings: Vec<&str> = payload["warnings"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            return (!warnings.is_empty()).then(|| Message::Notice(warnings.join("; ")));
        }
        (_, Ok(Response::Ok(_))) => return None,
        (_, Ok(Response::Err { error, .. })) => error,
        (_, Err(err)) => err.to_string(),
    };
    Some(Message::Error {
        title: failure_title(req),
        detail,
    })
}

/// The message for a batched clean-up: [`response_message`]'s for the summed reply, and the error that stopped the batches after what they deleted.
pub fn takes_cleaned_message(
    req: &Request,
    cleaned: Result<Value, TakesCleanStopped>,
) -> Option<Message> {
    let (payload, error) = match cleaned {
        Ok(payload) => return response_message(req, Ok(Response::Ok(payload))),
        Err(TakesCleanStopped {
            cleaned: None,
            error,
        }) => return response_message(req, Ok(Response::err(error))),
        Err(TakesCleanStopped {
            cleaned: Some(payload),
            error,
        }) => (payload, error),
    };
    let detail = match response_message(req, Ok(Response::Ok(payload))) {
        Some(Message::Notice(text) | Message::Error { detail: text, .. }) => {
            format!("{text}; {error}")
        }
        None => error,
    };
    Some(Message::Error {
        title: failure_title(req),
        detail,
    })
}

/// A `takes_unused` reply as the Clean up Takes dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedTakes {
    /// The Take files as `takes_clean` names them.
    pub files: Vec<String>,
    /// Each file's name in `takes/` and its size.
    pub rows: Vec<(String, String)>,
    pub total: String,
}

pub fn unused_takes(reply: &Value) -> UnusedTakes {
    let entries = reply["files"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let (files, rows) = entries
        .iter()
        .filter_map(|entry| {
            let file = entry["file"].as_str()?;
            let name = file.strip_prefix("takes/").unwrap_or(file);
            let row = (shown_file_name(name), size_text(entry["bytes"].as_u64()?));
            Some((file.to_string(), row))
        })
        .unzip();
    UnusedTakes {
        files,
        rows,
        total: size_text(reply["bytes"].as_u64().unwrap_or(0)),
    }
}

/// What the Clean up Takes dialog shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanTakesView {
    /// Until the `takes_unused` asked for when the dialog opened answers.
    Reading,
    Listed(UnusedTakes),
    /// `takes_unused` failed with this error.
    Failed(String),
}

impl CleanTakesView {
    pub fn from_reply(reply: &Result<Value, String>) -> Self {
        match reply {
            Ok(reply) => Self::Listed(unused_takes(reply)),
            Err(error) => Self::Failed(error.clone()),
        }
    }
}

/// `bytes` in B, KB, MB or GB (powers of 1000), one decimal past B.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss, reason = "a size shown to one decimal")]
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// The error toast's title for a failed `req`: the action that failed.
pub fn failure_title(req: &Request) -> &'static str {
    match req {
        Request::Status => "Could not read the status",
        Request::List => "Could not list the Bank",
        Request::Load { .. } => "Could not load Preset",
        Request::Set { .. } => SET_FAILED,
        Request::Get { .. } => "Could not read the Parameters",
        Request::Save { .. } => "Could not save Preset",
        Request::Modules => "Could not list the Modules",
        Request::RackAdd { .. } => "Could not add the Module",
        Request::RackRemove { .. } => "Could not remove the Module",
        Request::RackMove { .. } => "Could not move the Module",
        Request::Tuner => "Could not read the Tuner",
        Request::Mute { .. } => "Could not switch Mute",
        Request::Bypass { .. } => "Could not switch the bypass",
        Request::IrList => "Could not list the Cabinet IRs",
        Request::RackIr { .. } => "Could not load the Cabinet IR",
        Request::Ports => "Could not list the ports",
        Request::SetPorts { .. } => "Could not connect the ports",
        Request::Settings => "Could not read the settings",
        Request::SetSettings { .. } => "Could not apply the settings",
        Request::ProjectNew { .. } => "Could not create the Project",
        Request::ProjectOpen { .. } => "Could not open the Project",
        Request::ProjectSave => "Could not save the Project",
        Request::ProjectClose => "Could not close the Project",
        Request::ProjectList => "Could not list the Projects",
        Request::ProjectUndo => "Could not undo",
        Request::ProjectRedo => "Could not redo",
        Request::Items => "Could not read the Items",
        Request::TrackAdd { .. } => "Could not add the Track",
        Request::TrackRemove { .. } => "Could not remove the Track",
        Request::TrackSet { .. } => "Could not change the Track",
        Request::TrackSelect { .. } => "Could not select the Track",
        Request::Transport { action, .. } => match action {
            TransportAction::Play => "Could not start playback",
            TransportAction::Stop => "Could not stop",
            TransportAction::Record => "Could not start recording",
            TransportAction::Locate => "Could not move the playhead",
        },
        Request::Loop { .. } => "Could not change the loop",
        Request::Tempo { .. } => "Could not set the tempo",
        Request::Click { .. } => "Could not switch the click",
        Request::Metronome { .. } => "Could not change the Metronome",
        Request::MetronomeTap => "Could not tap the tempo",
        Request::BusAdd { .. } => "Could not add the Bus",
        Request::BusRemove { .. } => "Could not remove the Bus",
        Request::BusSet { .. } => "Could not change the Bus",
        Request::SendSet { .. } => "Could not set the Send",
        Request::MasterSet { .. } => "Could not set the Master volume",
        Request::Render { .. } => "Could not start the Render",
        Request::RenderCancel => "Could not cancel the Render",
        Request::ClipReset { .. } => "Could not reset the clip",
        Request::ItemMove { .. } => "Could not move the Items",
        Request::ItemTrim { .. } => "Could not trim the Item",
        Request::ItemSplit { .. } => "Could not split the Items",
        Request::ItemDelete { .. } => "Could not delete the Items",
        Request::ItemTake { .. } => "Could not change the Take",
        Request::ItemDuplicate { .. } => "Could not duplicate the Items",
        Request::ItemCopy { .. } => "Could not copy the Items",
        Request::ItemPaste { .. } => "Could not paste the Items",
        Request::TakePeaks { .. } => "Could not read the waveform",
        Request::TakesUnused => "Could not list the unused Takes",
        Request::TakesClean { .. } => "Could not clean up the Takes",
    }
}

/// The error toast's title for a refused `set` or a typed value that is no number.
pub const SET_FAILED: &str = "Could not set the Parameter";

/// The address of a `set` that the Daemon refused or that never reached it.
pub fn failed_set(req: &Request, response: &Result<Response, ClientError>) -> Option<String> {
    match (req, response) {
        (_, Ok(Response::Ok(_))) => None,
        (Request::Set { param, .. }, _) => Some(param.clone()),
        _ => None,
    }
}
