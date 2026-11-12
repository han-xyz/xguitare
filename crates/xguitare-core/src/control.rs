use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::{Deserializer, Error as _};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};
use xguitare_dsp::{Channels, ParamSpec, Rack, Unit};

pub use serde_json::Value;

use crate::build::{ENGINE_BUSY, build_rack, swap_rack};
use crate::engine::{
    Command, EngineQueues, EngineShared, FadeLengths, MeterPoint, MixMeters, SWITCHER_MAX_FRAMES,
    take_peak,
};
use crate::firstrun::{BASS_IR, DEFAULT_IR, ensure_bank};
use crate::item_control::{
    item_copy, item_delete, item_duplicate, item_move, item_paste, item_split, item_take, item_trim,
};
use crate::metronome::Metronome;
use crate::metronome_control::{metronome, metronome_status, metronome_tap, practice_settings};
use crate::mixer_control::{
    BusChange, bus_add, bus_remove, bus_set, clip_reset, master_set, send_set,
};
use crate::ports::{AudioPorts, AvailablePort, connect_plan};
use crate::preset::{CABINET_TYPE, ir_path, is_relative_name, shortest_f64};
use crate::project::{LoopRegion, MAX_BUSES, MAX_TRACKS, ProjectError};
use crate::project_control::{
    ProjectRack, Projects, TrackChange, bypass_command, change_rack_module, click,
    create_projects_dir, items, param_command, project_close, project_list, project_new,
    project_open, project_redo, project_save, project_undo, rack_preset, rebuild_arrangement,
    rebuild_arrangement_slots, set_loop, set_rack_modules, status_fields, target_rack, tempo,
    track_add, track_remove, track_select, track_set, transport,
};
use crate::render::{RenderBits, RenderRange};
use crate::render_control::{render, render_cancel};
use crate::state::{SavedState, read_state_bytes, restore_state, write_state};
use crate::tuner::Tuner;
use crate::{
    Bank, BankEntry, BankKind, BuildWarning, Config, ModuleRegistry, ParamAddress, Ports, Preset,
    PresetModule, list_irs,
};
use crate::{ResolveError, resolve};

const NO_PRESET: &str = "no preset loaded";
const ACTIVE_REMOVED: &str = "active preset file was removed; load a preset first";
const NANOS_PER_MICRO: u64 = 1_000;
pub const SILENCE_DB: f64 = -120.0;
const CABINET_IRS: [&str; 2] = [DEFAULT_IR, BASS_IR];
const CABINET_NEEDS_IR: &str = "cabinet needs an ir; add one in the preset file";
const AFTER_OR_AT: &str = "give either after or at";
const NO_CABINET: &str = "no cabinet module";
const EMPTY_PRESET_NAME: &str = "preset name must not be empty";
const SAVE_AS_TRIES: u64 = 8;
const NO_AUDIO_PORTS: &str = "audio ports are not available";
const BANK_DIR_IN_CONFIG: &str = "bank_dir is set in config.toml; edit it there";
const PROJECTS_DIR_NOT_ABSOLUTE: &str = "projects_dir must be an absolute path";
const PRESET_INTO_TRACK: &str = "a Preset loads only into a Track";
const OFF: &str = "off";
const BAD_LEVEL: &str = "level must be a number in dB or \"off\"";
/// Linux `EPERM`, which `hard_link` returns on a filesystem without hard links; `EACCES` shares its `ErrorKind`.
const EPERM: i32 = 1;

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

pub struct EngineState {
    pub bank: Bank,
    pub active: Option<(PathBuf, Preset)>,
    pub shared: Arc<EngineShared>,
    pub queues: EngineQueues,
    pub tuner: Tuner,
    pub config: Config,
    pub connections: Vec<(String, String)>,
    pub sample_rate: f32,
    pub max_frames: usize,
    /// Incremented on every change to the live Rack or the Bank; starts at 1.
    pub revision: u64,
    /// Whether the live Rack has changed since the active Preset was loaded, reloaded or saved.
    pub modified: bool,
    pub peaks: PeakMeters,
    /// The Preset `save` last wrote to each file, until the Bank watcher sees that file change or a `load`.
    pub saved: HashMap<PathBuf, Preset>,
    pub projects: Projects,
    pub metronome: Metronome,
}

/// One Control API connection, from `open_connection` to `close_connection`; `status` reports and clears only its own peaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionId(u64);

const IN_PROCESS: ConnectionId = ConnectionId(0);

/// For each open connection, and for `handle`'s in-process caller, the largest input and output peaks since its previous `status`.
#[derive(Debug)]
pub struct PeakMeters {
    opened: u64,
    meters: HashMap<ConnectionId, Peaks>,
}

