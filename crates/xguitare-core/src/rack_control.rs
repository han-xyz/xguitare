use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xguitare_dsp::{Channels, ParamSpec, Rack, Unit};
use xguitare_protocol::{GetReply, ParamStatus, to_payload};

use crate::atomic_file::{slug, write_new, write_through};
use crate::build::{ENGINE_BUSY, build_rack, swap_rack};
use crate::control::EngineState;
use crate::engine::Command;
use crate::firstrun::{BASS_IR, DEFAULT_IR};
use crate::preset::{CABINET_TYPE, ir_path, is_relative_name, shortest_f64};
use crate::project::ProjectError;
use crate::project_control::{
    ProjectRack, bypass_command, change_rack_module, param_command, rack_preset, set_rack_modules,
};
use crate::{
    Bank, BankEntry, BuildWarning, ModuleRegistry, ParamAddress, Preset, PresetModule, list_irs,
};
use crate::{ResolveError, resolve};

const NO_PRESET: &str = "no preset loaded";
const ACTIVE_REMOVED: &str = "active preset file was removed; load a preset first";
const CABINET_IRS: [&str; 2] = [DEFAULT_IR, BASS_IR];
const CABINET_NEEDS_IR: &str = "cabinet needs an ir; add one in the preset file";
const AFTER_OR_AT: &str = "give either after or at";
const NO_CABINET: &str = "no cabinet module";
const EMPTY_PRESET_NAME: &str = "preset name must not be empty";
const SAVE_AS_TRIES: u64 = 8;
const PRESET_INTO_TRACK: &str = "a Preset loads only into a Track";

pub(crate) fn list(state: &EngineState) -> Value {
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

pub(crate) fn load(
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

pub(crate) fn set(
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

pub(crate) fn bypass(
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

pub(crate) fn save(state: &mut EngineState) -> Result<Value, String> {
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
pub(crate) fn save_as(state: &mut EngineState, name: &str) -> Result<Value, String> {
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

pub(crate) fn modules() -> Value {
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

pub(crate) fn rack_add(
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

pub(crate) fn rack_remove(
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

pub(crate) fn rack_move(
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

pub(crate) fn ir_list(state: &EngineState) -> Result<Value, String> {
    let irs = list_irs(&state.config.ir_dir)?;
    Ok(json!({"ir_dir": state.config.ir_dir.display().to_string(), "irs": irs}))
}

/// Sets the cabinet's `ir` and swaps the rebuilt Rack in; an IR that fails to load is an error and changes nothing.
pub(crate) fn rack_ir(
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

/// The Project Rack's Modules as a Preset, or the active Preset for the live Rack.
fn rack_or_live_preset(state: &EngineState, rack: Option<ProjectRack>) -> Result<Preset, String> {
    match rack {
        Some(rack) => Ok(rack_preset(state, rack)),
        None => Ok(state.active.as_ref().ok_or(NO_PRESET)?.1.clone()),
    }
}

pub(crate) fn get(
    state: &EngineState,
    key: Option<&str>,
    rack: Option<ProjectRack>,
) -> Result<Value, String> {
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
    let params = selected
        .into_iter()
        .map(|(module, param)| param_status(&preset.modules[module], param))
        .collect();
    Ok(to_payload(&GetReply { params }))
}

fn param_status(module: &PresetModule, param: usize) -> ParamStatus {
    let spec = param_spec(module, param);
    let (name, value) = &module.params[param];
    ParamStatus {
        module: module.id.clone(),
        type_name: module.type_name.clone(),
        param: name.clone(),
        value: shortest_f64(*value),
        unit: unit_text(spec.unit).to_string(),
        min: shortest_f64(spec.min),
        max: shortest_f64(spec.max),
        default: shortest_f64(spec.default),
        integer: spec.integer,
        labels: spec.labels.iter().map(ToString::to_string).collect(),
        bypass: Some(module.bypass),
        ir: (module.type_name == CABINET_TYPE).then(|| module.ir.clone()),
    }
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

pub(crate) fn active_program(state: &EngineState) -> Option<usize> {
    state
        .active
        .as_ref()
        .and_then(|(file, _)| state.bank.position(file))
}

fn file_stem(file: &Path) -> String {
    file.file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
}
