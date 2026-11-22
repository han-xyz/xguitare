use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::json;

use crate::build::{ENGINE_BUSY, build_rack, swap_rack};
use crate::engine::{
    Command, EngineQueues, EngineShared, FadeLengths, MeterPoint, MixMeters, SWITCHER_MAX_FRAMES,
    take_peak,
};
use crate::item_control::{
    item_copy, item_delete, item_duplicate, item_move, item_paste, item_split, item_take, item_trim,
};
use crate::metronome::Metronome;
use crate::metronome_control::{metronome, metronome_status, metronome_tap, practice_settings};
use crate::mixer_control::{
    BusChange, bus_add, bus_remove, bus_set, clip_reset, master_set, send_set,
};
use crate::ports::AudioPorts;
use crate::project::{MAX_BUSES, MAX_TRACKS};
use crate::project_control::{
    TrackChange, items, project_close, project_list, project_new, project_open, project_save,
    status_fields, target_rack, track_add, track_remove, track_select, track_set,
};
use crate::project_state::{Projects, ReadProject};
use crate::project_sync::{rebuild_arrangement, rebuild_arrangement_slots};
pub use crate::rack_control::passthrough_rack;
use crate::rack_control::{
    active_program, bypass, get, ir_list, list, load, modules, rack_add, rack_ir, rack_move,
    rack_remove, save, save_as, set,
};
use crate::render_control::{render, render_cancel};
use crate::settings_control::{connected, port_names, set_ports, set_settings, settings};
use crate::take_peaks::peak_page;
use crate::takes_control::{takes_clean, takes_unused};
use crate::transport_control::{click, set_loop, tempo, transport};
use crate::tuner::Tuner;
use crate::undo_control::{project_redo, project_undo};
use crate::{Bank, BuildWarning, Config, Ports, Preset};
use xguitare_protocol::{CODE_STILL_CLOSING, PROTOCOL_VERSION, STILL_CLOSING};
use xguitare_protocol::{LoopRegion, Request, Response, SILENCE_DB, Value};
use xguitare_protocol::{PortConnection, StatusReply, to_payload};

const NANOS_PER_MICRO: u64 = 1_000;
const NO_AUDIO_PORTS: &str = "audio ports are not available";

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

impl EngineState {
    /// No active Preset, no connections, revision 1, unmodified; the rest starts empty.
    pub fn new(
        bank: Bank,
        shared: Arc<EngineShared>,
        queues: EngineQueues,
        tuner: Tuner,
        config: Config,
        sample_rate: f32,
        max_frames: usize,
    ) -> EngineState {
        EngineState {
            bank,
            active: None,
            shared,
            queues,
            tuner,
            config,
            connections: Vec::new(),
            sample_rate,
            max_frames,
            revision: 1,
            modified: false,
            peaks: PeakMeters::default(),
            saved: HashMap::new(),
            projects: Projects::default(),
            metronome: Metronome::default(),
        }
    }

    #[must_use]
    pub fn with_active(mut self, active: Option<(PathBuf, Preset)>) -> EngineState {
        self.active = active;
        self
    }

    #[must_use]
    pub fn with_connections(mut self, connections: Vec<(String, String)>) -> EngineState {
        self.connections = connections;
        self
    }

    #[must_use]
    pub fn with_revision(mut self, revision: u64) -> EngineState {
        self.revision = revision;
        self
    }
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
    handle_from(state, None, IN_PROCESS, req, None)
}

/// Handles one request as `handle` does, with `ports` serving `ports` and `set_ports`.
pub fn handle_with_ports(
    state: &mut EngineState,
    ports: &dyn AudioPorts,
    req: Request,
) -> (Response, Vec<BuildWarning>) {
    handle_from(state, Some(ports), IN_PROCESS, req, None)
}

#[allow(clippy::too_many_lines, reason = "one match arm per request")]
fn handle_from(
    state: &mut EngineState,
    ports: Option<&dyn AudioPorts>,
    connection: ConnectionId,
    req: Request,
    read: Option<ReadProject>,
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
        Request::ProjectOpen { name, restore } => project_open(state, &name, restore, read),
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
        Request::TakePeaks { file, first, count } => peak_page(state, &file, first, count),
        Request::TakesUnused => takes_unused(state),
        Request::TakesClean { files } => takes_clean(state, &files),
    };
    let response = match result {
        Ok(payload) => Response::Ok(payload),
        Err(error) => error_response(error),
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
    let (response, warnings) = match parse_line(line) {
        Ok(request) => handle_request(state, ports, connection, request, None),
        Err(response) => (response, Vec::new()),
    };
    let line = serde_json::to_string(&response).expect("a Response always serialises to JSON");
    (line, warnings)
}

/// Parses one request line, or returns the error Response `handle_line` answers it with; reads no state, so it runs outside the state lock.
///
/// # Errors
///
/// `invalid request: <serde error>` when the line is not a known request.
pub fn parse_line(line: &str) -> Result<Request, Response> {
    serde_json::from_str(line).map_err(|err| Response::err(format!("invalid request: {err}")))
}

/// Handles one parsed request from `connection` as `handle_line` does, returning the Response itself; a `project_open` given the `read` of its `project_control::begin_project_read` ticket installs that read unless a Project was opened or closed, or `projects_dir` changed, since the ticket, and then reads again.
pub fn handle_request(
    state: &mut EngineState,
    ports: Option<&dyn AudioPorts>,
    connection: ConnectionId,
    request: Request,
    read: Option<ReadProject>,
) -> (Response, Vec<BuildWarning>) {
    handle_from(state, ports, connection, request, read)
}

/// The error Response for `error`, with its `code` when it has one.
pub(crate) fn error_response(error: String) -> Response {
    if error == STILL_CLOSING {
        Response::err_code(error, CODE_STILL_CLOSING)
    } else {
        Response::err(error)
    }
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

fn status(state: &mut EngineState, connection: ConnectionId) -> Value {
    let peaks = take_peaks(state, connection);
    let shared = &state.shared;
    let connections = state
        .connections
        .iter()
        .map(|(from, to)| PortConnection {
            from: from.clone(),
            to: to.clone(),
        })
        .collect();
    to_payload(&StatusReply {
        preset: state.active.as_ref().map(|(_, preset)| preset.name.clone()),
        program: active_program(state),
        sample_rate: shared.sample_rate.load(Ordering::Relaxed),
        buffer_size: shared.buffer_size.load(Ordering::Relaxed),
        prepared_frames: state.max_frames,
        revision: state.revision,
        muted: shared.muted.load(Ordering::Relaxed),
        modified: state.modified,
        input_peak_db: peak_db(peaks.input),
        output_peak_db: peak_db(peaks.output),
        xruns: shared.xruns.load(Ordering::Relaxed),
        callback_max_us: shared.callback_ns_max.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        callback_last_us: shared.callback_ns_last.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        connections,
        metronome: metronome_status(state),
        protocol_version: PROTOCOL_VERSION,
        ..status_fields(state, &peaks.tracks, &peaks.mix)
    })
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

/// `peak` in dB as `status` shows it, one decimal; `None` when that is not a finite number.
pub(crate) fn peak_db(peak: f32) -> Option<f64> {
    let db = (20.0 * f64::from(peak).log10()).max(SILENCE_DB);
    let db = (db * 10.0).round() / 10.0;
    db.is_finite().then_some(db)
}
