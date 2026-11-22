use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::RenderRange;

const NOT_AN_OBJECT: &str = "a reply payload is not a JSON object";

/// The `get` reply: one entry per Parameter asked for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GetReply {
    pub params: Vec<ParamStatus>,
}

/// A Parameter as `get` lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParamStatus {
    pub module: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub param: String,
    pub value: f64,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub integer: bool,
    pub labels: Vec<String>,
    pub bypass: bool,
    /// `None` on a Module that is not a cabinet (no `ir` key); `Some(None)` on a cabinet without a Cabinet IR (`"ir": null`).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub ir: Option<Option<String>>,
}

/// The `items` reply: every Track of the open Project with its Items.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemsReply {
    pub sample_rate: u32,
    pub tracks: Vec<ItemsTrack>,
}

/// A Track as `items` lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemsTrack {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub items: Vec<ItemStatus>,
}

/// An Item as `items` lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemStatus {
    pub id: u64,
    pub position: u64,
    #[serde(default)]
    pub bars_beats: String,
    #[serde(default)]
    pub offset: u64,
    pub length: u64,
    pub take: usize,
    #[serde(default)]
    pub takes: Vec<String>,
    /// The active Take's frames; `null` without one.
    #[serde(default)]
    pub take_frames: Option<u64>,
}

/// The `status` reply. The Project keys are `null`, and `tracks` and `buses` empty, while no Project is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusReply {
    pub preset: Option<String>,
    pub program: Option<usize>,
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub prepared_frames: usize,
    pub revision: u64,
    pub muted: bool,
    pub modified: bool,
    /// `null` when the peak is not a finite number.
    pub input_peak_db: Option<f64>,
    pub output_peak_db: Option<f64>,
    pub xruns: u64,
    pub callback_max_us: u64,
    pub callback_last_us: u64,
    pub connections: Vec<PortConnection>,
    pub metronome: MetronomeStatus,
    /// `Some(1)` when the key is missing: a Daemon from before versioning speaks version 1. `None` when it holds anything but a non-negative whole number; the version must never make a `status` unreadable.
    #[serde(deserialize_with = "version_or_unknown")]
    pub protocol_version: Option<u64>,
    pub disk_overflows: u64,
    pub count_in_remaining: u32,
    pub loaded_audio_bytes: usize,
    pub project_warning: Option<String>,
    pub render: Option<RenderStatus>,
    pub project: Option<String>,
    /// `stopped`, `playing` or `recording`.
    pub transport: Option<String>,
    pub position: Option<PositionStatus>,
    pub tempo: Option<TempoStatus>,
    #[serde(rename = "loop")]
    pub loop_region: Option<LoopStatus>,
    pub click: Option<ClickStatus>,
    pub selected: Option<usize>,
    pub undo: Option<String>,
    pub redo: Option<String>,
    pub clipboard: usize,
    pub tracks: Vec<TrackStatus>,
    pub buses: Vec<BusStatus>,
    pub master: Option<MasterStatus>,
}

impl Default for StatusReply {
    fn default() -> Self {
        StatusReply {
            preset: None,
            program: None,
            sample_rate: 0,
            buffer_size: 0,
            prepared_frames: 0,
            revision: 0,
            muted: false,
            modified: false,
            input_peak_db: None,
            output_peak_db: None,
            xruns: 0,
            callback_max_us: 0,
            callback_last_us: 0,
            connections: Vec::new(),
            metronome: MetronomeStatus::default(),
            protocol_version: Some(1),
            disk_overflows: 0,
            count_in_remaining: 0,
            loaded_audio_bytes: 0,
            project_warning: None,
            render: None,
            project: None,
            transport: None,
            position: None,
            tempo: None,
            loop_region: None,
            click: None,
            selected: None,
            undo: None,
            redo: None,
            clipboard: 0,
            tracks: Vec::new(),
            buses: Vec::new(),
            master: None,
        }
    }
}

/// The Metronome as `status` shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetronomeStatus {
    pub on: bool,
    pub bpm: f64,
    pub meter: (u32, u32),
    pub volume_db: f64,
    pub beats_played: u64,
    pub beat: u32,
}

/// A port connection the Daemon made at start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PortConnection {
    pub from: String,
    pub to: String,
}

/// The running Render, or the last one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderStatus {
    pub name: String,
    pub range: RenderRange,
    pub progress: f32,
    /// `running`, `done`, `cancelled` or `failed`.
    pub state: String,
    pub file: String,
    /// Why the Render failed; no key otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The playhead of the open Project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PositionStatus {
    pub frames: u64,
    pub bars_beats: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TempoStatus {
    pub bpm: f64,
    pub meter: (u32, u32),
}

/// The loop region of the open Project, start and end in frames.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopStatus {
    pub start: u64,
    pub end: u64,
    pub on: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClickStatus {
    pub on: bool,
    pub count_in: u32,
}

/// A Track as `status` lists it; `volume_db` and each peak are `null` when off or not a finite number.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "mute, solo, armed, to_master and clip are independent Track switches"
)]
pub struct TrackStatus {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub volume_db: Option<f32>,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub armed: bool,
    #[serde(default)]
    pub input_peak_db: Option<f64>,
    #[serde(default)]
    pub to_master: bool,
    #[serde(default)]
    pub sends: Vec<SendStatus>,
    /// Left and right.
    #[serde(default)]
    pub peak_db: [Option<f64>; 2],
    #[serde(default)]
    pub clip: bool,
}

/// A Send that is not off, with its Bus's index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SendStatus {
    pub bus: usize,
    #[serde(default)]
    pub level_db: f32,
    #[serde(default)]
    pub pre: bool,
}

/// A Bus as `status` lists it; `volume_db` and each peak are `null` when off or not a finite number.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BusStatus {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub volume_db: Option<f32>,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub peak_db: [Option<f64>; 2],
    #[serde(default)]
    pub clip: bool,
}

/// Master as `status` shows it; `volume_db` and each peak are `null` when off or not a finite number.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MasterStatus {
    pub volume_db: Option<f32>,
    pub peak_db: [Option<f64>; 2],
    pub clip: bool,
}

/// A typed reply as the payload of `Response::Ok`.
///
/// # Panics
///
/// When `reply` does not serialise to JSON; the reply structs of this crate always do.
pub fn to_payload<T: Serialize>(reply: &T) -> Value {
    serde_json::to_value(reply).expect("a reply struct serialises to JSON")
}

/// The payload of `Response::Ok` read as a typed reply; a missing key reads as its default, except an identity or placement key (`id`, an Item's `position`, `length` and `take`, a Send's `bus`), and an unknown key is ignored.
///
/// # Errors
///
/// The `serde_json` error when `payload` is not a JSON object, a key holds a value of the wrong type, or an identity or placement key is missing.
pub fn from_payload<T: DeserializeOwned>(payload: &Value) -> Result<T, serde_json::Error> {
    if !payload.is_object() {
        return Err(serde::de::Error::custom(NOT_AN_OBJECT));
    }
    T::deserialize(payload)
}

fn version_or_unknown<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    Value::deserialize(deserializer).map(|version| version.as_u64())
}

/// Reads a key that is there, also as `null`, as `Some`; a missing key stays `None` through `#[serde(default)]`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
