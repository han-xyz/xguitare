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
pub struct PortChoice {
    pub input: Option<String>,
    pub output_left: Option<String>,
    pub output_right: Option<String>,
}

/// Resolves each configured port: `"auto"` takes the nth port of the physical list (input and left the first, right the second), `""` means do not connect, any other name is used verbatim.
pub fn choose_ports(
    available_capture: &[String],
    available_playback: &[String],
    ports: &Ports,
) -> PortChoice {
    PortChoice {
        input: resolve(available_capture, 0, &ports.input),
        output_left: resolve(available_playback, 0, &ports.output_left),
        output_right: resolve(available_playback, 1, &ports.output_right),
    }
}

fn resolve(physical: &[String], auto_index: usize, configured: &str) -> Option<String> {
    match configured {
        "" => None,
        AUTO_PORT => physical.get(auto_index).cloned(),
        name => Some(name.to_string()),
    }
}