impl Default for PeakMeters {
    fn default() -> Self {
        PeakMeters {
            opened: 0,
            meters: HashMap::from([(IN_PROCESS, Peaks::default())]),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Peaks {
    input: f32,
    output: f32,
    tracks: [f32; MAX_TRACKS],
    mix: MixPeaks,
}

/// Post-fader left and right peaks per Track, Bus and Master.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MixPeaks {
    pub tracks: [[f32; 2]; MAX_TRACKS],
    pub buses: [[f32; 2]; MAX_BUSES],
    pub master: [f32; 2],
}

impl MixPeaks {
    fn take(meters: &MixMeters) -> MixPeaks {
        MixPeaks {
            tracks: std::array::from_fn(|idx| meters.take_peak(MeterPoint::Track(idx))),
            buses: std::array::from_fn(|idx| meters.take_peak(MeterPoint::Bus(idx))),
            master: meters.take_peak(MeterPoint::Master),
        }
    }

    fn raise(&mut self, other: &MixPeaks) {
        let pairs = self.tracks.iter_mut().zip(&other.tracks);
        let pairs = pairs.chain(self.buses.iter_mut().zip(&other.buses));
        for (mine, theirs) in pairs.chain([(&mut self.master, &other.master)]) {
            for (peak, other) in mine.iter_mut().zip(theirs) {
                *peak = peak.max(*other);
            }
        }
    }
}

/// Registers a new connection; its first `status` reports the peaks since the previous `status` on any connection, so a one-request client such as the CLI still sees them.
pub fn open_connection(state: &mut EngineState) -> ConnectionId {
    let peaks = &mut state.peaks;
    peaks.opened += 1;
    let connection = ConnectionId(peaks.opened);
    peaks.meters.insert(connection, Peaks::default());
    connection
}

pub fn close_connection(state: &mut EngineState, connection: ConnectionId) {
    state.peaks.meters.remove(&connection);
}

fn drain_peaks(state: &mut EngineState) {
    let input = take_peak(&state.shared.input_peak_bits);
    let output = take_peak(&state.shared.output_peak_bits);
    let tracks: [f32; MAX_TRACKS] =
        std::array::from_fn(|idx| take_peak(&state.shared.track_input_peak_bits[idx]));
    let mix = MixPeaks::take(&state.shared.mix_meters);
    for peaks in state.peaks.meters.values_mut() {
        peaks.mix.raise(&mix);
        peaks.input = peaks.input.max(input);
        peaks.output = peaks.output.max(output);
        for (peak, track) in peaks.tracks.iter_mut().zip(tracks) {
            *peak = peak.max(track);
        }
    }
}

fn take_peaks(state: &mut EngineState, connection: ConnectionId) -> Peaks {
    drain_peaks(state);
    state
        .peaks
        .meters
        .get_mut(&connection)
        .map(std::mem::take)
        .unwrap_or_default()
}

/// Handles one request; the build warnings of a `load` or a Rack edit come back beside the Response for the caller to log. `ports` and `set_ports` fail here with `audio ports are not available`; `handle_with_ports` serves them.
pub fn handle(state: &mut EngineState, req: Request) -> (Response, Vec<BuildWarning>) {
    handle_from(state, None, IN_PROCESS, req)
}

/// Handles one request as `handle` does, with `ports` serving `ports` and `set_ports`.
pub fn handle_with_ports(
    state: &mut EngineState,
    ports: &dyn AudioPorts,
    req: Request,
) -> (Response, Vec<BuildWarning>) {
    handle_from(state, Some(ports), IN_PROCESS, req)
}

#[allow(clippy::too_many_lines, reason = "one match arm per request")]
fn handle_from(
    state: &mut EngineState,
    ports: Option<&dyn AudioPorts>,
    connection: ConnectionId,
    req: Request,
) -> (Response, Vec<BuildWarning>) {
    let mut warnings = Vec::new();
    let result = match req {
        Request::Status => Ok(status(state, connection)),
        Request::List => Ok(list(state)),
        Request::Load {
            preset,
            track,
            bus,
            master,
        } => target_rack(state, track.as_ref(), bus.as_ref(), master)
            .and_then(|rack| load(state, &preset, rack, &mut warnings)),
        Request::Set {
            param,
            value,
            track,
            bus,
            master,
        } => target_rack(state, track.as_ref(), bus.as_ref(), master)
            .and_then(|rack| set(state, &param, value, rack)),
        Request::Get {
            param,
            track,
            bus,
            master,
        } => target_rack(state, track.as_ref(), bus.as_ref(), master)
            .and_then(|rack| get(state, param.as_deref(), rack)),
        Request::Save { name: None } => save(state),
        Request::Save { name: Some(name) } => save_as(state, &name),
        Request::Modules => Ok(modules()),
        Request::RackAdd {
            r#type,
            after,
            at,
            track,
            bus,
            master,
        } => target_rack(state, track.as_ref(), bus.as_ref(), master)
            .and_then(|rack| rack_add(state, &r#type, after.as_deref(), at, rack, &mut warnings)),
        Request::RackRemove {
            id,
            track,
            bus,
            master,
        } => target_rack(state, track.as_ref(), bus.as_ref(), master)
            .and_then(|rack| rack_remove(state, &id, rack, &mut warnings)),
        Request::RackMove {
            id,
            to,
            track,
            bus,
            master,
        } => target_rack(state, track.as_ref(), bus.as_ref(), master)
            .and_then(|rack| rack_move(state, &id, to, rack, &mut warnings)),
        Request::Tuner => Ok(tuner(state)),
        Request::Mute { on } => Ok(mute(state, on)),
        Request::Bypass {
            id,
            on,
            track,
            bus,
            master,
        } => target_rack(state, track.as_ref(), bus.as_ref(), master)
            .and_then(|rack| bypass(state, &id, on, rack)),
        Request::IrList => ir_list(state),
        Request::RackIr {
            ir,
            id,
            track,
            bus,
            master,
        } => target_rack(state, track.as_ref(), bus.as_ref(), master)
            .and_then(|rack| rack_ir(state, &ir, id.as_deref(), rack, &mut warnings)),
        Request::Ports => ports.ok_or(NO_AUDIO_PORTS.to_string()).map(|ports| {
            let (capture, playback) = port_names(ports);
            json!({"capture": capture, "playback": playback, "connected": connected(state, ports)})
        }),
        Request::SetPorts {
            input,
            output_left,
            output_right,
        } => ports.ok_or(NO_AUDIO_PORTS.to_string()).and_then(|ports| {
            let wanted = Ports {
                input,
                output_left,
                output_right,
            };
            set_ports(state, ports, wanted)
        }),
        Request::Settings => Ok(settings(state)),
        Request::SetSettings {
            bank_kind,
            ir_dir,
            projects_dir,
        } => set_settings(state, bank_kind, ir_dir, projects_dir, &mut warnings),
        Request::ProjectNew { name } => project_new(state, &name),
        Request::ProjectOpen { name, restore } => project_open(state, &name, restore),
        Request::ProjectSave => project_save(state),
        Request::ProjectClose => project_close(state),
        Request::ProjectList => project_list(state),
        Request::ProjectUndo => project_undo(state),
        Request::ProjectRedo => project_redo(state),
        Request::Items => items(state),
        Request::TrackAdd { name } => track_add(state, name.as_deref()),
        Request::TrackRemove { track } => track_remove(state, &track),
        Request::TrackSet {
            track,
            input,
            volume,
            pan,
            mute,
            solo,
            arm,
            name,
            to_master,
        } => track_set(
            state,
            ports,
            &track,
            &TrackChange {
                input,
                volume,
                pan,
                mute,
                solo,
                arm,
                name,
                to_master,
            },
        ),
        Request::TrackSelect { track } => track_select(state, &track),
        Request::Transport { action, position } => transport(state, action, position.as_ref()),
        Request::Loop { start, end, on } => set_loop(state, LoopRegion { start, end, on }),
        Request::Tempo { bpm, meter } => tempo(state, bpm, meter),
        Request::Click { on, count_in } => click(state, on, count_in),
        Request::Metronome {
            on,
            bpm,
            meter,
            volume,
        } => metronome(state, on, bpm, meter, volume),
        Request::MetronomeTap => metronome_tap(state),
        Request::BusAdd { name } => bus_add(state, name.as_deref()),
        Request::BusRemove { bus } => bus_remove(state, &bus),
        Request::BusSet {
            bus,
            name,
            volume,
            pan,
            mute,
            solo,
        } => bus_set(
            state,
            &bus,
            &BusChange {
                name,
                volume,
                pan,
                mute,
                solo,
            },
        ),
        Request::SendSet {
            track,
            bus,
            level,
            pre,
        } => send_set(state, &track, &bus, level, pre),
        Request::MasterSet { volume } => master_set(state, volume),
        Request::ClipReset { track, bus, master } => {
            clip_reset(state, track.as_ref(), bus.as_ref(), master)
        }
        Request::Render { range, bits, name } => render(state, range, bits, name.as_deref()),
        Request::RenderCancel => render_cancel(state),
        Request::ItemMove { ids, delta, track } => item_move(state, &ids, delta, track.as_ref()),
        Request::ItemTrim { id, start, end } => item_trim(state, id, start, end),
        Request::ItemSplit { ids, at } => item_split(state, ids.as_deref(), at),
        Request::ItemDelete { ids } => item_delete(state, &ids),
        Request::ItemTake { id, take } => item_take(state, id, take),
        Request::ItemDuplicate { ids } => item_duplicate(state, &ids),
        Request::ItemCopy { ids } => item_copy(state, &ids),
        Request::ItemPaste { track, at } => item_paste(state, &track, at),
    };
    let response = match result {
        Ok(payload) => Response::Ok(payload),
        Err(error) => Response::Err { error },
    };
    (response, warnings)
}

/// Handles one request line from `connection` and returns the response line, without its newline, and the build warnings as `handle` does; `ports` serves `ports` and `set_ports` as in `handle_with_ports`.
#[allow(
    clippy::missing_panics_doc,
    reason = "the expect is an invariant: a Response always serialises"
)]
pub fn handle_line(
    state: &mut EngineState,
    ports: Option<&dyn AudioPorts>,
    connection: ConnectionId,
    line: &str,
) -> (String, Vec<BuildWarning>) {
    let (response, warnings) = match serde_json::from_str(line) {
        Ok(req) => handle_from(state, ports, connection, req),
        Err(err) => (
            Response::Err {
                error: format!("invalid request: {err}"),
            },
            Vec::new(),
        ),
    };
    let line = serde_json::to_string(&response).expect("a Response always serialises to JSON");
    (line, warnings)
}

/// The frame count the first Rack is prepared for: the larger of the requested and the effective buffer size, since the quantum can settle on either.
pub fn prepared_max_frames(requested_buffer_size: u32, effective_buffer_size: u32) -> usize {
    requested_buffer_size.max(effective_buffer_size) as usize
}

/// Whether the Rack prepared for `prepared` frames must be rebuilt for the quantum in effect: larger blocks would be silent, smaller ones would waste work on oversized FFTs. Never above `SWITCHER_MAX_FRAMES`, which the audio thread cannot run whatever the Rack.
pub fn needs_reprepare(prepared: usize, buffer_size: u32) -> bool {
    let frames = buffer_size as usize;
    prepared != frames && frames <= SWITCHER_MAX_FRAMES
}

/// What `reprepare` swapped in, for the caller to log.
#[derive(Debug, Clone, PartialEq)]
pub enum Reprepare {
    /// The active Preset's Rack, or the passthrough Rack when no Preset is active.
    Built(Vec<BuildWarning>),
    /// The active Preset failed to build, so the passthrough Rack runs and no Preset is active.
    Passthrough { preset: String, error: String },
}

/// Rebuilds the active Preset's Rack, or the empty passthrough Rack when none is active or the build fails, for `buffer_size` frames and pushes it as a `SwapRack`; fails only with `engine busy`, leaving the state unchanged.
///
/// # Errors
///
/// `engine busy` when the command queue has no free slot for the `SwapRack`. A Preset that fails to build is not an error but `Reprepare::Passthrough`.
pub fn reprepare(state: &mut EngineState, buffer_size: u32) -> Result<Reprepare, String> {
    let max_frames = buffer_size as usize;
    if state.queues.commands.slots() < 1 + rebuild_arrangement_slots(state) {
        return Err(ENGINE_BUSY.to_string());
    }
    let mut outcome = rebuild(state, state.sample_rate, max_frames)?;
    state.max_frames = max_frames;
    add_warnings(&mut outcome, rebuild_arrangement(state)?);
    Ok(outcome)
}

/// Calls `change_sample_rate` with the rate JACK last reported, or returns `None` when the live Rack already runs at it.
pub fn follow_sample_rate(state: &mut EngineState) -> Option<Result<Reprepare, String>> {
    let reported = state.shared.sample_rate.load(Ordering::Relaxed);
    if reported == state.sample_rate as u32 {
        return None;
    }
    Some(change_sample_rate(state, reported))
}

/// Rebuilds the live Rack as `reprepare` does, IRs resampled, and the Tuner at `sample_rate`, and sends the switcher the crossfade and Mute ramp lengths at that rate ahead of the Rack; fails only with `engine busy`, leaving the state unchanged.
///
/// # Errors
///
/// `engine busy` when the command queue has fewer than two free slots (one for the fade lengths, one for the `SwapRack`), or when either push fails. A Preset that fails to build is not an error but `Reprepare::Passthrough`.
pub fn change_sample_rate(state: &mut EngineState, sample_rate: u32) -> Result<Reprepare, String> {
    let sample_rate = sample_rate as f32;
    let fades = FadeLengths::at(sample_rate, state.config.crossfade_ms);
    let metronome_slots = usize::from(state.metronome.on);
    if state.queues.commands.slots() < 2 + metronome_slots + rebuild_arrangement_slots(state)
        || state
            .queues
            .commands
            .push(Command::SetFadeLengths(fades))
            .is_err()
    {
        return Err(ENGINE_BUSY.to_string());
    }
    let mut outcome = rebuild(state, sample_rate, state.max_frames)?;
    state.sample_rate = sample_rate;
    state.tuner.set_sample_rate(sample_rate);
    if state.metronome.on {
        let settings = practice_settings(state, true);
        state
            .queues
            .commands
            .push(Command::Metronome(settings))
            .map_err(|_| ENGINE_BUSY.to_string())?;
    }
    add_warnings(&mut outcome, rebuild_arrangement(state)?);
    Ok(outcome)
}

fn add_warnings(outcome: &mut Reprepare, more: Vec<BuildWarning>) {
    if let Reprepare::Built(warnings) = outcome {
        warnings.extend(more);
    }
}

fn rebuild(
    state: &mut EngineState,
    sample_rate: f32,
    max_frames: usize,
) -> Result<Reprepare, String> {
    let built = state.active.as_ref().map(|(_, preset)| {
        build_rack(&state.config, sample_rate, max_frames, preset)
            .map_err(|err| (preset.name.clone(), err.to_string()))
    });
    let (rack, outcome) = match built {
        Some(Ok((rack, warnings))) => (rack, Reprepare::Built(warnings)),
        Some(Err((preset, error))) => (
            passthrough_rack(sample_rate, max_frames),
            Reprepare::Passthrough { preset, error },
        ),
        None => (
            passthrough_rack(sample_rate, max_frames),
            Reprepare::Built(Vec::new()),
        ),
    };
    swap_rack(&mut state.queues, rack)?;
    state.revision += 1;
    if let Reprepare::Passthrough { .. } = outcome {
        state.active = None;
        state.modified = false;
    }
    Ok(outcome)
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
    /// # Errors
    ///
    /// The JSON error when `line` is not a Response.
    pub fn from_line(line: &str) -> Result<Response, serde_json::Error> {
        serde_json::from_str(line)
    }
}

fn status(state: &mut EngineState, connection: ConnectionId) -> Value {
    let peaks = take_peaks(state, connection);
    let shared = &state.shared;
    let connections: Vec<Value> = state
        .connections
        .iter()
        .map(|(from, to)| json!({"from": from, "to": to}))
        .collect();
    let mut payload = json!({
        "preset": state.active.as_ref().map(|(_, preset)| &preset.name),
        "program": active_program(state),
        "sample_rate": shared.sample_rate.load(Ordering::Relaxed),
        "buffer_size": shared.buffer_size.load(Ordering::Relaxed),
        "prepared_frames": state.max_frames,
        "revision": state.revision,
        "muted": shared.muted.load(Ordering::Relaxed),
        "modified": state.modified,
        "input_peak_db": peak_db(peaks.input),
        "output_peak_db": peak_db(peaks.output),
        "xruns": shared.xruns.load(Ordering::Relaxed),
        "callback_max_us": shared.callback_ns_max.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        "callback_last_us": shared.callback_ns_last.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        "connections": connections,
        "metronome": metronome_status(state),
    });
    if let Some(fields) = payload.as_object_mut() {
        fields.extend(status_fields(state, &peaks.tracks, &peaks.mix));
    }
    payload
}

fn tuner(state: &EngineState) -> Value {
    match state.tuner.reading() {
        Some(reading) => json!({
            "hz": (f64::from(reading.hz) * 10.0).round() / 10.0,
            "note": reading.note,
            "cents": reading.cents,
            "confident": reading.confident,
        }),
        None => json!({"hz": null, "note": null, "cents": null, "confident": false}),
    }
}

fn mute(state: &EngineState, on: bool) -> Value {
    state.shared.muted.store(on, Ordering::Relaxed);
    json!({ "muted": on })
}

pub(crate) fn peak_db(peak: f32) -> f64 {
    let db = (20.0 * f64::from(peak).log10()).max(SILENCE_DB);
    (db * 10.0).round() / 10.0
}

fn list(state: &EngineState) -> Value {
    let entries: Vec<Value> = state
        .bank
        .entries
        .iter()
        .enumerate()
        .map(|(program, entry)| match entry {
            BankEntry::Valid { preset, .. } => {
                json!({"program": program, "name": preset.name, "valid": true})
            }
            BankEntry::Invalid { file, error } => {
                json!({"program": program, "name": file_stem(file), "valid": false, "error": error})
            }
        })
        .collect();
    json!({"active": active_program(state), "entries": entries})
}

fn load(
    state: &mut EngineState,
    key: &str,
    rack: Option<ProjectRack>,
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    if rack.is_some_and(ProjectRack::is_stereo) {
        return Err(PRESET_INTO_TRACK.to_string());
    }
    let (program, entry) = state
        .bank
        .by_name_or_number(key)
        .ok_or_else(|| format!("no preset '{key}'"))?;
    let (file, preset) = match entry {
        BankEntry::Valid { file, preset } => (file.clone(), preset.clone()),
        BankEntry::Invalid { error, .. } => return Err(error.clone()),
    };
    let payload = json!({"program": program, "name": preset.name});
    if let Some(rack) = rack {
        set_rack_modules(state, rack, preset.modules, warnings)?;
        return Ok(payload);
    }
    install_preset(state, file, preset, warnings)?;
    state.modified = false;
    state.saved.clear();
    Ok(payload)
}

/// Builds `preset`'s Rack, swaps it in and makes it the active Preset; on an error the state is unchanged.
fn install_preset(
    state: &mut EngineState,
    file: PathBuf,
    preset: Preset,
    warnings: &mut Vec<BuildWarning>,
) -> Result<(), String> {
    let (rack, build_warnings) = build_preset(state, &preset)?;
    *warnings = build_warnings;
    swap_in(state, file, preset, rack)
}

fn build_preset(state: &EngineState, preset: &Preset) -> Result<(Rack, Vec<BuildWarning>), String> {
    build_rack(&state.config, state.sample_rate, state.max_frames, preset)
        .map_err(|err| err.to_string())
}

/// Swaps `rack`, built from `preset`, in and makes `preset` the active Preset; on an error the state is unchanged.
fn swap_in(
    state: &mut EngineState,
    file: PathBuf,
    preset: Preset,
    rack: Rack,
) -> Result<(), String> {
    swap_rack(&mut state.queues, rack)?;
    state.revision += 1;
    state.active = Some((file, preset));
    Ok(())
}

/// The empty Rack, which sends the input to both outputs unchanged.
#[allow(
    clippy::missing_panics_doc,
    reason = "the expect is an invariant: an empty Rack breaks no layout rule"
)]
pub fn passthrough_rack(sample_rate: f32, max_frames: usize) -> Rack {
    let mut rack = Rack::new(Vec::new()).expect("an empty Rack has no layout to violate");
    rack.prepare(sample_rate, max_frames);
    rack
}

