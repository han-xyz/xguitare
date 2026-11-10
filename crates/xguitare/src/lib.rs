mod cli;
mod daemon;
pub mod disk_writer;
mod jack_adapter;
mod logging;
pub mod socket_server;

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Args, Parser, Subcommand, ValueEnum};
use signal_hook::consts::{SIGINT, SIGTERM};
use xguitare_client::{Reconnecting, Transport, UnixSocketTransport};
use xguitare_core::Config;

pub use cli::{CliEnv, run_cli};

#[derive(Parser)]
#[command(
    name = "xguitare",
    version,
    about = "Live guitar amp and effects rack",
    arg_required_else_help = false
)]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the Daemon: JACK client, Rack, auto-connect
    Run,
    /// Show the active Preset, audio settings and connections
    Status,
    /// List the Bank's Presets
    List,
    /// Switch to a Preset by name or program number
    Load { preset: String },
    /// Set a Parameter: <module>.<param> <value>
    Set {
        param: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
        #[command(flatten)]
        rack: MixerRack,
    },
    /// Show Parameters: all, one Module's, or one
    Get {
        param: Option<String>,
        #[command(flatten)]
        rack: MixerRack,
    },
    /// Write the live Parameter values back to the active Preset file, or with --as to a new Preset
    Save {
        #[arg(long = "as", value_name = "NAME")]
        name: Option<String>,
    },
    /// List the Module types, their channels and Parameters
    Modules,
    /// Edit the live Rack's layout
    #[command(subcommand, arg_required_else_help = false)]
    Rack(RackCmd),
    /// List or load Cabinet IRs
    #[command(subcommand, arg_required_else_help = false)]
    Ir(IrCmd),
    /// Print the Tuner reading; --watch keeps printing until Ctrl-C and, with tuner_mute, mutes meanwhile
    #[allow(
        clippy::doc_markdown,
        reason = "clap prints this doc as help text, where backticks would show"
    )]
    Tuner {
        #[arg(long)]
        watch: bool,
    },
    /// Turn Mute on or off
    Mute { state: MuteState },
    /// Bypass a Module (on) or switch it back in (off)
    Bypass {
        id: String,
        state: BypassState,
        #[command(flatten)]
        rack: MixerRack,
    },
    /// Create, open, save, close or list Projects
    #[command(subcommand, arg_required_else_help = false)]
    Project(ProjectCmd),
    /// Add, remove, edit or select a Track of the open Project
    #[command(subcommand, arg_required_else_help = false)]
    Track(TrackCmd),
    /// Start playback from the playhead
    Play,
    /// Stop playback or recording; the playhead stays
    Stop,
    /// Record the armed Tracks, after the count-in when stopped
    Record,
    /// Move the playhead: <bar>.<beat>, <bar> or <seconds>s
    Locate { position: String },
    /// Set the tempo in bpm and, optionally, the meter as <beats>/<note>
    Tempo { bpm: f64, meter: Option<String> },
    /// Turn the click on or off; --count-in sets the bars of click before recording (0, 1 or 2)
    Click {
        state: Switch,
        #[arg(long)]
        count_in: Option<u32>,
    },
    /// Add, remove or edit a Bus of the open Project
    #[command(subcommand, arg_required_else_help = false)]
    Bus(BusCmd),
    /// Set a Track's Send to a Bus: a level in dB (-60 to 6) or off; --pre on takes it before the Track's volume
    Send {
        track: String,
        bus: String,
        #[arg(allow_hyphen_values = true)]
        level: String,
        #[arg(long)]
        pre: Option<Switch>,
    },
    /// Set the Master volume in dB, -60 to 6
    Master {
        #[arg(allow_negative_numbers = true)]
        volume: f32,
    },
    /// Reset clip indicators: of one Track, Bus or Master, or all
    #[command(subcommand, arg_required_else_help = false)]
    Clip(ClipCmd),
}

/// The Bus (`--bus`, index or name) or Master Rack a Rack command edits instead of the selected Track's.
#[derive(Args)]
struct MixerRack {
    /// Edit this Bus's Rack: a zero-based index or a name
    #[arg(long, conflicts_with = "master")]
    bus: Option<String>,
    /// Edit the Master Rack
    #[arg(long)]
    master: bool,
}

/// A Bus is named by its zero-based index or its name.
#[derive(Subcommand)]
enum BusCmd {
    /// Add a Bus with an empty Rack at 0 dB; without a name it is Bus <n>
    Add { name: Option<String> },
    /// Remove a Bus and every Send to it
    Remove { bus: String },
    /// Change a Bus's name, volume, pan or switches
    Edit {
        bus: String,
        #[arg(long)]
        name: Option<String>,
        /// dB, -60 to 6
        #[arg(long, allow_negative_numbers = true)]
        volume: Option<f32>,
        /// -1 (left) to 1 (right)
        #[arg(long, allow_negative_numbers = true)]
        pan: Option<f32>,
        #[arg(long)]
        mute: Option<Switch>,
        #[arg(long)]
        solo: Option<Switch>,
    },
}

