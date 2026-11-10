use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;
use xguitare_client::ClientError;
use xguitare_core::BankKind;
use xguitare_core::control::{Request, Response, SILENCE_DB};

pub const SPAWN_AFTER_FAILURES: u32 = 3;
pub const SPAWN_BACKOFF: Duration = Duration::from_secs(5);
pub const METER_FLOOR_DB: f64 = -60.0;
pub const SET_INTERVAL: Duration = Duration::from_millis(30);
pub const FEEDBACK_FOR: Duration = Duration::from_secs(3);
pub const INVALID_PRESET: &str = "invalid preset";
pub const KNOB_DRAG_PX: f32 = 220.0;
pub const KNOB_FINE_DRAG_PX: f32 = 2200.0;
pub const KNOB_ARC_START_DEG: f32 = 135.0;
pub const KNOB_ARC_SWEEP_DEG: f32 = 270.0;
pub const KNOB_ARC_POINTS: usize = 49;
pub const LADDER_SEGMENTS: usize = 24;
pub const LADDER_YELLOW_DB: f64 = -12.0;
pub const LADDER_RED_DB: f64 = -3.0;
pub const EAR_WIDTH: f32 = 30.0;
pub const EAR_HOLE_RADIUS: f32 = 4.0;
pub const EAR_HOLE_INSET: f32 = 22.0;
pub const NEUTRAL_ACCENT: (u8, u8, u8) = (120, 120, 120);
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
    Disconnected { since: Instant },
    Starting { since: Instant },
}

/// What the client thread last saw; the payloads are the Control API's `status`, `list`, `get`, `tuner`, `ports` and `settings` replies as sent, `module_types` the `modules` reply of the current connection, `irs` the last `ir_list` reply's names.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub link: Link,
    pub status: Option<Value>,
    pub bank: Option<Value>,
    pub params: Option<Value>,
    pub tuner: Option<Value>,
    pub ports: Option<Value>,
    pub settings: Option<Value>,
    pub module_types: Vec<ModuleType>,
    pub irs: Vec<String>,
    pub last_message: Option<(String, Instant)>,
    pub failed_set: Option<(String, Instant)>,
    pub refused_remove: Option<(String, Instant)>,
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
    RackIr {
        id: String,
        ir: String,
    },
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
    },
    Quit,
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