fn set(
    state: &mut EngineState,
    address: &str,
    value: f64,
    rack: Option<ProjectRack>,
) -> Result<Value, String> {
    if !value.is_finite() {
        return Err("value must be a finite number".to_string());
    }
    let address = ParamAddress::parse(address).map_err(|err| err.to_string())?;
    let preset = rack_or_live_preset(state, rack)?;
    let (module, param) = resolve(&preset, &address).map_err(|err| err.to_string())?;
    let spec = param_spec(&preset.modules[module], param);
    let clamped = value < f64::from(spec.min) || value > f64::from(spec.max);
    let value = if spec.integer { value.round() } else { value };
    let applied = (value as f32).clamp(spec.min, spec.max);
    let payload = json!({
        "param": format!("{}.{}", address.module, address.param),
        "value": shortest_f64(applied),
        "clamped": clamped,
    });
    if let Some(rack) = rack {
        let command = param_command(rack, module, param, applied);
        change_rack_module(state, rack, command, |modules| {
            modules[module].params[param].1 = applied;
        })?;
        return Ok(payload);
    }
    let (_, preset) = state.active.as_mut().ok_or(NO_PRESET)?;
    state
        .queues
        .commands
        .push(Command::SetParam {
            module,
            param,
            value: applied,
        })
        .map_err(|_| ENGINE_BUSY.to_string())?;
    preset.modules[module].params[param].1 = applied;
    state.revision += 1;
    state.modified = true;
    Ok(payload)
}

