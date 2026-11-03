use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use xguitare_dsp::modules::cabinet::Cabinet;
use xguitare_dsp::{AudioBuffers, Module, ModuleSpec, Rack, RackError};

use crate::ir::load_ir;
use crate::registry::ModuleRegistry;

const CABINET_TYPE: &str = "cabinet";

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub modules: Vec<PresetModule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresetModule {
    pub id: String,
    pub type_name: String,
    pub ir: Option<String>,
    pub params: Vec<(String, f32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetError {
    Syntax {
        line: usize,
        col: usize,
        msg: String,
    },
    UnknownModule(String),
    UnknownParam {
        module: String,
        param: String,
    },
    DuplicateId(String),
    MissingIr(String),
    NonFiniteParam {
        module: String,
        param: String,
    },
    Layout(String),
}

#[derive(Debug)]
pub enum BuildError {
    UnknownModule(String),
    Layout(RackError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildWarning {
    Ir { id: String, error: String },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetFile {
    name: String,
    #[serde(default)]
    module: Vec<ModuleEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleEntry {
    #[serde(rename = "type")]
    type_name: String,
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ir: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    params: BTreeMap<String, f64>,
}

impl Preset {
    pub fn parse(text: &str) -> Result<Preset, PresetError> {
        let file: PresetFile = toml::from_str(text).map_err(|error| {
            let (line, col) = crate::toml_position(text, &error);
            PresetError::Syntax {
                line,
                col,
                msg: error.message().to_string(),
            }
        })?;
        let registry = ModuleRegistry::default();
        let mut ids = HashSet::new();
        let mut modules = Vec::with_capacity(file.module.len());
        let mut probes: Vec<(String, Box<dyn Module>)> = Vec::new();
        for entry in file.module {
            let spec = registry
                .spec(&entry.type_name)
                .ok_or_else(|| PresetError::UnknownModule(entry.type_name.clone()))?;
            let module = resolve_entry(entry, spec)?;
            if !ids.insert(module.id.clone()) {
                return Err(PresetError::DuplicateId(module.id));
            }
            probes.push((module.id.clone(), Box::new(LayoutProbe(spec))));
            modules.push(module);
        }
        Rack::new(probes).map_err(|error| PresetError::Layout(error.to_string()))?;
        Ok(Preset {
            name: file.name,
            modules,
        })
    }

    pub fn to_toml(&self) -> String {
        let file = PresetFile {
            name: self.name.clone(),
            module: self
                .modules
                .iter()
                .map(|module| ModuleEntry {
                    type_name: module.type_name.clone(),
                    id: Some(module.id.clone()),
                    ir: module.ir.clone(),
                    params: module
                        .params
                        .iter()
                        .map(|(name, value)| (name.clone(), shortest_f64(*value)))
                        .collect(),
                })
                .collect(),
        };
        toml::to_string_pretty(&file).expect("a PresetFile always serialises to TOML")
    }

    pub fn build(
        &self,
        sample_rate: f32,
        max_frames: usize,
        bank_dir: &Path,
        ir_max_ms: f32,
    ) -> Result<(Rack, Vec<BuildWarning>), BuildError> {
        let registry = ModuleRegistry::default();
        let mut modules = Vec::with_capacity(self.modules.len());
        let mut warnings = Vec::new();
        for preset_module in &self.modules {
            let mut module: Box<dyn Module> = if preset_module.type_name == CABINET_TYPE {
                let ir = match preset_module
                    .ir
                    .as_deref()
                    .map(|file| load_ir(&bank_dir.join(file), sample_rate as u32, ir_max_ms))
                {
                    Some(Ok(ir)) => Some(ir),
                    Some(Err(error)) => {
                        warnings.push(BuildWarning::Ir {
                            id: preset_module.id.clone(),
                            error: error.to_string(),
                        });
                        None
                    }
                    None => None,
                };
                Box::new(Cabinet::new(ir))
            } else {
                registry
                    .construct(&preset_module.type_name)
                    .ok_or_else(|| BuildError::UnknownModule(preset_module.type_name.clone()))?
            };
            for (idx, (_, value)) in preset_module.params.iter().enumerate() {
                module.set_param(idx, *value);
            }
            modules.push((preset_module.id.clone(), module));
        }
        let mut rack = Rack::new(modules).map_err(BuildError::Layout)?;
        rack.prepare(sample_rate, max_frames);
        for idx in 0..rack.modules().len() {
            rack.module_mut(idx).reset();
        }
        Ok((rack, warnings))
    }
}

fn resolve_entry(entry: ModuleEntry, spec: &ModuleSpec) -> Result<PresetModule, PresetError> {
    let id = entry.id.unwrap_or_else(|| entry.type_name.clone());
    if let Some(param) = entry
        .params
        .keys()
        .find(|name| !spec.params.iter().any(|p| p.name == name.as_str()))
    {
        return Err(PresetError::UnknownParam {
            module: id,
            param: param.clone(),
        });
    }
    if let Some(param) = entry
        .params
        .iter()
        .find(|(_, v)| !v.is_finite())
        .map(|(name, _)| name)
    {
        return Err(PresetError::NonFiniteParam {
            module: id,
            param: param.clone(),
        });
    }
    if entry.type_name == CABINET_TYPE && entry.ir.is_none() {
        return Err(PresetError::MissingIr(id));
    }
    let params = spec
        .params
        .iter()
        .map(|p| {
            let value = entry
                .params
                .get(p.name)
                .map_or(p.default, |v| (*v as f32).clamp(p.min, p.max));
            (p.name.to_string(), value)
        })
        .collect();
    Ok(PresetModule {
        id,
        type_name: entry.type_name,
        ir: entry.ir,
        params,
    })
}

fn shortest_f64(value: f32) -> f64 {
    value
        .to_string()
        .parse()
        .expect("an f32's Display text is a valid f64")
}

struct LayoutProbe(&'static ModuleSpec);

impl Module for LayoutProbe {
    fn spec(&self) -> &'static ModuleSpec {
        self.0
    }
    fn prepare(&mut self, _sample_rate: f32, _max_frames: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _idx: usize, _value: f32) {}
    fn param(&self, _idx: usize) -> f32 {
        0.0
    }
    fn process(&mut self, _io: &mut AudioBuffers) {}
}

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PresetError::Syntax { line, col, msg } => write!(f, "line {line}:{col}: {msg}"),
            PresetError::UnknownModule(type_name) => {
                write!(f, "unknown module type '{type_name}'")
            }
            PresetError::UnknownParam { module, param } => {
                write!(f, "module '{module}' has no parameter '{param}'")
            }
            PresetError::DuplicateId(id) => write!(f, "duplicate module id '{id}'"),
            PresetError::MissingIr(id) => write!(f, "cabinet '{id}' has no ir"),
            PresetError::NonFiniteParam { module, param } => {
                write!(
                    f,
                    "module '{module}' parameter '{param}' is not a finite number"
                )
            }
            PresetError::Layout(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for PresetError {}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::UnknownModule(type_name) => {
                write!(f, "unknown module type '{type_name}'")
            }
            BuildError::Layout(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BuildError {}

impl fmt::Display for BuildWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildWarning::Ir { id, error } => write!(f, "cabinet '{id}': {error}"),
        }
    }
}