/// The Cabinet IR name of Module `id` in a `get` payload, read from its first entry.
pub fn cabinet_ir(get_payload: &Value, id: &str) -> Option<String> {
    get_payload["params"]
        .as_array()?
        .iter()
        .find(|entry| entry["module"].as_str() == Some(id))?["ir"]
        .as_str()
        .map(str::to_string)
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

/// Offsets from the knob's centre in egui's y-down space: the pointer tip at `radius`, and the arc from the sweep's start to the value.
#[derive(Debug, Clone, PartialEq)]
pub struct KnobPoints {
    pub pointer: (f32, f32),
    pub arc: Vec<(f32, f32)>,
}

pub fn knob_points(fraction: f32, radius: f32) -> KnobPoints {
    let value_deg = knob_angle_deg(fraction);
    let last = (KNOB_ARC_POINTS - 1) as f32;
    let arc = (0..KNOB_ARC_POINTS)
        .map(|step| {
            let deg = KNOB_ARC_START_DEG + (value_deg - KNOB_ARC_START_DEG) * step as f32 / last;
            point_at_deg(radius, deg)
        })
        .collect();
    KnobPoints {
        pointer: point_at_deg(radius, value_deg),
        arc,
    }
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

/// How far to scroll the Rack while a unit is dragged by its ears with the pointer at `pointer_y` in a scroll area from `top` to `bottom`: negative up, positive down, growing to [`AUTOSCROLL_MAX_PX`] as the pointer reaches an edge within [`AUTOSCROLL_EDGE_PX`]; 0 elsewhere.
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

/// The Rack position a unit dragged from `from` lands on: the unit under `pointer_y`, given each unit's top edge in Rack order; above the first unit is the front, below the last the end. A release outside the window (`None`) or an empty Rack leaves it at `from`.
pub fn drop_index(from: usize, pointer_y: Option<f32>, unit_tops: &[f32]) -> usize {
    let Some(pointer_y) = pointer_y.filter(|_| !unit_tops.is_empty()) else {
        return from;
    };
    unit_tops
        .iter()
        .filter(|&&top| top <= pointer_y)
        .count()
        .saturating_sub(1)
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

/// Settles every id in `removing` against the first `get` payload after its Remove: the open state of an id the payload no longer has is dropped, a Module still there means the Remove was refused and keeps its unit open. Another Preset lacking an id that was never removed keeps it.
pub fn forget_removed_units(
    open: &mut BTreeSet<String>,
    removing: &mut BTreeSet<String>,
    get_payload: &Value,
) {
    let present: BTreeSet<String> = param_rows(get_payload)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
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

pub type Rgb = (u8, u8, u8);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub ground: Rgb,
    pub sidebar: Rgb,
    pub unit: Rgb,
    pub unit_open: Rgb,
    pub meter_bridge: Rgb,
    pub rail: Rgb,
    pub edge_light: Rgb,
    pub edge_dark: Rgb,
    pub button_border: Rgb,
    pub outline: Rgb,
    pub row_active: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub knob_body: Rgb,
    pub knob_rim: Rgb,
    pub ear_edge: Rgb,
    pub ear_centre: Rgb,
    pub ear_hole: Rgb,
    pub led_off: Rgb,
    pub green: Rgb,
    pub yellow: Rgb,
    pub red: Rgb,
}

pub fn palette() -> Palette {
    Palette {
        ground: (0x1a, 0x1d, 0x20),
        sidebar: (0x22, 0x26, 0x2a),
        unit: (0x26, 0x2a, 0x2e),
        unit_open: (0x30, 0x35, 0x3a),
        meter_bridge: (0x2a, 0x2e, 0x33),
        rail: (0x0f, 0x11, 0x13),
        edge_light: (0x3d, 0x44, 0x4b),
        edge_dark: (0x11, 0x13, 0x16),
        button_border: (0x4a, 0x52, 0x5a),
        outline: (0x34, 0x3a, 0x40),
        row_active: (0x2d, 0x32, 0x37),
        text: (0xe4, 0xe7, 0xea),
        muted: (0xa9, 0xb0, 0xb7),
        knob_body: (0x15, 0x17, 0x1a),
        knob_rim: (0x5a, 0x63, 0x6c),
        ear_edge: (0x8c, 0x93, 0x9a),
        ear_centre: (0xc2, 0xc8, 0xcd),
        ear_hole: (0x0b, 0x0c, 0x0d),
        led_off: (0x33, 0x38, 0x3d),
        green: (0x46, 0xb4, 0x50),
        yellow: (0xf0, 0xb4, 0x32),
        red: (0xe5, 0x48, 0x3f),
    }
}

pub fn accent_for(type_name: &str) -> (u8, u8, u8) {
    match type_name {
        "gain" => (170, 175, 185),
        "gate" => (100, 120, 175),
        "drive" => (235, 130, 40),
        "fuzz" => (210, 50, 45),
        "preamp" => (240, 180, 50),
        "tonestack" => (215, 195, 150),
        "compressor" => (40, 170, 160),
        "cabinet" => (140, 95, 60),
        "delay" => (60, 120, 220),
        "reverb" => (150, 90, 210),
        "chorus" => (70, 180, 80),
        "flanger" => (50, 200, 220),
        _ => NEUTRAL_ACCENT,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Up,
    Down,
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
        KeyEvent::Digit(..) => None,
    }
}

pub fn spawn_policy(failures: u32, last_spawn: Option<Instant>, now: Instant) -> bool {
    failures >= SPAWN_AFTER_FAILURES
        && last_spawn.is_none_or(|spawned| now.duration_since(spawned) >= SPAWN_BACKOFF)
}

/// Only a missing socket counts: a Daemon that does not answer may still be alive, and an insecure socket directory fails a spawned Daemon too.
pub fn counts_toward_spawn(err: &ClientError) -> bool {
    matches!(err, ClientError::NotRunning(_))
}

pub fn requests_until_quit(queued: Vec<Command>) -> (Vec<Request>, bool) {
    let mut requests = Vec::new();
    for command in queued {
        let req = match command {
            Command::Load(preset) => Request::Load { preset },
            Command::Set { param, value } => Request::Set { param, value },
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
            },
            Command::RackRemove { id } => Request::RackRemove { id },
            Command::RackMove { id, to } => Request::RackMove { id, to },
            Command::Mute(on) => Request::Mute { on },
            Command::RackIr { id, ir } => Request::RackIr { ir, id: Some(id) },
            Command::SetPorts {
                input,
                output_left,
                output_right,
            } => Request::SetPorts {
                input,
                output_left,
                output_right,
            },
            Command::SetSettings { bank_kind, ir_dir } => {
                Request::SetSettings { bank_kind, ir_dir }
            }
            Command::RefreshIrs | Command::RefreshPorts => continue,
            Command::Quit => return (requests, true),
        };
        requests.push(req);
    }
    (requests, false)
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

/// Opens or closes the Tuner view: (open after, command to send, whether the Mute is ours after). Opening mutes when `mute_default`; closing unmutes only a Mute this view turned on.
pub fn tuner_toggle(
    open_now: bool,
    mute_default: bool,
    muted_by_us: bool,
) -> (bool, Option<Command>, bool) {
    match (open_now, mute_default, muted_by_us) {
        (false, true, _) => (true, Some(Command::Mute(true)), true),
        (false, false, _) => (true, None, false),
        (true, _, true) => (false, Some(Command::Mute(false)), false),
        (true, _, false) => (false, None, false),
    }
}

/// `held_db` is `None` while no status arrives, so the text shows no value.
pub fn peak_text(held_db: Option<f64>, clipped: bool) -> String {
    let Some(db) = held_db else {
        return "peak —".to_string();
    };
    let text = if clipped {
        format!("peak {db:+.1} dB, clipped")
    } else {
        format!("peak {db:+.1} dB")
    };
    text.replace('-', "\u{2212}")
}

pub fn refresh_decision(last_seen: Option<u64>, revision: u64) -> bool {
    last_seen != Some(revision)
}

/// A peak the payload lacks or carries as `null` (a non-finite value serialises as `null`) reads as −120 dB.
pub fn peak_db(status: &Value, key: &str) -> f64 {
    status[key].as_f64().unwrap_or(SILENCE_DB)
}

/// How long one buffer of `buffer` frames lasts at `rate` Hz.
pub fn latency_ms(buffer: u64, rate: u64) -> f64 {
    buffer as f64 * 1000.0 / rate as f64
}

/// The sidebar's label and value rows for a `status` payload; only the Daemon row without one.
pub fn engine_readout(status: Option<&Value>) -> Vec<(String, String)> {
    let Some(status) = status else {
        return vec![("Daemon".to_string(), "disconnected".to_string())];
    };
    let rate = status["sample_rate"].as_u64().unwrap_or_default();
    let buffer = status["buffer_size"].as_u64().unwrap_or_default();
    [
        ("Rate", format!("{} kHz", rate as f64 / 1000.0)),
        ("Buffer", format!("{buffer} frames")),
        ("Latency", format!("{:.1} ms", latency_ms(buffer, rate))),
        ("Xruns", status["xruns"].to_string()),
        ("Daemon", "connected".to_string()),
    ]
    .into_iter()
    .map(|(label, value)| (label.to_string(), value))
    .collect()
}

pub fn link_text(link: &Link, now: Instant) -> String {
    match link {
        Link::Connected => "connected".to_string(),
        Link::Disconnected { since } => format!("disconnected {}s", seconds_since(*since, now)),
        Link::Starting { since } => format!("starting daemon {}s", seconds_since(*since, now)),
    }
}

fn seconds_since(since: Instant, now: Instant) -> u64 {
    now.saturating_duration_since(since).as_secs()
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
}

/// Groups the `get` payload's `params` per Module, keeping Rack order.
pub fn param_rows(get_payload: &Value) -> Vec<(String, Vec<ParamRow>)> {
    let mut modules: Vec<(String, Vec<ParamRow>)> = Vec::new();
    for param in get_payload["params"].as_array().into_iter().flatten() {
        let module = param["module"].as_str().unwrap_or_default().to_string();
        let name = param["param"].as_str().unwrap_or_default().to_string();
        let row = ParamRow {
            address: format!("{module}.{name}"),
            module: module.clone(),
            module_type: param["type"].as_str().unwrap_or_default().to_string(),
            param: name,
            value: param["value"].as_f64().unwrap_or_default(),
            min: param["min"].as_f64().unwrap_or_default(),
            max: param["max"].as_f64().unwrap_or_default(),
            default: param["default"].as_f64().unwrap_or_default(),
            unit: param["unit"].as_str().unwrap_or_default().to_string(),
            integer: param["integer"].as_bool().unwrap_or_default(),
        };
        match modules.last_mut() {
            Some((id, rows)) if *id == module => rows.push(row),
            _ => modules.push((module, vec![row])),
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

/// Rows of small knobs a closed unit of `knob_count` Parameters needs in `width`: as many per row as fit, at least one per row and one row.
pub fn closed_unit_rows(knob_count: usize, width: f32) -> usize {
    let per_row = ((width / CLOSED_KNOB_WIDTH).floor() as usize).max(1);
    knob_count.div_ceil(per_row).max(1)
}

const KILO: f64 = 1000.0;
const RATIO_PARAM: &str = "ratio";

/// A Parameter name in sentence case: `low_cut` reads `Low cut`.
pub fn param_label(param: &str) -> String {
    let words = param.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A unit's name: the Module id in sentence case, its type after it when the two differ.
pub fn module_title(module: &str, type_name: &str) -> String {
    let title = param_label(module);
    if type_name.is_empty() || type_name == module {
        return title;
    }
    format!("{title} ({type_name})")
}

/// The value as a knob shows it: one decimal at most, none on a whole value, a Unicode minus, `kHz` from 1000 Hz, a ratio as `6 : 1`.
pub fn format_value(spec: &ParamRow, value: f64) -> String {
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

/// The value typed into a value field, or the footer message for text that is no number.
pub fn typed_value(text: &str) -> Result<f64, String> {
    parse_typed_value(text).ok_or_else(|| format!("error: '{}' is not a number", text.trim()))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Preset,
    Bank,
    Rack,
    Tuner,
    Audio,
    Settings,
    Engine,
}

/// The left panel's sections, top to bottom.
pub const SECTIONS: [Section; 7] = [
    Section::Preset,
    Section::Bank,
    Section::Rack,
    Section::Tuner,
    Section::Audio,
    Section::Settings,
    Section::Engine,
];

impl Section {
    pub fn title(self, bank_kind: BankKind) -> &'static str {
        match (self, bank_kind) {
            (Section::Preset, _) => "Preset",
            (Section::Bank, BankKind::Guitar) => "Guitar Bank",
            (Section::Bank, BankKind::Bass) => "Bass Bank",
            (Section::Rack, _) => "Rack",
            (Section::Tuner, _) => "Tuner and Mute",
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
}

/// The Settings section's values from a `settings` payload; `None` when it lacks them.
pub fn settings_draft(settings_payload: &Value) -> Option<SettingsDraft> {
    Some(SettingsDraft {
        bank_kind: serde_json::from_value(settings_payload["bank_kind"].clone()).ok()?,
        ir_dir: settings_payload["ir_dir"].as_str()?.to_string(),
    })
}

/// A `set_settings` with only the keys `draft` changes from `current`; `None` when it changes none.
pub fn set_settings_command(current: &SettingsDraft, draft: &SettingsDraft) -> Option<Command> {
    let bank_kind = (draft.bank_kind != current.bank_kind).then_some(draft.bank_kind);
    let ir_dir = draft.ir_dir.trim();
    let ir_dir = (ir_dir != current.ir_dir).then(|| PathBuf::from(ir_dir));
    if bank_kind.is_none() && ir_dir.is_none() {
        return None;
    }
    Some(Command::SetSettings { bank_kind, ir_dir })
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

#[derive(Debug, Default)]
pub struct Feedback {
    message: Option<(String, Instant)>,
}

impl Feedback {
    pub fn show(&mut self, text: String, at: Instant) {
        self.message = Some((text, at));
    }

    pub fn text(&self, now: Instant) -> Option<&str> {
        self.message
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) < FEEDBACK_FOR)
            .map(|(text, _)| text.as_str())
    }
}

/// The id of a `rack remove` that the Daemon refused or that never reached it.
pub fn refused_remove(req: &Request, response: &Result<Response, ClientError>) -> Option<String> {
    match (req, response) {
        (_, Ok(Response::Ok(_))) => None,
        (Request::RackRemove { id }, _) => Some(id.clone()),
        _ => None,
    }
}

/// The status bar message for a command's response: a saved path or an error; other successes are silent.
pub fn response_text(req: &Request, response: Result<Response, ClientError>) -> Option<String> {
    match (req, response) {
        (Request::Save { .. }, Ok(Response::Ok(payload))) => Some(format!(
            "saved {}",
            payload["path"].as_str().unwrap_or_default()
        )),
        (_, Ok(Response::Ok(_))) => None,
        (_, Ok(Response::Err { error })) => Some(format!("error: {error}")),
        (_, Err(err)) => Some(format!("error: {err}")),
    }
}

/// The address of a `set` that the Daemon refused or that never reached it.
pub fn failed_set(req: &Request, response: &Result<Response, ClientError>) -> Option<String> {
    match (req, response) {
        (_, Ok(Response::Ok(_))) => None,
        (Request::Set { param, .. }, _) => Some(param.clone()),
        _ => None,
    }
}
