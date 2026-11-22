use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use jack::{
    AsyncClient, AudioIn, AudioOut, Client, ClientOptions, Control, Frames, LatencyType, MidiIn,
    NotificationHandler, Port, PortFlags, PortSpec, ProcessHandler, ProcessScope,
};
use tracing::{info, warn};
use xguitare_core::Ports;
use xguitare_core::control::prepared_max_frames;
use xguitare_core::engine::{
    AudioQueues, EngineShared, FadeLengths, RackSwitcher, SWITCHER_MAX_FRAMES, block_peak,
    raise_peak,
};
use xguitare_core::ports::{AudioPorts, AvailablePort, choose_ports, foreign_ports};
use xguitare_core::project::MAX_TRACKS;
use xguitare_core::take_capture::Latency;
use xguitare_dsp::{Rack, midi};

const CLIENT_NAME: &str = "xguitare";
const AUDIO_PORT_TYPE: &str = "32 bit float mono audio";
/// Track n's input; `in` stays the input of the Rack and the Tuner.
const TRACK_PORTS: [&str; MAX_TRACKS] = [
    "in_1", "in_2", "in_3", "in_4", "in_5", "in_6", "in_7", "in_8", "in_9", "in_10", "in_11",
    "in_12", "in_13", "in_14", "in_15", "in_16",
];

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
    built_sample_rate: u32,
}

/// Opens the JACK client, requests `buffer_size`, builds the Rack with the effective sample rate and buffer size, and activates it; the audio thread then takes Commands from `queues`.
pub fn start(
    buffer_size: u32,
    crossfade_ms: f32,
    build_rack: impl FnOnce(f32, usize) -> Rack,
    shared: &Arc<EngineShared>,
    queues: AudioQueues,
) -> Result<Session, JackError> {
    let (client, _status) =
        Client::new(CLIENT_NAME, ClientOptions::NO_START_SERVER).map_err(JackError::Connect)?;
    let input = register(&client, "in", AudioIn::default())?;
    let track_inputs = TRACK_PORTS
        .iter()
        .map(|name| register(&client, name, AudioIn::default()))
        .collect::<Result<Vec<_>, _>>()?;
    let out_l = register(&client, "out_l", AudioOut::default())?;
    let out_r = register(&client, "out_r", AudioOut::default())?;
    let midi_in = register(&client, "midi_in", MidiIn::default())?;
    if let Err(err) = client.set_buffer_size(buffer_size) {
        warn!("cannot set buffer size {buffer_size}: {err}");
    }
    let max_frames = prepared_max_frames(buffer_size, client.buffer_size());
    let built_sample_rate = client.sample_rate();
    let sample_rate = built_sample_rate as f32;
    let rack = Box::new(build_rack(sample_rate, max_frames));
    let process = Process {
        switcher: RackSwitcher::new(
            rack,
            SWITCHER_MAX_FRAMES,
            FadeLengths::at(sample_rate, crossfade_ms),
            queues,
            Arc::clone(shared),
        ),
        input,
        track_inputs,
        out_l,
        out_r,
        midi_in,
        shared: Arc::clone(shared),
    };
    shared
        .sample_rate
        .store(built_sample_rate, Ordering::Relaxed);
    let notifications = Notifications {
        shared: Arc::clone(shared),
    };
    let client = client
        .activate_async(notifications, process)
        .map_err(JackError::Activate)?;
    let sample_rate = client.as_client().sample_rate();
    let effective = client.as_client().buffer_size();
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
    Ok(Session {
        client,
        max_frames,
        built_sample_rate,
    })
}

impl Session {
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// The rate the first Rack was built at; the JACK rate read later can differ, since pipewire-jack may report a passing rate while the ports connect.
    pub fn built_sample_rate(&self) -> u32 {
        self.built_sample_rate
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

    /// Connects Track n's `in_<n>` to the capture port in `inputs[n - 1]` after dropping its old connections; a Track past the end of `inputs`, or with no input, stays unconnected. Returns the `(from, to)` pairs that succeeded.
    pub fn connect_track_inputs(&self, inputs: &[Option<String>]) -> Vec<(String, String)> {
        let client = self.client.as_client();
        let mut connected = Vec::new();
        for (idx, port) in TRACK_PORTS.iter().enumerate() {
            let own = format!("{}:{port}", client.name());
            if let Some(own_port) = client.port_by_name(&own)
                && let Err(err) = client.disconnect(&own_port)
            {
                warn!("cannot disconnect {own}: {err}");
            }
            let Some(Some(src)) = inputs.get(idx) else {
                continue;
            };
            match client.connect_ports_by_name(src, &own) {
                Ok(()) => {
                    info!("connected {src} -> {own}");
                    connected.push((src.clone(), own));
                }
                Err(err) => warn!("cannot connect {src} -> {own}: {err}"),
            }
        }
        connected
    }

    /// The largest capture latency among the ports feeding Track `track`'s input and the largest playback latency among the ports `out_l` feeds, 0 where nothing is connected.
    pub fn track_latency(&self, track: usize) -> Latency {
        let client = self.client.as_client();
        let own = |port: &str| format!("{}:{port}", client.name());
        Latency {
            capture: TRACK_PORTS.get(track).map_or(0, |port| {
                connected_latency(client, &own(port), LatencyType::Capture)
            }),
            playback: connected_latency(client, &own("out_l"), LatencyType::Playback),
        }
    }

    pub fn stop(self) -> Result<(), JackError> {
        self.client
            .deactivate()
            .map(|_| ())
            .map_err(JackError::Deactivate)
    }
}

impl AudioPorts for Session {
    fn audio_ports(&self) -> (Vec<AvailablePort>, Vec<AvailablePort>) {
        let client = self.client.as_client();
        (
            foreign_audio_ports(client, PortFlags::IS_OUTPUT),
            foreign_audio_ports(client, PortFlags::IS_INPUT),
        )
    }

