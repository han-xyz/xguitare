use std::fmt;
use std::path::PathBuf;

use serde::de::{Deserializer, Error as _};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Map;

pub use serde_json::Value;

const BAD_BITS: &str = "bits must be 16, 24 or \"32f\"";
const FLOAT_BITS: &str = "32f";
const BAD_POSITION: &str = "position must be <bar>.<beat> or <seconds>s";
const OFF: &str = "off";
const BAD_LEVEL: &str = "level must be a number in dB or \"off\"";
const SECONDS_SUFFIX: char = 's';

/// The frames one min/max pair of a peak table covers.
pub const FRAMES_PER_PEAK: usize = 256;
/// The answer to `project_new` and `project_open` while a closed Project still gets its last Takes.
pub const SILENCE_DB: f64 = -120.0;
pub const STILL_CLOSING: &str = "a project is still closing; try again";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    List,
    Load {
        preset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    Set {
        param: String,
        value: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    Get {
        param: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    Save {
        /// Writes a new Preset file with this name instead of the active Preset's file.
        #[serde(rename = "as")]
        name: Option<String>,
    },
    Modules,
    RackAdd {
        r#type: String,
        after: Option<String>,
        at: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    RackRemove {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    RackMove {
        id: String,
        to: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    Tuner,
    Mute {
        on: bool,
    },
    Bypass {
        id: String,
        on: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    IrList,
    RackIr {
        ir: String,
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    Ports,
    SetPorts {
        input: String,
        output_left: String,
        output_right: String,
    },
    Settings,
    SetSettings {
        bank_kind: Option<BankKind>,
        ir_dir: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projects_dir: Option<PathBuf>,
    },
    ProjectNew {
        name: String,
    },
    ProjectOpen {
        name: String,
        /// Loads `project.autosave.toml` instead of `project.toml`.
        #[serde(default)]
        restore: bool,
    },
    ProjectSave,
    ProjectClose,
    ProjectList,
    ProjectUndo,
    ProjectRedo,
    Items,
    TrackAdd {
        name: Option<String>,
    },
    TrackRemove {
        track: TrackRef,
    },
    TrackSet {
        track: TrackRef,
        /// A capture port; `""` disconnects the Track's input.
        input: Option<String>,
        volume: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
        arm: Option<bool>,
        name: Option<String>,
        to_master: Option<bool>,
    },
    TrackSelect {
        track: TrackRef,
    },
    Transport {
        action: TransportAction,
        position: Option<Position>,
    },
    Loop {
        start: u64,
        end: u64,
        on: bool,
    },
    Tempo {
        bpm: Option<f64>,
        meter: Option<(u32, u32)>,
    },
    Click {
        on: Option<bool>,
        count_in: Option<u32>,
    },
    /// `volume` in dB, −40..=0.
    Metronome {
        on: Option<bool>,
        bpm: Option<f64>,
        meter: Option<(u32, u32)>,
        volume: Option<f64>,
    },
    MetronomeTap,
    BusAdd {
        name: Option<String>,
    },
    BusRemove {
        bus: BusRef,
    },
    BusSet {
        bus: BusRef,
        name: Option<String>,
        volume: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
    },
    SendSet {
        track: TrackRef,
        bus: BusRef,
        level: Option<SendLevel>,
        pre: Option<bool>,
    },
    MasterSet {
        volume: SendLevel,
    },
    /// Starts a Render of the open Project.
    Render {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<RenderRange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bits: Option<RenderBits>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    RenderCancel,
    /// Clears the clip flag of the Track, Bus or Master given, or of all when none is.
    ClipReset {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<BusRef>,
        #[serde(default, skip_serializing_if = "is_false")]
        master: bool,
    },
    /// Moves Items by `delta` frames, onto `track` for the first when given.
    ItemMove {
        ids: Vec<u64>,
        delta: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackRef>,
    },
    /// Moves an Item's start and end to these timeline frames.
    ItemTrim {
        id: u64,
        start: Option<u64>,
        end: Option<u64>,
    },
    /// Splits Items at the timeline frame `at`; without `ids`, those on the selected Track.
    ItemSplit {
        ids: Option<Vec<u64>>,
        at: u64,
    },
    ItemDelete {
        ids: Vec<u64>,
    },
    /// Makes the Take at index `take` the Item's active one.
    ItemTake {
        id: u64,
        take: usize,
    },
    /// Copies Items to right after the group they make, each on its own Track.
    ItemDuplicate {
        ids: Vec<u64>,
    },
    /// Puts Items in the Daemon's clipboard; not an edit.
    ItemCopy {
        ids: Vec<u64>,
    },
    /// Places the clipboard with its first Item on `track` and its start at the timeline frame `at`.
    ItemPaste {
        track: TrackRef,
        at: u64,
    },
    /// Pairs of the loaded Take `file`'s peak table from pair `first`, `count` of them.
    TakePeaks {
        file: String,
        first: Option<usize>,
        count: Option<usize>,
    },
    /// The open Project's Take files that nothing uses.
    TakesUnused,
    /// Deletes these unused Take files, `takes/<name>.wav` each, after saving the Project.
    TakesClean {
        files: Vec<String>,
    },
}

/// A Track or Bus by its zero-based index (a JSON number) or its name (a JSON string).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IndexOrName {
    Index(usize),
    Name(String),
}

/// A Track, as [`IndexOrName`].
pub type TrackRef = IndexOrName;

/// A Bus, as [`IndexOrName`].
pub type BusRef = IndexOrName;

/// A Send level on the wire: a number in dB or the string `"off"`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LevelOnWire", into = "LevelOnWire")]
pub enum SendLevel {
    Db(f32),
    Off,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum LevelOnWire {
    Db(f32),
    Word(String),
}

impl TryFrom<LevelOnWire> for SendLevel {
    type Error = &'static str;

    fn try_from(level: LevelOnWire) -> Result<Self, Self::Error> {
        match level {
            LevelOnWire::Db(db) => Ok(SendLevel::Db(db)),
            LevelOnWire::Word(word) if word == OFF => Ok(SendLevel::Off),
            LevelOnWire::Word(_) => Err(BAD_LEVEL),
        }
    }
}

impl From<SendLevel> for LevelOnWire {
    fn from(level: SendLevel) -> Self {
        match level {
            SendLevel::Db(db) => LevelOnWire::Db(db),
            SendLevel::Off => LevelOnWire::Word(OFF.to_string()),
        }
    }
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_false(on: &bool) -> bool {
    !on
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportAction {
    Play,
    Stop,
    Record,
    Locate,
}

/// Where `locate` moves the playhead: a frame (a JSON number), or text `<bar>.<beat>`, `<bar>` or `<seconds>s`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Position {
    Frames(u64),
    Text(String),
}

/// One Control API reply; on the wire `{"ok":true, ...payload fields}` or `{"ok":false,"error":"..."}`.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Ok(Value),
    Err { error: String },
}

impl Request {
    #[allow(
        clippy::missing_panics_doc,
        reason = "the expect is an invariant: a Request always serialises"
    )]
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("a Request always serialises to JSON")
    }
}

impl Response {
    pub fn err(error: impl Into<String>) -> Response {
        Response::Err {
            error: error.into(),
        }
    }

    /// # Errors
    ///
    /// The JSON error when `line` is not a Response.
    pub fn from_line(line: &str) -> Result<Response, serde_json::Error> {
        serde_json::from_str(line)
    }
}

/// Which starter Bank the first run seeds into a missing `bank_dir`; without a `bank_dir` in the config file, `[banks]` in `state.toml` can name each kind's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BankKind {
    #[default]
    Guitar,
    Bass,
}

impl fmt::Display for BankKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BankKind::Guitar => "guitar",
            BankKind::Bass => "bass",
        })
    }
}

