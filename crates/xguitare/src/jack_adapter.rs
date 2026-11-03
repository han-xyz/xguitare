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
use xguitare_core::engine::{EngineShared, choose_ports};
use xguitare_dsp::Rack;

const CLIENT_NAME: &str = "xguitare";
const AUDIO_PORT_TYPE: &str = "32 bit float mono audio";
const SYSTEM_PREFIX: &str = "system:";

#[derive(Debug)]
pub enum JackError {
    Connect(jack::Error),
    RegisterPort(&'static str, jack::Error),
    Activate(jack::Error),
    Deactivate(jack::Error),
}

pub struct Session {
    client: AsyncClient<Notifications, Process>,
}

/// Opens the JACK client, requests `buffer_size`, builds the Rack with the effective sample rate and buffer size, and activates it.
pub fn start(
    buffer_size: u32,
    build_rack: impl FnOnce(f32, usize) -> Rack,
    shared: Arc<EngineShared>,
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
    let max_frames = client.buffer_size() as usize;
    let rack = build_rack(client.sample_rate() as f32, max_frames);
    let process = Process {
        rack,
        max_frames,
        input,
        out_l,
        out_r,
        _midi_in: midi_in,
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
    Ok(Session { client })
}

impl Session {
    pub fn connect(&self, ports: &Ports) {
        let client = self.client.as_client();
        let capture = system_ports(client, PortFlags::IS_OUTPUT);
        let playback = system_ports(client, PortFlags::IS_INPUT);
        let choice = choose_ports(&capture, &playback, ports);
        let own = |port: &str| format!("{}:{port}", client.name());
        let links = [
            (&ports.input, choice.input.map(|src| (src, own("in")))),
            (
                &ports.output_left,
                choice.output_left.map(|dst| (own("out_l"), dst)),
            ),
            (
                &ports.output_right,
                choice.output_right.map(|dst| (own("out_r"), dst)),
            ),
        ];
        for (configured, link) in links {
            match link {
                Some((src, dst)) => match client.connect_ports_by_name(&src, &dst) {
                    Ok(()) => info!("connected {src} -> {dst}"),
                    Err(err) => warn!("cannot connect {src} -> {dst}: {err}"),
                },
                None if configured.is_empty() => {}
                None => warn!("port '{configured}' not found, not connecting"),
            }
        }
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

fn system_ports(client: &Client, flags: PortFlags) -> Vec<String> {
    client
        .ports(None, Some(AUDIO_PORT_TYPE), flags)
        .into_iter()
        .filter(|name| name.starts_with(SYSTEM_PREFIX))
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
    rack: Rack,
    max_frames: usize,
    input: Port<AudioIn>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    _midi_in: Port<MidiIn>,
    shared: Arc<EngineShared>,
}

impl ProcessHandler for Process {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        let start = Instant::now();
        let input = self.input.as_slice(ps);
        let out_l = self.out_l.as_mut_slice(ps);
        let out_r = self.out_r.as_mut_slice(ps);
        if input.len() > self.max_frames {
            // The Rack's scratch buffers hold max_frames; PipeWire can raise the quantum after prepare, and resizing here would allocate.
            out_l.fill(0.0);
            out_r.fill(0.0);
            self.shared.oversize_blocks.fetch_add(1, Ordering::Relaxed);
        } else {
            self.rack.process(input, out_l, out_r);
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