fn bypass(
    state: &mut EngineState,
    id: &str,
    on: bool,
    rack: Option<ProjectRack>,
) -> Result<Value, String> {
    if let Some(rack) = rack {
        let module = module_index(&rack_preset(state, rack).modules, id)?;
        let command = bypass_command(rack, module, on);
        change_rack_module(state, rack, command, |modules| {
            modules[module].bypass = on;
        })?;
        return Ok(json!({ "bypass": on }));
    }
    let (_, preset) = state.active.as_mut().ok_or(NO_PRESET)?;
    let module = module_index(&preset.modules, id)?;
    state
        .queues
        .commands
        .push(Command::SetBypass { module, on })
        .map_err(|_| ENGINE_BUSY.to_string())?;
    preset.modules[module].bypass = on;
    state.revision += 1;
    state.modified = true;
    Ok(json!({ "bypass": on }))
}

fn save(state: &mut EngineState) -> Result<Value, String> {
    let (file, preset) = state.active.as_ref().ok_or(NO_PRESET)?;
    if state.bank.position(file).is_none() {
        return Err(ACTIVE_REMOVED.to_string());
    }
    let text = preset.to_toml();
    write_through(file, &text).map_err(|err| format!("cannot save {}: {err}", file.display()))?;
    let payload = json!({"path": file.display().to_string()});
    let saved = Preset::parse(&text).expect("a saved Preset parses back");
    state.saved.insert(file.clone(), saved.clone());
    if let Some((_, active)) = state.active.as_mut() {
        *active = saved;
    }
    state.revision += 1;
    state.modified = false;
    Ok(payload)
}

