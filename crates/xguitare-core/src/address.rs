use std::fmt;

use crate::preset::Preset;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamAddress {
    pub module: String,
    pub param: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressError(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    UnknownModule(String),
    UnknownParam { module: String, param: String },
}

impl ParamAddress {
    pub fn parse(input: &str) -> Result<ParamAddress, AddressError> {
        match input.split_once('.') {
            Some((module, param))
                if !module.is_empty() && !param.is_empty() && !param.contains('.') =>
            {
                Ok(ParamAddress {
                    module: module.to_string(),
                    param: param.to_string(),
                })
            }
            _ => Err(AddressError(input.to_string())),
        }
    }
}

pub fn resolve(preset: &Preset, addr: &ParamAddress) -> Result<(usize, usize), ResolveError> {
    let module_idx = preset
        .modules
        .iter()
        .position(|module| module.id == addr.module)
        .ok_or_else(|| ResolveError::UnknownModule(addr.module.clone()))?;
    let param_idx = preset.modules[module_idx]
        .params
        .iter()
        .position(|(name, _)| *name == addr.param)
        .ok_or_else(|| ResolveError::UnknownParam {
            module: addr.module.clone(),
            param: addr.param.clone(),
        })?;
    Ok((module_idx, param_idx))
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected <module>.<param>, got '{}'", self.0)
    }
}

impl std::error::Error for AddressError {}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::UnknownModule(id) => write!(f, "no module '{id}'"),
            ResolveError::UnknownParam { module, param } => {
                write!(f, "module '{module}' has no parameter '{param}'")
            }
        }
    }
}

impl std::error::Error for ResolveError {}
