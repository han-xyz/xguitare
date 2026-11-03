use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde::de::{Deserializer, Error as _};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};
use xguitare_dsp::{ParamSpec, Rack, Unit};

pub use serde_json::Value;

use crate::engine::{Command, EngineQueues, EngineShared};
use crate::preset::shortest_f64;
use crate::{
    Bank, BankEntry, BuildWarning, Config, ModuleRegistry, ParamAddress, Preset, PresetModule,
};
use crate::{ResolveError, resolve};

const ENGINE_BUSY: &str = "engine busy";
const NO_PRESET: &str = "no preset loaded";
const ACTIVE_REMOVED: &str = "active preset file was removed; load a preset first";
const NANOS_PER_MICRO: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    Status,
    List,
    Load { preset: String },
    Set { param: String, value: f64 },
    Get { param: Option<String> },
    Save,
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
}

/// Handles one request; the build warnings of a `load` come back beside the Response for the caller to log.
pub fn handle(state: &mut EngineState, req: Request) -> (Response, Vec<BuildWarning>) {
    let mut warnings = Vec::new();
    let result = match req {
        Request::Status => Ok(status(state)),
        Request::List => Ok(list(state)),
        Request::Load { preset } => load(state, &preset, &mut warnings),
        Request::Set { param, value } => set(state, &param, value),
        Request::Get { param } => get(state, param.as_deref()),
        Request::Save => save(state),
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

/// Whether the Rack prepared for `prepared` frames must be rebuilt for the quantum in effect: larger blocks would be silent, smaller ones would waste work on oversized FFTs.
pub fn needs_reprepare(prepared: usize, buffer_size: u32) -> bool {
    prepared != buffer_size as usize
}

/// Rebuilds the active Preset's Rack, or the empty passthrough Rack when none is active, for `buffer_size` frames and pushes it as a `SwapRack`; the build warnings come back for the caller to log.
pub fn reprepare(state: &mut EngineState, buffer_size: u32) -> Result<Vec<BuildWarning>, String> {
    let max_frames = buffer_size as usize;
    let (rack, warnings) = match &state.active {
        Some((_, preset)) => preset
            .build(
                state.sample_rate,
                max_frames,
                &state.config.bank_dir,
                state.config.ir_max_ms,
            )
            .map_err(|err| err.to_string())?,
        None => {
            let mut rack = Rack::new(Vec::new()).expect("an empty Rack has no layout to violate");
            rack.prepare(state.sample_rate, max_frames);
            (rack, Vec::new())
        }
    };
    swap_rack(state, rack)?;
    state.max_frames = max_frames;
    Ok(warnings)
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
        "xruns": shared.xruns.load(Ordering::Relaxed),
        "callback_max_us": shared.callback_ns_max.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        "callback_last_us": shared.callback_ns_last.load(Ordering::Relaxed) / NANOS_PER_MICRO,
        "connections": connections,
    })
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
    let payload = json!({"program": program, "name": preset.name});
    state.active = Some((file, preset));
    Ok(payload)
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
    Ok(json!({
        "param": format!("{}.{}", address.module, address.param),
        "value": shortest_f64(applied),
        "clamped": clamped,
    }))
}

fn save(state: &EngineState) -> Result<Value, String> {
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
    Ok(json!({"path": file.display().to_string()}))
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
        "param": name,
        "value": shortest_f64(*value),
        "unit": unit_text(spec.unit),
        "min": shortest_f64(spec.min),
        "max": shortest_f64(spec.max),
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