/// Writes the live Preset, renamed `name`, to a new `<NN>-<slug>.toml` in the Bank dir and makes it the active Preset; the name reaches the path only through the slug.
fn save_as(state: &mut EngineState, name: &str) -> Result<Value, String> {
    let (_, live) = state.active.as_ref().ok_or(NO_PRESET)?;
    let name: String = name.chars().filter(|c| !c.is_ascii_control()).collect();
    let name = name.trim();
    let slug = slug(name);
    if slug.is_empty() {
        return Err(EMPTY_PRESET_NAME.to_string());
    }
    let dir = state.bank.dir.clone();
    let cannot_read = |err: io::Error| format!("cannot save {}: {err}", dir.display());
    let bank = Bank::load_dir(&dir).map_err(cannot_read)?;
    let wanted = name.to_lowercase();
    if bank.entries.iter().any(
        |entry| matches!(entry, BankEntry::Valid { preset, .. } if preset.name.to_lowercase() == wanted),
    ) {
        return Err(format!("a preset named '{name}' already exists"));
    }
    let text = Preset {
        name: name.to_string(),
        modules: live.modules.clone(),
    }
    .to_toml();
    let file = write_next_free(&dir, next_prefix(&bank), &slug, &text)?;
    state.bank = Bank::load_dir(&dir).map_err(cannot_read)?;
    let saved = Preset::parse(&text).expect("a saved Preset parses back");
    state.saved.insert(file.clone(), saved.clone());
    state.active = Some((file.clone(), saved));
    state.revision += 1;
    state.modified = false;
    Ok(json!({"path": file.display().to_string(), "program": active_program(state)}))
}

/// Writes `text` to the first of `SAVE_AS_TRIES` names `<NN>-<slug>.toml` from `first` on that no file takes, since one may appear after the Bank was read.
fn write_next_free(dir: &Path, first: u64, slug: &str, text: &str) -> Result<PathBuf, String> {
    let mut file = PathBuf::new();
    for prefix in first..=first.saturating_add(SAVE_AS_TRIES - 1) {
        file = dir.join(format!("{prefix:02}-{slug}.toml"));
        match write_new(&file, text, |tmp, path| fs::hard_link(tmp, path)) {
            Ok(()) => return Ok(file),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("cannot save {}: {err}", file.display())),
        }
    }
    Err(format!("cannot save {}: file exists", file.display()))
}

