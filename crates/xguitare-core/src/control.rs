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
    Command, EngineQueues, EngineShared, FadeLengths, SWITCHER_MAX_FRAMES, take_peak,
};
use crate::firstrun::{BASS_IR, DEFAULT_IR};
use crate::preset::{CABINET_TYPE, ir_path, is_relative_name, shortest_f64};
use crate::tuner::Tuner;
use crate::{
    Bank, BankEntry, BuildWarning, Config, ModuleRegistry, ParamAddress, Preset, PresetModule,
    list_irs,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    List,
    Load {
        preset: String,
    },
    Set {
        param: String,
        value: f64,
    },
    Get {
        param: Option<String>,
    },
    Save,
    Modules,
    RackAdd {
        r#type: String,
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
    Tuner,
    Mute {
        on: bool,
    },
    IrList,
    RackIr {
        ir: String,
        id: Option<String>,
    },
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
    pub peaks: PeakMeters,
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
    for peaks in state.peaks.meters.values_mut() {
        peaks.input = peaks.input.max(input);
        peaks.output = peaks.output.max(output);
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

/// Handles one request; the build warnings of a `load` or a Rack edit come back beside the Response for the caller to log.
pub fn handle(state: &mut EngineState, req: Request) -> (Response, Vec<BuildWarning>) {
    handle_from(state, IN_PROCESS, req)
}

fn handle_from(
    state: &mut EngineState,
    connection: ConnectionId,
    req: Request,
) -> (Response, Vec<BuildWarning>) {
    let mut warnings = Vec::new();
    let result = match req {
        Request::Status => Ok(status(state, connection)),
        Request::List => Ok(list(state)),
        Request::Load { preset } => load(state, &preset, &mut warnings),
        Request::Set { param, value } => set(state, &param, value),
        Request::Get { param } => get(state, param.as_deref()),
        Request::Save => save(state),
        Request::Modules => Ok(modules()),
        Request::RackAdd { r#type, after, at } => {
            rack_add(state, &r#type, after.as_deref(), at, &mut warnings)
        }
        Request::RackRemove { id } => rack_remove(state, &id, &mut warnings),
        Request::RackMove { id, to } => rack_move(state, &id, to, &mut warnings),
        Request::Tuner => Ok(tuner(state)),
        Request::Mute { on } => Ok(mute(state, on)),
        Request::IrList => ir_list(state),
        Request::RackIr { ir, id } => rack_ir(state, &ir, id.as_deref(), &mut warnings),
    };
    let response = match result {
        Ok(payload) => Response::Ok(payload),
        Err(error) => Response::Err { error },
    };
    (response, warnings)
}

/// Handles one request line from `connection` and returns the response line, without its newline, and the build warnings as `handle` does.
pub fn handle_line(
    state: &mut EngineState,
    connection: ConnectionId,
    line: &str,
) -> (String, Vec<BuildWarning>) {
    let (response, warnings) = match serde_json::from_str(line) {
        Ok(req) => handle_from(state, connection, req),
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
pub fn reprepare(state: &mut EngineState, buffer_size: u32) -> Result<Reprepare, String> {
    let max_frames = buffer_size as usize;
    let outcome = rebuild(state, state.sample_rate, max_frames)?;
    state.max_frames = max_frames;
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
pub fn change_sample_rate(state: &mut EngineState, sample_rate: u32) -> Result<Reprepare, String> {
    let sample_rate = sample_rate as f32;
    let fades = FadeLengths::at(sample_rate, state.config.crossfade_ms);
    if state.queues.commands.slots() < 2
        || state
            .queues
            .commands
            .push(Command::SetFadeLengths(fades))
            .is_err()
    {
        return Err(ENGINE_BUSY.to_string());
    }
    let outcome = rebuild(state, sample_rate, state.max_frames)?;
    state.sample_rate = sample_rate;
    state.tuner.set_sample_rate(sample_rate);
    Ok(outcome)
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
    }
    Ok(outcome)
}

impl Request {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("a Request always serialises to JSON")
    }
}

impl Response {
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
    json!({
        "preset": state.active.as_ref().map(|(_, preset)| &preset.name),
        "program": active_program(state),
        "sample_rate": shared.sample_rate.load(Ordering::Relaxed),
        "buffer_size": shared.buffer_size.load(Ordering::Relaxed),
        "prepared_frames": state.max_frames,
        "revision": state.revision,
        "muted": shared.muted.load(Ordering::Relaxed),
        "input_peak_db": peak_db(peaks.input),
        "output_peak_db": peak_db(peaks.output),
        "xruns": shared.xruns.load(Ordering::Relaxed),
        "callback_max_us": shared.callback_ns_max.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        "callback_last_us": shared.callback_ns_last.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        "connections": connections,
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

fn peak_db(peak: f32) -> f64 {
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
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    let (program, entry) = state
        .bank
        .by_name_or_number(key)
        .ok_or_else(|| format!("no preset '{key}'"))?;
    let (file, preset) = match entry {
        BankEntry::Valid { file, preset } => (file.clone(), preset.clone()),
        BankEntry::Invalid { error, .. } => return Err(error.clone()),
    };
    let payload = json!({"program": program, "name": preset.name});
    install_preset(state, file, preset, warnings)?;
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
pub fn passthrough_rack(sample_rate: f32, max_frames: usize) -> Rack {
    let mut rack = Rack::new(Vec::new()).expect("an empty Rack has no layout to violate");
    rack.prepare(sample_rate, max_frames);
    rack
}

fn set(state: &mut EngineState, address: &str, value: f64) -> Result<Value, String> {
    if !value.is_finite() {
        return Err("value must be a finite number".to_string());
    }
    let address = ParamAddress::parse(address).map_err(|err| err.to_string())?;
    let (_, preset) = state.active.as_mut().ok_or(NO_PRESET)?;
    let (module, param) = resolve(preset, &address).map_err(|err| err.to_string())?;
    let spec = param_spec(&preset.modules[module], param);
    let clamped = value < f64::from(spec.min) || value > f64::from(spec.max);
    let value = if spec.integer { value.round() } else { value };
    let applied = (value as f32).clamp(spec.min, spec.max);
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
    Ok(json!({
        "param": format!("{}.{}", address.module, address.param),
        "value": shortest_f64(applied),
        "clamped": clamped,
    }))
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
    if let Some((_, active)) = state.active.as_mut() {
        *active = saved;
    }
    state.revision += 1;
    Ok(payload)
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
    options
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW);
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

/// `<file name>.<pid>.<n>.tmp` beside `target`, unique within this process.
fn temp_path(target: &Path) -> PathBuf {
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
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    let (ir_dir, bank_dir) = (state.config.ir_dir.clone(), state.config.bank_dir.clone());
    edit_rack(state, warnings, |preset| {
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
                params: spec
                    .params
                    .iter()
                    .map(|p| (p.name.to_string(), p.default))
                    .collect(),
            },
        );
        preset.validate_layout().map_err(|err| err.to_string())?;
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
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    edit_rack(state, warnings, |preset| {
        let modules = &mut preset.modules;
        modules.remove(module_index(modules, id)?);
        Ok(json!({ "id": id }))
    })
}

fn rack_move(
    state: &mut EngineState,
    id: &str,
    to: usize,
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    edit_rack(state, warnings, |preset| {
        let modules = &mut preset.modules;
        let module = modules.remove(module_index(modules, id)?);
        let to = to.min(modules.len());
        modules.insert(to, module);
        Ok(json!({ "id": id, "to": to }))
    })
}

fn edit_rack(
    state: &mut EngineState,
    warnings: &mut Vec<BuildWarning>,
    edit: impl FnOnce(&mut Preset) -> Result<Value, String>,
) -> Result<Value, String> {
    let (file, preset) = state.active.as_ref().ok_or(NO_PRESET)?;
    let (file, mut preset) = (file.clone(), preset.clone());
    let payload = edit(&mut preset)?;
    install_preset(state, file, preset, warnings)?;
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
    warnings: &mut Vec<BuildWarning>,
) -> Result<Value, String> {
    let (file, preset) = state.active.as_ref().ok_or(NO_PRESET)?;
    let index = cabinet_index(&preset.modules, id)?;
    if !is_relative_name(ir) {
        return Err(format!("invalid ir name '{ir}'"));
    }
    let (file, mut preset) = (file.clone(), preset.clone());
    preset.modules[index].ir = Some(ir.to_string());
    let id = preset.modules[index].id.clone();
    let (rack, build_warnings) = build_preset(state, &preset)?;
    if let Some(error) = build_warnings
        .iter()
        .find_map(|BuildWarning::Ir { id: module, error }| (*module == id).then_some(error))
    {
        return Err(error.clone());
    }
    *warnings = build_warnings;
    swap_in(state, file, preset, rack)?;
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

fn get(state: &EngineState, key: Option<&str>) -> Result<Value, String> {
    let (_, preset) = state.active.as_ref().ok_or(NO_PRESET)?;
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
