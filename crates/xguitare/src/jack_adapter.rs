use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use jack::{
    AsyncClient, AudioIn, AudioOut, Client, ClientOptions, Control, Frames, MidiIn,
    NotificationHandler, Port, PortFlags, PortSpec, ProcessHandler, ProcessScope,
};
use tracing::{info, warn};
use xguitare_core::Ports;
use xguitare_core::engine::{AudioQueues, EngineShared, RackSwitcher, SWITCHER_MAX_FRAMES};
use xguitare_core::ports::{AvailablePort, choose_ports};
use xguitare_dsp::{Rack, midi};

const CLIENT_NAME: &str = "xguitare";
const AUDIO_PORT_TYPE: &str = "32 bit float mono audio";

#[derive(Debug)]
pub enum JackError {
    Connect(jack::Error),
    RegisterPort(&'static str, jack::Error),
    Activate(jack::Error),
    Deactivate(jack::Error),
}

pub struct Session {
    client: AsyncClient<Notifications, Process>,
    max_frames: usize,
}

/// Opens the JACK client, requests `buffer_size`, builds the Rack with the effective sample rate and buffer size, and activates it; the audio thread then takes Commands from `queues`.
pub fn start(
    buffer_size: u32,
    crossfade_ms: f32,
    build_rack: impl FnOnce(f32, usize) -> Rack,
    shared: Arc<EngineShared>,
    queues: AudioQueues,
) -> Result<Session, JackError> {
    let (client, _status) =
        Client::new(CLIENT_NAME, ClientOptions::NO_START_SERVER).map_err(JackError::Connect)?;
    let input = register(&client, "in", AudioIn::default())?;
    let out_l = register(&client, "out_l", AudioOut::default())?;
    let out_r = register(&client, "out_r", AudioOut::default())?;
    let midi_in = register(&client, "midi_in", MidiIn::default())?;
    if let Err(err) = client.set_buffer_size(buffer_size) {
        warn!("cannot set buffer size {buffer_size}: {err}");
    }
    let max_frames = (buffer_size as usize).max(client.buffer_size() as usize);
    let sample_rate = client.sample_rate() as f32;
    let rack = Box::new(build_rack(sample_rate, max_frames));
    let crossfade_frames = (crossfade_ms * sample_rate / 1000.0) as usize;
    let process = Process {
        switcher: RackSwitcher::new(rack, SWITCHER_MAX_FRAMES, crossfade_frames, queues),
        input,
        out_l,
        out_r,
        midi_in,
        shared: Arc::clone(&shared),
    };
    let notifications = Notifications {
        shared: Arc::clone(&shared),
    };
    let client = client
        .activate_async(notifications, process)
        .map_err(JackError::Activate)?;
    let sample_rate = client.as_client().sample_rate();
    let effective = client.as_client().buffer_size();
    shared.sample_rate.store(sample_rate, Ordering::Relaxed);
    shared.buffer_size.store(effective, Ordering::Relaxed);
    info!("jack client active: sample_rate={sample_rate} buffer_size={effective}");
    if effective != buffer_size {
        warn!(
            "buffer size is {effective}, wanted {buffer_size}; force it with: pw-metadata -n settings 0 clock.force-quantum {buffer_size}"
        );
    }
    if effective as usize != max_frames {
        info!("buffer size {effective} differs from prepared {max_frames}; re-preparing");
    }
    Ok(Session { client, max_frames })
}

impl Session {
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Connects the Daemon's ports per `ports` and returns the `(from, to)` pairs that succeeded.
    pub fn connect(&self, ports: &Ports) -> Vec<(String, String)> {
        let client = self.client.as_client();
        let capture = foreign_audio_ports(client, PortFlags::IS_OUTPUT);
        let playback = foreign_audio_ports(client, PortFlags::IS_INPUT);
        let choice = choose_ports(&capture, &playback, ports);
        for warning in &choice.warnings {
            warn!("{warning}");
        }
        let own = |port: &str| format!("{}:{port}", client.name());
        let links = [
            choice.input.map(|src| (src, own("in"))),
            choice.output_left.map(|dst| (own("out_l"), dst)),
            choice.output_right.map(|dst| (own("out_r"), dst)),
        ];
        let mut connected = Vec::new();
        for (src, dst) in links.into_iter().flatten() {
            match client.connect_ports_by_name(&src, &dst) {
                Ok(()) => {
                    info!("connected {src} -> {dst}");
                    connected.push((src, dst));
                }
                Err(err) => warn!("cannot connect {src} -> {dst}: {err}"),
            }
        }
        connected
    }