/// The name lowercased, ASCII letters and digits kept, every other run of characters one `-`, no `-` at either end.
pub(crate) fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// One more than the highest number before the first `-` among the Bank's file names, at most `u64::MAX`.
fn next_prefix(bank: &Bank) -> u64 {
    bank.entries
        .iter()
        .filter_map(|entry| {
            let name = entry.file().file_name()?.to_str()?;
            let (prefix, _) = name.split_once('-')?;
            if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            prefix.parse::<u64>().ok()
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

/// Replaces the file `path` names, through a symlink, by a temp file and rename beside it, keeping the old file's mode.
fn write_through(path: &Path, text: &str) -> io::Result<()> {
    let target = if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    let permissions = match fs::metadata(&target) {
        Ok(meta) => Some(meta.permissions()),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let tmp = temp_path(&target);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    if let Some(permissions) = &permissions {
        options.mode(permissions.mode());
    }
    let mut file = options.open(&tmp)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| match permissions {
            Some(permissions) => file.set_permissions(permissions),
            None => Ok(()),
        })
        .and_then(|()| fs::rename(&tmp, &target));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Creates the file `path` names with `text` by `link`ing a temp file beside it to `path` (`save` with `as` passes `fs::hard_link`), so a file or symlink already there fails with `AlreadyExists` and is left as it is. When the filesystem has no hard links (EPERM, EOPNOTSUPP, EXDEV), `path` is created and written directly, still never replacing a file.
///
/// # Errors
///
/// `AlreadyExists` when `path` already names a file or symlink; the I/O error when the temp file or `path` cannot be created or written.
pub fn write_new(
    path: &Path,
    text: &str,
    link: impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let tmp = temp_path(path);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| link(&tmp, path));
    let _ = fs::remove_file(&tmp);
    match written {
        Err(err) if has_no_hard_links(&err) => create_and_write(path, text),
        written => written,
    }
}

pub(crate) fn has_no_hard_links(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::Unsupported | ErrorKind::CrossesDevices
    ) || err.raw_os_error() == Some(EPERM)
}

fn create_and_write(path: &Path, text: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let written = file.write_all(text.as_bytes());
    if written.is_err() {
        let _ = fs::remove_file(path);
    }
    written
}

/// `<file name>.<pid>.<n>.tmp` beside `target`, unique within this process.
pub(crate) fn temp_path(target: &Path) -> PathBuf {
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let n = SAVES.fetch_add(1, Ordering::Relaxed);
    let mut name = target.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    target.with_file_name(name)
}

fn modules() -> Value {
    let modules: Vec<Value> = ModuleRegistry::default()
        .specs()
        .map(|spec| {
            let params: Vec<Value> = spec
                .params
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name,
                        "min": shortest_f64(p.min),
                        "max": shortest_f64(p.max),
                        "default": shortest_f64(p.default),
                        "unit": unit_text(p.unit),
                        "integer": p.integer,
                        "labels": p.labels,
                    })
                })
                .collect();
            json!({"type": spec.type_name, "channels": channels_text(spec.channels), "params": params})
        })
        .collect();
    json!({ "modules": modules })
}

fn rack_add(
    state: &mut EngineState,
    type_name: &str,
    after: Option<&str>,
    at: Option<usize>,
    rack: Option<ProjectRack>,
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    let (ir_dir, bank_dir) = (state.config.ir_dir.clone(), state.config.bank_dir.clone());
    let is_stereo = rack.is_some_and(ProjectRack::is_stereo);
    if is_stereo && type_name == CABINET_TYPE {
        return Err(ProjectError::CabinetOnBus.to_string());
    }
    edit_rack(state, rack, warnings, |preset| {
        let spec = ModuleRegistry::default()
            .spec(type_name)
            .ok_or_else(|| format!("unknown module type '{type_name}'"))?;
        let modules = &mut preset.modules;
        let index = match (after, at) {
            (Some(_), Some(_)) => return Err(AFTER_OR_AT.to_string()),
            (Some(after), None) => module_index(modules, after)? + 1,
            (None, Some(at)) => at.min(modules.len()),
            (None, None) => modules.len(),
        };
        let id = free_id(modules, type_name);
        modules.insert(
            index,
            PresetModule {
                id: id.clone(),
                type_name: type_name.to_string(),
                ir: None,
                bypass: false,
                params: spec
                    .params
                    .iter()
                    .map(|p| (p.name.to_string(), p.default))
                    .collect(),
            },
        );
        if !is_stereo {
            preset.validate_layout().map_err(|err| err.to_string())?;
        }
        if type_name == CABINET_TYPE {
            let ir = CABINET_IRS
                .into_iter()
                .find(|ir| ir_path(&ir_dir, &bank_dir, ir).is_file())
                .ok_or(CABINET_NEEDS_IR)?;
            preset.modules[index].ir = Some(ir.to_string());
        }
        Ok(json!({ "id": id }))
    })
}

fn rack_remove(
    state: &mut EngineState,
    id: &str,
    rack: Option<ProjectRack>,
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    edit_rack(state, rack, warnings, |preset| {
        let modules = &mut preset.modules;
        modules.remove(module_index(modules, id)?);
        Ok(json!({ "id": id }))
    })
}

fn rack_move(
    state: &mut EngineState,
    id: &str,
    to: usize,
    rack: Option<ProjectRack>,
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    edit_rack(state, rack, warnings, |preset| {
        let modules = &mut preset.modules;
        let module = modules.remove(module_index(modules, id)?);
        let to = to.min(modules.len());
        modules.insert(to, module);
        Ok(json!({ "id": id, "to": to }))
    })
}

fn edit_rack(
    state: &mut EngineState,
    rack: Option<ProjectRack>,
    warnings: &mut Vec<BuildWarning>,
    edit: impl FnOnce(&mut Preset) -> Result<Value, String>,
) -> Result<Value, String> {
    if let Some(rack) = rack {
        let mut preset = rack_preset(state, rack);
        let payload = edit(&mut preset)?;
        set_rack_modules(state, rack, preset.modules, warnings)?;
        return Ok(payload);
    }
    let (file, preset) = state.active.as_ref().ok_or(NO_PRESET)?;
    let (file, mut preset) = (file.clone(), preset.clone());
    let payload = edit(&mut preset)?;
    install_preset(state, file, preset, warnings)?;
    state.modified = true;
    Ok(payload)
}

fn ir_list(state: &EngineState) -> Result<Value, String> {
    let irs = list_irs(&state.config.ir_dir)?;
    Ok(json!({"ir_dir": state.config.ir_dir.display().to_string(), "irs": irs}))
}

/// Sets the cabinet's `ir` and swaps the rebuilt Rack in; an IR that fails to load is an error and changes nothing.
fn rack_ir(
    state: &mut EngineState,
    ir: &str,
    id: Option<&str>,
    rack_target: Option<ProjectRack>,
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    if rack_target.is_some_and(ProjectRack::is_stereo) {
        return Err(ProjectError::CabinetOnBus.to_string());
    }
    let (file, mut preset) = match rack_target {
        Some(rack) => (PathBuf::new(), rack_preset(state, rack)),
        None => state.active.clone().ok_or(NO_PRESET)?,
    };
    let index = cabinet_index(&preset.modules, id)?;
    if !is_relative_name(ir) {
        return Err(format!("invalid ir name '{ir}'"));
    }
    preset.modules[index].ir = Some(ir.to_string());
    let id = preset.modules[index].id.clone();
    let (rack, build_warnings) = build_preset(state, &preset)?;
    if let Some(error) = build_warnings
        .iter()
        .find_map(|BuildWarning::Ir { id: module, error }| (*module == id).then_some(error))
    {
        return Err(error.clone());
    }
    if let Some(target) = rack_target {
        drop(rack);
        set_rack_modules(state, target, preset.modules, warnings)?;
        return Ok(json!({"id": id, "ir": ir}));
    }
    *warnings = build_warnings;
    swap_in(state, file, preset, rack)?;
    state.modified = true;
    Ok(json!({"id": id, "ir": ir}))
}

