use std::sync::atomic::{AtomicU32, AtomicU64};

use crate::Ports;

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

pub fn choose_ports(
    available_capture: &[String],
    available_playback: &[String],
    ports: &Ports,
) -> PortChoice {
    PortChoice {
        input: find(available_capture, &ports.input),
        output_left: find(available_playback, &ports.output_left),
        output_right: find(available_playback, &ports.output_right),
    }
}

fn find(available: &[String], configured: &str) -> Option<String> {
    available
        .iter()
        .find(|name| !configured.is_empty() && name.as_str() == configured)
        .cloned()
}
