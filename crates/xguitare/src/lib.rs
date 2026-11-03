mod cli;
mod daemon;
mod jack_adapter;
mod logging;
mod socket_server;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use xguitare_core::Config;

pub use cli::{Transport, TransportError, UnixSocketTransport, run_cli};

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
        _ => Config::load(cli.config.as_deref())
            .map_err(|err| err.to_string())
            .map(|config| {
                let argv: Vec<String> = argv
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                let mut transport = UnixSocketTransport::new(config.socket);
                run_cli(&argv, &mut transport, &mut io::stdout(), &mut io::stderr())
            }),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    }
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
            writeln!(err, "{}", rendered.lines().next().unwrap_or_default()).map(|()| 1)
        };
        written.unwrap_or(1)
    })
}
