use std::mem;
use std::sync::atomic::{AtomicU32, AtomicU64};

use rtrb::{Consumer, Producer, RingBuffer};
use xguitare_dsp::Rack;
use xguitare_dsp::crossfade::Crossfader;

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

pub enum Command {
    SwapRack(Box<Rack>),
    SetParam {
        module: usize,
        param: usize,
        value: f32,
    },
}

pub enum FromAudio {
    Retired(Box<Rack>),
    ProgramChange(u8),
}

/// The control side of the queues between the control thread and the audio thread.
pub struct EngineQueues {
    pub commands: Producer<Command>,
    pub from_audio: Consumer<FromAudio>,
    program_changes: Vec<u8>,
}

/// The audio side: the audio thread pops `commands` and hands every Rack it lets go of back through `from_audio`, so no Rack is freed on the audio thread.
pub struct AudioQueues {
    pub commands: Consumer<Command>,
    pub from_audio: Producer<FromAudio>,
}

const PROGRAM_CHANGE_SLOTS: usize = 16;

pub fn engine_queues(capacity: usize) -> (EngineQueues, AudioQueues) {
    let (commands_tx, commands_rx) = RingBuffer::new(capacity);
    // At most every queued SwapRack plus the current and the fading-in Rack, minus the one left current, can be retired between two drains, and `control::handle` drains before each SwapRack push; program changes may only use the slots beyond that, so the audio thread never finds the queue full when it retires a Rack.
    let (from_audio_tx, from_audio_rx) = RingBuffer::new(capacity + 2 + PROGRAM_CHANGE_SLOTS);
    (
        EngineQueues {
            commands: commands_tx,
            from_audio: from_audio_rx,
            program_changes: Vec::new(),
        },
        AudioQueues {
            commands: commands_rx,
            from_audio: from_audio_tx,
        },
    )
}

impl EngineQueues {
    /// Empties `from_audio`: drops every retired Rack and keeps program changes for `take_program_changes`.
    pub fn drain_from_audio(&mut self) {
        while let Ok(message) = self.from_audio.pop() {
            match message {
                FromAudio::Retired(rack) => drop(rack),
                FromAudio::ProgramChange(program) => self.program_changes.push(program),
            }
        }
    }

    pub fn take_program_changes(&mut self) -> Vec<u8> {
        self.drain_from_audio();
        mem::take(&mut self.program_changes)
    }
}

/// The audio thread's Rack state: the current Rack, the one fading in, and the Commands that change them. Allocates and frees nothing after `new`; every Rack it lets go of leaves through `AudioQueues::from_audio`.
pub struct RackSwitcher {
    current: Box<Rack>,
    incoming: Option<Box<Rack>>,
    crossfader: Crossfader,
    crossfade_frames: usize,
    queues: AudioQueues,
    reserved_slots: usize,
}

impl RackSwitcher {
    pub fn new(
        rack: Box<Rack>,
        max_frames: usize,
        crossfade_frames: usize,
        queues: AudioQueues,
    ) -> RackSwitcher {
        let mut crossfader = Crossfader::new();
        crossfader.prepare(max_frames);
        let reserved_slots = queues.from_audio.buffer().capacity() - PROGRAM_CHANGE_SLOTS;
        RackSwitcher {
            current: rack,
            incoming: None,
            crossfader,
            crossfade_frames,
            queues,
            reserved_slots,
        }
    }

    pub fn apply_commands(&mut self) {
        while let Ok(command) = self.queues.commands.pop() {
            match command {
                Command::SetParam {
                    module,
                    param,
                    value,
                } => {
                    let newest = self.incoming.as_mut().unwrap_or(&mut self.current);
                    newest.module_mut(module).set_param(param, value);
                }
                Command::SwapRack(new) => match self.incoming.take() {
                    // A swap during a fade cuts straight to the newest Rack rather than queueing fades, so at most two Racks are ever live.
                    Some(fading_in) => {
                        let old = mem::replace(&mut self.current, new);
                        self.retire(old);
                        self.retire(fading_in);
                    }
                    None => {
                        self.incoming = Some(new);
                        self.crossfader.start(self.crossfade_frames);
                    }
                },
            }
        }
    }

    /// Runs one block of at most `max_frames` through the current Rack, or through both while a fade runs.
    pub fn process(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let Some(incoming) = self.incoming.as_mut() else {
            self.current.process(input, out_l, out_r);
            return;
        };
        let faded = self
            .crossfader
            .process(&mut self.current, incoming, input, out_l, out_r);
        if let Some(new) = self.incoming.take_if(|_| faded) {
            let old = mem::replace(&mut self.current, new);
            self.retire(old);
        }
    }

    /// Hands a MIDI program change to the control thread, or drops it when only the slots reserved for retired Racks are free.
    pub fn push_program_change(&mut self, program: u8) {
        if self.queues.from_audio.slots() > self.reserved_slots {
            let pushed = self
                .queues
                .from_audio
                .push(FromAudio::ProgramChange(program));
            debug_assert!(pushed.is_ok(), "from_audio queue full");
        }
    }

    fn retire(&mut self, rack: Box<Rack>) {
        let pushed = self.queues.from_audio.push(FromAudio::Retired(rack));
        debug_assert!(pushed.is_ok(), "from_audio queue full");
        if let Err(unretired) = pushed {
            // Leaking beats freeing on the audio thread; the queue sizing in engine_queues and the reserve in push_program_change make this unreachable.
            mem::forget(unretired);
        }
    }
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