/// What a Render plays: the Project from frame 0 to the end of its last Item plus the tail, or the loop region exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderRange {
    Project,
    Loop,
}

/// The sample format of the WAV a Render writes; on the wire `16`, `24` or `"32f"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BitsOnWire", into = "BitsOnWire")]
pub enum RenderBits {
    /// 16-bit integer with TPDF dither.
    Pcm16,
    Pcm24,
    Float32,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum BitsOnWire {
    Depth(u32),
    Word(String),
}

impl TryFrom<BitsOnWire> for RenderBits {
    type Error = &'static str;

    fn try_from(bits: BitsOnWire) -> Result<Self, Self::Error> {
        match bits {
            BitsOnWire::Depth(16) => Ok(RenderBits::Pcm16),
            BitsOnWire::Depth(24) => Ok(RenderBits::Pcm24),
            BitsOnWire::Word(word) if word == FLOAT_BITS => Ok(RenderBits::Float32),
            BitsOnWire::Depth(_) | BitsOnWire::Word(_) => Err(BAD_BITS),
        }
    }
}

impl From<RenderBits> for BitsOnWire {
    fn from(bits: RenderBits) -> Self {
        match bits {
            RenderBits::Pcm16 => BitsOnWire::Depth(16),
            RenderBits::Pcm24 => BitsOnWire::Depth(24),
            RenderBits::Float32 => BitsOnWire::Word(FLOAT_BITS.to_string()),
        }
    }
}

/// Start and end in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopRegion {
    pub start: u64,
    pub end: u64,
    pub on: bool,
}

