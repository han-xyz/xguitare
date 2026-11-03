mod daemon;
mod jack_adapter;
mod logging;

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xguitare", version, about = "Live guitar amp and effects rack")]
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
}

/// Parses `argv` and runs the command; returns the process exit code (0 on success, `--help` and `--version`; 1 on any error, after one line on stderr).
pub fn run_main<I, T>(argv: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(err) if err.exit_code() == 0 => return err.print().map_or(1, |()| 0),
        Err(err) => {
            let rendered = err.render().to_string();
            eprintln!("{}", rendered.lines().next().unwrap_or_default());
            return 1;
        }
    };
    match cli.cmd {
        Cmd::Run => {
            logging::init();
            match daemon::run(cli.config.as_deref()) {
                Ok(()) => 0,
                Err(err) => {
                    eprintln!("error: {err}");
                    1
                }
            }
        }
    }
}