fn cabinet_index(modules: &[PresetModule], id: Option<&str>) -> Result<usize, String> {
    let Some(id) = id else {
        return modules
            .iter()
            .position(|m| m.type_name == CABINET_TYPE)
            .ok_or_else(|| NO_CABINET.to_string());
    };
    let index = module_index(modules, id)?;
    if modules[index].type_name != CABINET_TYPE {
        return Err(format!("module '{id}' is not a cabinet"));
    }
    Ok(index)
}

fn module_index(modules: &[PresetModule], id: &str) -> Result<usize, String> {
    modules
        .iter()
        .position(|m| m.id == id)
        .ok_or_else(|| ResolveError::UnknownModule(id.to_string()).to_string())
}

#[allow(
    clippy::maybe_infinite_iter,
    reason = "a finite Rack leaves some suffix free, so find returns"
)]
fn free_id(modules: &[PresetModule], type_name: &str) -> String {
    let is_taken = |id: &str| modules.iter().any(|m| m.id == id);
    if !is_taken(type_name) {
        return type_name.to_string();
    }
    (2..)
        .map(|n| format!("{type_name}-{n}"))
        .find(|id| !is_taken(id))
        .expect("a finite Rack leaves some suffix free")
}

fn port_names(ports: &dyn AudioPorts) -> (Vec<String>, Vec<String>) {
    let names = |available: Vec<AvailablePort>| {
        let mut names: Vec<String> = available.into_iter().map(|port| port.name).collect();
        names.sort();
        names
    };
    let (capture, playback) = ports.audio_ports();
    (names(capture), names(playback))
}

fn connected(state: &EngineState, ports: &dyn AudioPorts) -> Value {
    let own = ports.own_ports();
    let linked = |own_port: &str, is_input: bool| {
        state.connections.iter().find_map(|(from, to)| {
            if is_input {
                (to == own_port).then_some(from)
            } else {
                (from == own_port).then_some(to)
            }
        })
    };
    json!({
        "input": linked(&own.input, true),
        "output_left": linked(&own.output_left, false),
        "output_right": linked(&own.output_right, false),
    })
}

