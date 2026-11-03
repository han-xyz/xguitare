mod app;
mod widgets;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::Parser;
use signal_hook::consts::{SIGINT, SIGTERM};
use tracing_subscriber::EnvFilter;
use xguitare_core::Config;

use crate::app::App;

const APP_NAME: &str = "xguitare";
const WINDOW_SIZE: [f32; 2] = [960.0, 640.0];

#[derive(Parser)]
#[command(
    name = "xguitare-gui",
    version,
    about = "Window for the xguitare Daemon"
)]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) if err.exit_code() == 0 => err.exit(),
        Err(err) => {
            let rendered = err.render().to_string();
            eprintln!("{}", rendered.lines().next().unwrap_or_default());
            return ExitCode::FAILURE;
        }
    };
    init_logging();
    match run(cli.config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(config_path: Option<PathBuf>) -> Result<(), String> {
    let config = Config::load(config_path.as_deref()).map_err(|err| err.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop)).map_err(|err| err.to_string())?;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size(WINDOW_SIZE),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                cc.egui_ctx.clone(),
                config,
                config_path,
                stop,
            )))
        }),
    )
    .map_err(|err| err.to_string())
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