#[derive(Subcommand)]
enum ClipCmd {
    /// Reset track <track>, bus <bus> or master; with nothing given, all
    Reset {
        kind: Option<ClipKind>,
        /// The Track's or Bus's zero-based index or name
        name: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ClipKind {
    Track,
    Bus,
    Master,
}

#[derive(Clone, Copy, ValueEnum)]
enum Switch {
    On,
    Off,
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Create a Project in the projects folder and open it
    New { name: String },
    /// Open a Project; --restore loads its autosave
    Open {
        name: String,
        #[arg(long)]
        restore: bool,
    },
    /// Write the open Project to its project.toml
    Save,
    /// Save and close the open Project
    Close,
    /// List the Projects in the projects folder
    List,
    /// List the open Project's Items: Track, bar.beat, length, active Take
    Items,
}

/// A Track is named by its zero-based index or its name.
#[derive(Subcommand)]
enum TrackCmd {
    /// Add a Track with the active Preset's Modules
    Add { name: Option<String> },
    /// Remove a Track
    Remove { track: String },
    /// Change a Track's input, volume, pan, switches or name
    Edit {
        track: String,
        /// A capture port; "" disconnects
        #[arg(long)]
        input: Option<String>,
        /// dB, -60 to 6
        #[arg(long, allow_negative_numbers = true)]
        volume: Option<f32>,
        /// -1 (left) to 1 (right)
        #[arg(long, allow_negative_numbers = true)]
        pan: Option<f32>,
        #[arg(long)]
        mute: Option<Switch>,
        #[arg(long)]
        solo: Option<Switch>,
        #[arg(long)]
        arm: Option<Switch>,
        #[arg(long)]
        name: Option<String>,
        /// Whether the Track's own output goes to Master
        #[arg(long)]
        to_master: Option<Switch>,
    },
    /// Make a Track the one Rack commands edit
    Select { track: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum MuteState {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum BypassState {
    On,
    Off,
}

#[derive(Subcommand)]
enum RackCmd {
    /// Add a Module with default Parameters: after <id>, at a zero-based position, or at the end
    Add {
        #[arg(value_name = "TYPE")]
        type_name: String,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, conflicts_with = "after")]
        at: Option<usize>,
        #[command(flatten)]
        rack: MixerRack,
    },
    /// Remove a Module
    Remove {
        id: String,
        #[command(flatten)]
        rack: MixerRack,
    },
    /// Move a Module to a zero-based position
    Move {
        id: String,
        to: usize,
        #[command(flatten)]
        rack: MixerRack,
    },
}

#[derive(Subcommand)]
enum IrCmd {
    /// List the Cabinet IRs under the IR directory
    List,
    /// Load a Cabinet IR into the cabinet Module (the first one, or --id)
    Set {
        file: String,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        rack: MixerRack,
    },
}

/// Parses `argv` and runs the command; returns the process exit code (0 on success, `--help` and `--version`; 1 on any error, after one line on stderr).
pub fn run_main<I, T>(argv: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    let cli = match parse(&argv, &mut io::stdout(), &mut io::stderr()) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let result = match cli.cmd {
        Cmd::Run => {
            logging::init();
            daemon::run(cli.config.as_deref())
                .map(|()| 0)
                .map_err(|err| err.to_string())
        }
        Cmd::Tuner { watch: true } => run_client(&argv, cli.config.as_deref(), true),
        _ => run_client(&argv, cli.config.as_deref(), false),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    }
}

/// `stops_on_signal`: SIGINT and SIGTERM set `CliEnv::stop` instead of ending the process; only `tuner --watch` asks for it, so Ctrl-C still ends every other command.
fn run_client(
    argv: &[OsString],
    config_path: Option<&Path>,
    stops_on_signal: bool,
) -> Result<i32, String> {
    let config = Config::load(config_path).map_err(|err| err.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    if stops_on_signal {
        for signal in [SIGINT, SIGTERM] {
            signal_hook::flag::register(signal, Arc::clone(&stop))
                .map_err(|err| format!("cannot register signal handler: {err}"))?;
        }
    }
    let env = CliEnv {
        tuner_mute: config.tuner_mute,
        is_tty: io::stdout().is_terminal(),
        stop,
    };
    let mut transport: Box<dyn Transport> = if stops_on_signal {
        Box::new(Reconnecting::new(config.clone()))
    } else {
        Box::new(UnixSocketTransport::from_config(&config))
    };
    let argv: Vec<String> = argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    Ok(run_cli(
        &argv,
        &mut *transport,
        &mut io::stdout(),
        &mut io::stderr(),
        &env,
    ))
}

/// Parses `argv`; on `--help`/`--version` writes to `out` and returns `Err(0)`, on any other error writes its first line to `err` and returns `Err(1)`.
fn parse<I, T>(argv: I, out: &mut dyn Write, err: &mut dyn Write) -> Result<Cli, i32>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(argv).map_err(|error| {
        let rendered = error.render().to_string();
        let written = if error.exit_code() == 0 {
            write!(out, "{rendered}").map(|()| 0)
        } else {
            let message = rendered
                .lines()
                .take_while(|line| !line.starts_with("Usage:"))
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("; ")
                .replace(":; ", ": ");
            writeln!(err, "{message}").map(|()| 1)
        };
        written.unwrap_or(1)
    })
}