/// `name` with each control character and each Unicode bidi override or isolate escaped as `escape_debug` writes it, so a file name shown to people reads as the name it is.
pub fn shown_file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if is_hidden_char(c) {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// A control character, or a bidi override or isolate (U+202A–U+202E, U+2066–U+2069), which can make shown text read as other text.
pub(crate) fn is_hidden_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// The frame of `<bar>.<beat>` or `<bar>` (both from 1, the beat within `beats_per_bar`) or `<seconds>s`, at `sample_rate` and `tempo_bpm`, rounded; not bounded.
///
/// # Errors
///
/// `position must be <bar>.<beat> or <seconds>s` for any other text, a negative time, or a bar or beat of 0 or a beat past the bar.
pub fn position_frame(
    text: &str,
    sample_rate: u32,
    tempo_bpm: f64,
    beats_per_bar: u32,
) -> Result<f64, String> {
    if let Some(seconds) = text.strip_suffix(SECONDS_SUFFIX) {
        let seconds: f64 = seconds.parse().map_err(|_| BAD_POSITION)?;
        if !seconds.is_finite() || seconds.is_sign_negative() {
            return Err(BAD_POSITION.to_string());
        }
        return Ok((seconds * f64::from(sample_rate)).round());
    }
    let (bar, beat) = text.split_once('.').unwrap_or((text, "1"));
    let (Ok(bar), Ok(beat)) = (bar.parse::<u64>(), beat.parse::<u64>()) else {
        return Err(BAD_POSITION.to_string());
    };
    let beats_per_bar = u64::from(beats_per_bar);
    if bar == 0 || beat == 0 || beat > beats_per_bar {
        return Err(BAD_POSITION.to_string());
    }
    let beat_index = (bar - 1) as f64 * beats_per_bar as f64 + (beat - 1) as f64;
    Ok((beat_index * (f64::from(sample_rate) * 60.0 / tempo_bpm)).round())
}

impl Serialize for Response {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self {
            Response::Ok(payload) => {
                map.serialize_entry("ok", &true)?;
                for (key, value) in payload.as_object().into_iter().flatten() {
                    map.serialize_entry(key, value)?;
                }
            }
            Response::Err { error } => {
                map.serialize_entry("ok", &false)?;
                map.serialize_entry("error", error)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Response {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Response, D::Error> {
        let mut fields = Map::deserialize(deserializer)?;
        match fields.remove("ok") {
            Some(Value::Bool(true)) => Ok(Response::Ok(Value::Object(fields))),
            Some(Value::Bool(false)) => match fields.remove("error") {
                Some(Value::String(error)) => Ok(Response::err(error)),
                _ => Err(D::Error::custom(
                    "error response without an \"error\" string",
                )),
            },
            _ => Err(D::Error::custom("response without a boolean \"ok\"")),
        }
    }
}