/// Links the Daemon's ports to the named ports, after checking each is listed and storing them in `state.toml`; a link that fails is reported once the others are made.
fn set_ports(
    state: &mut EngineState,
    ports: &dyn AudioPorts,
    wanted: Ports,
) -> Result<Value, String> {
    let (capture, playback) = port_names(ports);
    if !capture.contains(&wanted.input) {
        return Err(format!("no capture port '{}'", wanted.input));
    }
    for name in [&wanted.output_left, &wanted.output_right] {
        if !playback.contains(name) {
            return Err(format!("no playback port '{name}'"));
        }
    }
    let mut saved = state.config.state.clone();
    saved.ports = Some(wanted.clone());
    save_state(&state.config, &saved)?;
    state.config.state = saved;
    let plan = connect_plan(&state.connections, &ports.own_ports(), &wanted);
    state.config.ports = wanted;
    state.revision += 1;
    let mut failures = Vec::new();
    for (from, to) in plan.disconnect {
        match ports.disconnect(&from, &to) {
            Ok(()) => state
                .connections
                .retain(|link| *link != (from.clone(), to.clone())),
            Err(err) => failures.push(format!("cannot disconnect {from} -> {to}: {err}")),
        }
    }
    for (from, to) in plan.connect {
        match ports.connect(&from, &to) {
            Ok(()) => state.connections.push((from, to)),
            Err(err) => failures.push(format!("cannot connect {from} -> {to}: {err}")),
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(json!({ "connected": connected(state, ports) }))
}

pub(crate) fn save_state(config: &Config, saved: &SavedState) -> Result<(), String> {
    write_state(&config.state_file, saved)
        .map_err(|err| format!("cannot save {}: {err}", config.state_file.display()))
}

fn settings(state: &EngineState) -> Value {
    json!({
        "bank_kind": state.config.bank_kind.to_string(),
        "bank_dir": state.config.bank_dir.display().to_string(),
        "ir_dir": state.config.ir_dir.display().to_string(),
        "projects_dir": state.config.projects_dir.display().to_string(),
    })
}

/// Stores `bank_kind` and `ir_dir` in `state.toml` and applies them: a changed `bank_kind` records the Bank directory in use under the current kind in `[banks]`, switches to the new kind's directory (seeded when missing) and plays Program 0 (the passthrough Rack when it does not build). On an error the state and `state.toml` are unchanged; only a seeded Bank directory may stay.
fn set_settings(
    state: &mut EngineState,
    bank_kind: Option<BankKind>,
    ir_dir: Option<PathBuf>,
    projects_dir: Option<PathBuf>,
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    if projects_dir.as_ref().is_some_and(|dir| !dir.is_absolute()) {
        return Err(PROJECTS_DIR_NOT_ABSOLUTE.to_string());
    }
    if let Some(dir) = ir_dir
        .as_ref()
        .filter(|dir| !dir.is_absolute() || !dir.is_dir())
    {
        return Err(format!("ir_dir '{}' is not a directory", dir.display()));
    }
    let switch = bank_kind.filter(|kind| *kind != state.config.bank_kind);
    if switch.is_some() && state.config.bank_dir_in_config {
        return Err(BANK_DIR_IN_CONFIG.to_string());
    }
    let ir_dir = ir_dir.filter(|dir| state.config.state.ir_dir.as_ref() != Some(dir));
    let projects_dir = projects_dir.filter(|dir| *dir != state.config.projects_dir);
    if switch.is_none() && ir_dir.is_none() && projects_dir.is_none() {
        return Ok(settings(state));
    }
    let mut config = state.config.clone();
    if let Some(dir) = projects_dir {
        create_projects_dir(&dir)?;
        config.state.projects_dir = Some(dir.clone());
        config.projects_dir = dir;
    }
    if let Some(dir) = ir_dir {
        config.state.ir_dir = Some(dir.clone());
        config.ir_dir = dir;
    }
    let Some(kind) = switch else {
        save_state(&config, &config.state)?;
        state.config = config;
        state.revision += 1;
        return Ok(settings(state));
    };
    let bank = switched_bank(state, kind)?;
    config
        .state
        .banks
        .insert(config.bank_kind, config.bank_dir.clone());
    config.state.banks.insert(kind, bank.dir.clone());
    config.state.bank_kind = Some(kind);
    if !config.ir_dir_in_config && config.state.ir_dir.is_none() {
        config.ir_dir.clone_from(&bank.dir);
    }
    config.bank_kind = kind;
    config.bank_dir.clone_from(&bank.dir);
    let (rack, active, build_warnings) = first_program(state, &config, &bank);
    swap_saving_state(state, &config, rack)?;
    *warnings = build_warnings;
    state.config = config;
    state.bank = bank;
    state.active = active;
    state.modified = false;
    state.saved.clear();
    state.revision += 1;
    Ok(settings(state))
}

fn switched_bank(state: &EngineState, kind: BankKind) -> Result<Bank, String> {
    let dir = state.config.kind_bank_dir(kind);
    ensure_bank(&dir, kind, state.sample_rate as u32)
        .map_err(|err| format!("cannot create bank {}: {err}", dir.display()))?;
    Bank::load_dir(&dir).map_err(|err| format!("cannot read bank {}: {err}", dir.display()))
}

/// Program 0's Rack built with `config`, and the Preset it plays; the passthrough Rack and no Preset when Program 0 is missing, invalid or does not build.
fn first_program(
    state: &EngineState,
    config: &Config,
    bank: &Bank,
) -> (Rack, Option<(PathBuf, Preset)>, Vec<BuildWarning>) {
    if let Some(BankEntry::Valid { file, preset }) = bank.entries.first()
        && let Ok((rack, warnings)) =
            build_rack(config, state.sample_rate, state.max_frames, preset)
    {
        return (rack, Some((file.clone(), preset.clone())), warnings);
    }
    let rack = passthrough_rack(state.sample_rate, state.max_frames);
    (rack, None, Vec::new())
}

/// Saves `config.state` to `state.toml`, then swaps `rack` in; when the swap is refused, the previous `state.toml` (or its absence) is put back.
fn swap_saving_state(state: &mut EngineState, config: &Config, rack: Rack) -> Result<(), String> {
    let cannot_save =
        |err: io::Error| format!("cannot save {}: {err}", config.state_file.display());
    let previous = read_state_bytes(&config.state_file).map_err(cannot_save)?;
    save_state(config, &config.state)?;
    let Err(busy) = swap_rack(&mut state.queues, rack) else {
        return Ok(());
    };
    restore_state(&config.state_file, previous.as_deref()).map_err(|err| {
        format!(
            "{busy}; cannot restore {}: {err}",
            config.state_file.display()
        )
    })?;
    Err(busy)
}

/// The Project Rack's Modules as a Preset, or the active Preset for the live Rack.
fn rack_or_live_preset(state: &EngineState, rack: Option<ProjectRack>) -> Result<Preset, String> {
    match rack {
        Some(rack) => Ok(rack_preset(state, rack)),
        None => Ok(state.active.as_ref().ok_or(NO_PRESET)?.1.clone()),
    }
}

fn get(state: &EngineState, key: Option<&str>, rack: Option<ProjectRack>) -> Result<Value, String> {
    let preset = rack_or_live_preset(state, rack)?;
    let preset = &preset;
    let selected: Vec<(usize, usize)> = match key {
        None => preset
            .modules
            .iter()
            .enumerate()
            .flat_map(|(module, m)| (0..m.params.len()).map(move |param| (module, param)))
            .collect(),
        Some(key) if key.contains('.') => {
            let address = ParamAddress::parse(key).map_err(|err| err.to_string())?;
            vec![resolve(preset, &address).map_err(|err| err.to_string())?]
        }
        Some(id) => {
            let module = preset
                .modules
                .iter()
                .position(|m| m.id == id)
                .ok_or_else(|| ResolveError::UnknownModule(id.to_string()).to_string())?;
            (0..preset.modules[module].params.len())
                .map(|param| (module, param))
                .collect()
        }
    };
    let params: Vec<Value> = selected
        .into_iter()
        .map(|(module, param)| param_json(&preset.modules[module], param))
        .collect();
    Ok(json!({ "params": params }))
}

fn param_json(module: &PresetModule, param: usize) -> Value {
    let spec = param_spec(module, param);
    let (name, value) = &module.params[param];
    let mut entry = json!({
        "module": module.id,
        "type": module.type_name,
        "param": name,
        "value": shortest_f64(*value),
        "unit": unit_text(spec.unit),
        "min": shortest_f64(spec.min),
        "max": shortest_f64(spec.max),
        "default": shortest_f64(spec.default),
        "integer": spec.integer,
        "labels": spec.labels,
        "bypass": module.bypass,
    });
    if module.type_name == CABINET_TYPE {
        entry["ir"] = json!(module.ir);
    }
    entry
}

fn param_spec(module: &PresetModule, param: usize) -> &'static ParamSpec {
    let spec = ModuleRegistry::default()
        .spec(&module.type_name)
        .expect("a parsed Preset names only registered Module types");
    &spec.params[param]
}

fn unit_text(unit: Unit) -> &'static str {
    match unit {
        Unit::Linear => "",
        Unit::Db => "dB",
        Unit::Ms => "ms",
        Unit::Hz => "Hz",
        Unit::Percent => "%",
    }
}

fn channels_text(channels: Channels) -> &'static str {
    match channels {
        Channels::Mono => "mono",
        Channels::Stereo => "stereo",
        Channels::MonoToStereo => "mono_to_stereo",
    }
}

fn active_program(state: &EngineState) -> Option<usize> {
    state
        .active
        .as_ref()
        .and_then(|(file, _)| state.bank.position(file))
}

fn file_stem(file: &Path) -> String {
    file.file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
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
                Some(Value::String(error)) => Ok(Response::Err { error }),
                _ => Err(D::Error::custom(
                    "error response without an \"error\" string",
                )),
            },
            _ => Err(D::Error::custom("response without a boolean \"ok\"")),
        }
    }
}
