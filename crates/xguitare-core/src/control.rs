use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde::de::{Deserializer, Error as _};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};
use xguitare_dsp::{Channels, ParamSpec, Rack, Unit};

pub use serde_json::Value;

use crate::engine::{Command, EngineQueues, EngineShared, SWITCHER_MAX_FRAMES, take_peak};
use crate::firstrun::{BASS_IR, DEFAULT_IR};
use crate::preset::{CABINET_TYPE, shortest_f64};
use crate::{
    Bank, BankEntry, BuildWarning, Config, ModuleRegistry, ParamAddress, Preset, PresetModule,
};
use crate::{ResolveError, resolve};

const ENGINE_BUSY: &str = "engine busy";
const NO_PRESET: &str = "no preset loaded";
const ACTIVE_REMOVED: &str = "active preset file was removed; load a preset first";
const NANOS_PER_MICRO: u64 = 1_000;
const SILENCE_DB: f32 = -120.0;
const CABINET_IRS: [&str; 2] = [DEFAULT_IR, BASS_IR];
const CABINET_NEEDS_IR: &str = "cabinet needs an ir; add one in the preset file";
const AFTER_OR_AT: &str = "give either after or at";

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
    pub config: Config,
    pub connections: Vec<(String, String)>,
    pub sample_rate: f32,
    pub max_frames: usize,
    /// Incremented on every change to the live Rack or the Bank; starts at 1.
    pub revision: u64,
}

/// Handles one request; the build warnings of a `load` or a Rack edit come back beside the Response for the caller to log.
pub fn handle(state: &mut EngineState, req: Request) -> (Response, Vec<BuildWarning>) {
    let mut warnings = Vec::new();
    let result = match req {
        Request::Status => Ok(status(state)),
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
    };
    let response = match result {
        Ok(payload) => Response::Ok(payload),
        Err(error) => Response::Err { error },
    };
    (response, warnings)
}

/// Handles one request line and returns the response line, without its newline, and the build warnings as `handle` does.
pub fn handle_line(state: &mut EngineState, line: &str) -> (String, Vec<BuildWarning>) {
    let (response, warnings) = match serde_json::from_str(line) {
        Ok(req) => handle(state, req),
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
    let built = state.active.as_ref().map(|(_, preset)| {
        preset
            .build(
                state.sample_rate,
                max_frames,
                &state.config.bank_dir,
                state.config.ir_max_ms,
            )
            .map_err(|err| (preset.name.clone(), err.to_string()))
    });
    let (rack, outcome) = match built {
        Some(Ok((rack, warnings))) => (rack, Reprepare::Built(warnings)),
        Some(Err((preset, error))) => (
            passthrough_rack(state.sample_rate, max_frames),
            Reprepare::Passthrough { preset, error },
        ),
        None => (
            passthrough_rack(state.sample_rate, max_frames),
            Reprepare::Built(Vec::new()),
        ),
    };
    swap_rack(state, rack)?;
    state.revision += 1;
    if let Reprepare::Passthrough { .. } = outcome {
        state.active = None;
    }
    state.max_frames = max_frames;
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

fn status(state: &EngineState) -> Value {
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
        "input_peak_db": peak_db(take_peak(&shared.input_peak_bits)),
        "output_peak_db": peak_db(take_peak(&shared.output_peak_bits)),
        "xruns": shared.xruns.load(Ordering::Relaxed),
        "callback_max_us": shared.callback_ns_max.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        "callback_last_us": shared.callback_ns_last.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        "connections": connections,
    })
}

fn peak_db(peak: f32) -> f64 {
    let db = (20.0 * f64::from(peak).log10()).max(f64::from(SILENCE_DB));
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
    let (rack, build_warnings) = preset
        .build(
            state.sample_rate,
            state.max_frames,
            &state.config.bank_dir,
            state.config.ir_max_ms,
        )
        .map_err(|err| err.to_string())?;
    *warnings = build_warnings;
    swap_rack(state, rack)?;
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

fn swap_rack(state: &mut EngineState, rack: Rack) -> Result<(), String> {
    state.queues.drain_from_audio();
    state
        .queues
        .commands
        .push(Command::SwapRack(Box::new(rack)))
        .map_err(|_| ENGINE_BUSY.to_string())
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
    let tmp = file.with_extension("toml.tmp");
    fs::write(&tmp, preset.to_toml())
        .and_then(|()| fs::rename(&tmp, file))
        .map_err(|err| {
            let _ = fs::remove_file(&tmp);
            format!("cannot save {}: {err}", file.display())
        })?;
    let payload = json!({"path": file.display().to_string()});
    state.revision += 1;
    Ok(payload)
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
    let bank_dir = state.config.bank_dir.clone();
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
                .find(|ir| bank_dir.join(ir).is_file())
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
    json!({
        "module": module.id,
        "type": module.type_name,
        "param": name,
        "value": shortest_f64(*value),
        "unit": unit_text(spec.unit),
        "min": shortest_f64(spec.min),
        "max": shortest_f64(spec.max),
        "integer": spec.integer,
    })
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
