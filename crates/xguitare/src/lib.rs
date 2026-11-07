mod cli;
mod daemon;
mod jack_adapter;
mod logging;
pub mod socket_server;

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand, ValueEnum};
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
    },
    /// Show Parameters: all, one Module's, or one
    Get { param: Option<String> },
    /// Write the live Parameter values back to the active Preset file
    Save,
    /// List the Module types, their channels and Parameters
    Modules,
    /// Edit the live Rack's layout
    #[command(subcommand)]
    Rack(RackCmd),
    /// Print the Tuner reading; --watch keeps printing until Ctrl-C and, with tuner_mute, mutes meanwhile
    Tuner {
        #[arg(long)]
        watch: bool,
    },
    /// Turn Mute on or off
    Mute { state: MuteState },
}

#[derive(Clone, Copy, ValueEnum)]
enum MuteState {
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
    },
    /// Remove a Module
    Remove { id: String },
    /// Move a Module to a zero-based position
    Move { id: String, to: usize },
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
        Box::new(UnixSocketTransport::from_config(&config).map_err(|err| err.to_string())?)
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
