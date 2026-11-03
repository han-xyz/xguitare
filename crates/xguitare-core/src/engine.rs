use std::sync::atomic::{AtomicU32, AtomicU64};

use crate::{AUTO_PORT, Ports};

#[derive(Debug, Default)]
pub struct EngineShared {
    pub xruns: AtomicU64,
    pub oversize_blocks: AtomicU64,
    pub callback_ns_max: AtomicU64,
    pub callback_ns_last: AtomicU64,
    pub sample_rate: AtomicU32,
    pub buffer_size: AtomicU32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailablePort {
    pub name: String,
    pub is_physical: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortChoice {
    pub input: Option<String>,
    pub output_left: Option<String>,
    pub output_right: Option<String>,
    pub warnings: Vec<String>,
}

const WILDCARD: char = '*';

/// Resolves each configured port against the available capture and playback ports (see `Ports` for the three forms: "auto" looks at physical ports only, a pattern at all of them); every port left unconnected for a reason the user can fix adds a line to `warnings`.
pub fn choose_ports(
    available_capture: &[AvailablePort],
    available_playback: &[AvailablePort],
    ports: &Ports,
) -> PortChoice {
    let capture = sorted(port_names(available_capture, false));
    let playback = sorted(port_names(available_playback, false));
    let physical_capture = port_names(available_capture, true);
    let physical_playback = port_names(available_playback, true);
    let mut warnings = Vec::new();

    let input = match ports.input.as_str() {
        AUTO_PORT => auto_device(&physical_capture, "capture", "ports.input", &mut warnings)
            .and_then(|device| device.first().cloned()),
        pattern => first_match(&capture, pattern, &mut warnings),
    };

    let auto_outputs: Vec<&str> = [
        ("ports.output_left", &ports.output_left),
        ("ports.output_right", &ports.output_right),
    ]
    .into_iter()
    .filter(|(_, configured)| configured.as_str() == AUTO_PORT)
    .map(|(key, _)| key)
    .collect();
    let playback_device = if auto_outputs.is_empty() {
        None
    } else {
        auto_device(
            &physical_playback,
            "playback",
            &auto_outputs.join(", "),
            &mut warnings,
        )
    };

    let output_left = match ports.output_left.as_str() {
        AUTO_PORT => playback_device
            .as_ref()
            .and_then(|device| device.first().cloned()),
        pattern => first_match(&playback, pattern, &mut warnings),
    };
    let output_right = match ports.output_right.as_str() {
        AUTO_PORT => playback_device.as_ref().and_then(|device| {
            let second = device.get(1).cloned();
            if second.is_none() {
                warnings.push(format!(
                    "playback device has only {}, not connecting ports.output_right",
                    device[0]
                ));
            }
            second
        }),
        pattern => first_match(&playback, pattern, &mut warnings),
    };

    PortChoice {
        input,
        output_left,
        output_right,
        warnings,
    }
}

fn port_names(available: &[AvailablePort], physical_only: bool) -> Vec<String> {
    available
        .iter()
        .filter(|port| port.is_physical || !physical_only)
        .map(|port| port.name.clone())
        .collect()
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

fn device_of(port: &str) -> &str {
    port.rsplit_once(':').map_or(port, |(device, _)| device)
}

/// Returns the ports of the only device in `ports`, in the order given (JACK's order, which is channel order).
fn auto_device(
    ports: &[String],
    kind: &str,
    keys: &str,
    warnings: &mut Vec<String>,
) -> Option<Vec<String>> {
    let Some(first) = ports.first() else {
        warnings.push(format!(
            "no physical {kind} port found for {keys}, not connecting"
        ));
        return None;
    };
    if ports.iter().any(|port| device_of(port) != device_of(first)) {
        warnings.push(format!(
            "several {kind} devices; pick one in config {keys}: {}",
            sorted(ports.to_vec()).join(", ")
        ));
        return None;
    }
    Some(ports.to_vec())
}

fn first_match(
    sorted_ports: &[String],
    pattern: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }
    let found = sorted_ports
        .iter()
        .find(|name| matches_pattern(pattern, name))
        .cloned();
    if found.is_none() {
        warnings.push(format!("port '{pattern}' not found, not connecting"));
    }
    found
}

fn matches_pattern(pattern: &str, name: &str) -> bool {
    if !pattern.contains(WILDCARD) {
        return pattern == name;
    }
    let mut pieces = pattern.split(WILDCARD);
    let first = pieces.next().unwrap_or_default();
    let mut middle: Vec<&str> = pieces.collect();
    let last = middle.pop().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    for piece in middle {
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}
