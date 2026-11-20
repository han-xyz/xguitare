use std::io;
use std::path::PathBuf;

use serde_json::{Value, json};
use xguitare_dsp::Rack;

use crate::build::{build_rack, swap_rack};
use crate::control::EngineState;
use crate::firstrun::ensure_bank;
use crate::ports::{AudioPorts, AvailablePort, connect_plan};
use crate::project_control::create_projects_dir;
use crate::rack_control::passthrough_rack;
use crate::state::{SavedState, read_state_bytes, restore_state, write_state};
use crate::{Bank, BankEntry, BankKind, BuildWarning, Config, Ports, Preset};

const BANK_DIR_IN_CONFIG: &str = "bank_dir is set in config.toml; edit it there";
const PROJECTS_DIR_NOT_ABSOLUTE: &str = "projects_dir must be an absolute path";

pub(crate) fn port_names(ports: &dyn AudioPorts) -> (Vec<String>, Vec<String>) {
    let names = |available: Vec<AvailablePort>| {
        let mut names: Vec<String> = available.into_iter().map(|port| port.name).collect();
        names.sort();
        names
    };
    let (capture, playback) = ports.audio_ports();
    (names(capture), names(playback))
}

pub(crate) fn connected(state: &EngineState, ports: &dyn AudioPorts) -> Value {
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
pub(crate) fn set_ports(
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

pub(crate) fn settings(state: &EngineState) -> Value {
    json!({
        "bank_kind": state.config.bank_kind.to_string(),
        "bank_dir": state.config.bank_dir.display().to_string(),
        "ir_dir": state.config.ir_dir.display().to_string(),
        "projects_dir": state.config.projects_dir.display().to_string(),
    })
}

/// Stores `bank_kind` and `ir_dir` in `state.toml` and applies them: a changed `bank_kind` records the Bank directory in use under the current kind in `[banks]`, switches to the new kind's directory (seeded when missing) and plays Program 0 (the passthrough Rack when it does not build). On an error the state and `state.toml` are unchanged; only a seeded Bank directory may stay.
pub(crate) fn set_settings(
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