    fn own_ports(&self) -> Ports {
        let own = |port: &str| format!("{}:{port}", self.client.as_client().name());
        Ports {
            input: own("in"),
            output_left: own("out_l"),
            output_right: own("out_r"),
        }
    }

    fn connect(&self, from: &str, to: &str) -> Result<(), String> {
        match self.client.as_client().connect_ports_by_name(from, to) {
            Ok(()) | Err(jack::Error::PortAlreadyConnected(..)) => {
                info!("connected {from} -> {to}");
                Ok(())
            }
            Err(err) => {
                warn!("cannot connect {from} -> {to}: {err}");
                Err(err.to_string())
            }
        }
    }

    fn disconnect(&self, from: &str, to: &str) -> Result<(), String> {
        match self.client.as_client().disconnect_ports_by_name(from, to) {
            Ok(()) => {
                info!("disconnected {from} -> {to}");
                Ok(())
            }
            Err(err) => {
                warn!("cannot disconnect {from} -> {to}: {err}");
                Err(err.to_string())
            }
        }
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

fn connected_latency(client: &Client, own_port: &str, mode: LatencyType) -> u32 {
    let connections = client
        .port_by_name(own_port)
        .and_then(|port| port.get_connections().ok())
        .unwrap_or_default();
    connections
        .iter()
        .filter_map(|name| client.port_by_name(name))
        .filter_map(|port| port.get_latency_range(mode).ok())
        .map(|(_, max)| max)
        .max()
        .unwrap_or(0)
}

fn foreign_audio_ports(client: &Client, direction: PortFlags) -> Vec<AvailablePort> {
    let physical = client.ports(
        None,
        Some(AUDIO_PORT_TYPE),
        direction | PortFlags::IS_PHYSICAL,
    );
    let all = client.ports(None, Some(AUDIO_PORT_TYPE), direction);
    foreign_ports(&[CLIENT_NAME, client.name()], all, &physical)
}

struct Notifications {
    shared: Arc<EngineShared>,
}

impl NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.shared.xruns.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }

    fn sample_rate(&mut self, _: &Client, srate: Frames) -> Control {
        self.shared.sample_rate.store(srate, Ordering::Relaxed);
        Control::Continue
    }
}

struct Process {
    switcher: RackSwitcher,
    input: Port<AudioIn>,
    track_inputs: Vec<Port<AudioIn>>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    midi_in: Port<MidiIn>,
    shared: Arc<EngineShared>,
}

impl ProcessHandler for Process {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        no_alloc(|| {
            let start = Instant::now();
            flush_denormals();
            for event in self.midi_in.iter(ps) {
                if let Some(program) = midi::program_change(event.bytes) {
                    self.switcher.push_program_change(program);
                }
            }
            self.switcher.apply_commands();
            let input = self.input.as_slice(ps);
            let track_inputs: [&[f32]; MAX_TRACKS] =
                std::array::from_fn(|idx| self.track_inputs[idx].as_slice(ps));
            let out_l = self.out_l.as_mut_slice(ps);
            let out_r = self.out_r.as_mut_slice(ps);
            // PipeWire can raise the quantum past the size the Rack was prepared for, and resizing here would allocate: the switcher writes silence until the control thread swaps in a Rack re-prepared for the quantum.
            if !self
                .switcher
                .process_tracks(input, &track_inputs, out_l, out_r)
            {
                self.shared.oversize_blocks.fetch_add(1, Ordering::Relaxed);
            }
            raise_peak(&self.shared.input_peak_bits, block_peak(input));
            raise_peak(
                &self.shared.output_peak_bits,
                block_peak(out_l).max(block_peak(out_r)),
            );
            let elapsed = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.shared
                .callback_ns_last
                .store(elapsed, Ordering::Relaxed);
            self.shared
                .callback_ns_max
                .fetch_max(elapsed, Ordering::Relaxed);
            Control::Continue
        })
    }

    fn buffer_size(&mut self, _: &Client, size: Frames) -> Control {
        self.shared.buffer_size.store(size, Ordering::Relaxed);
        Control::Continue
    }
}

#[cfg(feature = "rt-check")]
fn no_alloc<T>(block: impl FnOnce() -> T) -> T {
    assert_no_alloc::assert_no_alloc(block)
}

#[cfg(not(feature = "rt-check"))]
fn no_alloc<T>(block: impl FnOnce() -> T) -> T {
    block()
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