    pub fn stop(self) -> Result<(), JackError> {
        self.client
            .deactivate()
            .map(|_| ())
            .map_err(JackError::Deactivate)
    }
}

fn register<PS: PortSpec>(
    client: &Client,
    name: &'static str,
    spec: PS,
) -> Result<Port<PS>, JackError> {
    client
        .register_port(name, spec)
        .map_err(|err| JackError::RegisterPort(name, err))
}

fn foreign_audio_ports(client: &Client, direction: PortFlags) -> Vec<AvailablePort> {
    let own_prefix = format!("{}:", client.name());
    let physical = client.ports(
        None,
        Some(AUDIO_PORT_TYPE),
        direction | PortFlags::IS_PHYSICAL,
    );
    client
        .ports(None, Some(AUDIO_PORT_TYPE), direction)
        .into_iter()
        .filter(|name| !name.starts_with(&own_prefix))
        .map(|name| AvailablePort {
            is_physical: physical.contains(&name),
            name,
        })
        .collect()
}

struct Notifications {
    shared: Arc<EngineShared>,
}

impl NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.shared.xruns.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }
}

struct Process {
    switcher: RackSwitcher,
    input: Port<AudioIn>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    midi_in: Port<MidiIn>,
    shared: Arc<EngineShared>,
}

impl ProcessHandler for Process {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        let start = Instant::now();
        flush_denormals();
        for event in self.midi_in.iter(ps) {
            if let Some(program) = midi::program_change(event.bytes) {
                self.switcher.push_program_change(program);
            }
        }
        self.switcher.apply_commands();
        let input = self.input.as_slice(ps);
        let out_l = self.out_l.as_mut_slice(ps);
        let out_r = self.out_r.as_mut_slice(ps);
        // PipeWire can raise the quantum past the size the Rack was prepared for, and resizing here would allocate: the switcher writes silence until the control thread swaps in a Rack re-prepared for the quantum.
        if !self.switcher.process(input, out_l, out_r) {
            self.shared.oversize_blocks.fetch_add(1, Ordering::Relaxed);
        }
        let elapsed = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.shared
            .callback_ns_last
            .store(elapsed, Ordering::Relaxed);
        self.shared
            .callback_ns_max
            .fetch_max(elapsed, Ordering::Relaxed);
        Control::Continue
    }

    fn buffer_size(&mut self, _: &Client, size: Frames) -> Control {
        self.shared.buffer_size.store(size, Ordering::Relaxed);
        Control::Continue
    }
}

fn flush_denormals() {
    // FTZ (0x8000) and DAZ (0x0040): recursive filter tails decay into subnormals, which cost 10-100x per operation.
    #[cfg(target_arch = "x86_64")]
    #[expect(
        deprecated,
        reason = "_mm_getcsr/_mm_setcsr are deprecated in favour of inline asm that does the same"
    )]
    // SAFETY: changes only this thread's MXCSR.
    unsafe {
        use core::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        let csr = _mm_getcsr();
        if csr & 0x8040 != 0x8040 {
            _mm_setcsr(csr | 0x8040);
        }
    }
}

impl fmt::Display for JackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JackError::Connect(err) => write!(f, "cannot connect to JACK/PipeWire: {err}"),
            JackError::RegisterPort(name, err) => write!(f, "cannot register port '{name}': {err}"),
            JackError::Activate(err) => write!(f, "cannot activate JACK client: {err}"),
            JackError::Deactivate(err) => write!(f, "cannot deactivate JACK client: {err}"),
        }
    }
}

impl std::error::Error for JackError {}
