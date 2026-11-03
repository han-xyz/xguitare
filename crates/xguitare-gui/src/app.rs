use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use egui::{CentralPanel, Panel, Ui};
use tracing::error;
use xguitare_core::Config;
use xguitare_gui::model::{Command, Link, Snapshot, link_text, peak_fraction, status_line};

use crate::client::{self, TICK};
use crate::widgets;

const NO_PRESET: &str = "no preset";

pub struct App {
    commands: Sender<Command>,
    snapshots: Receiver<Snapshot>,
    client: Option<JoinHandle<()>>,
    snapshot: Snapshot,
    stop: Arc<AtomicBool>,
}

impl App {
    pub fn new(
        ctx: egui::Context,
        config: Config,
        config_path: Option<PathBuf>,
        stop: Arc<AtomicBool>,
    ) -> App {
        let (commands, snapshots, client) =
            client::spawn(config, config_path, Arc::clone(&stop), ctx);
        App {
            commands,
            snapshots,
            client: Some(client),
            snapshot: Snapshot {
                link: Link::Disconnected {
                    since: Instant::now(),
                },
                status: None,
                bank: None,
                params: None,
                last_message: None,
            },
            stop,
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.stop.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(snapshot) = self.snapshots.try_iter().last() {
            self.snapshot = snapshot;
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let snapshot = &self.snapshot;
        Panel::bottom("status").show(ui, |ui| status_bar(ui, snapshot, Instant::now()));
        Panel::left("bank").show(ui, |ui| ui.label("Bank"));
        CentralPanel::default().show(ui, |ui| {
            let preset = snapshot
                .status
                .as_ref()
                .and_then(|status| status["preset"].as_str());
            ui.label(preset.unwrap_or(NO_PRESET));
        });
        ui.ctx().request_repaint_after(TICK);
    }

    fn on_exit(&mut self) {
        // An Err means the client thread has already ended; joining it is still right.
        let _ = self.commands.send(Command::Quit);
        if let Some(client) = self.client.take()
            && client.join().is_err()
        {
            error!("client thread panicked");
        }
    }
}

fn status_bar(ui: &mut Ui, snapshot: &Snapshot, now: Instant) {
    ui.horizontal(|ui| {
        ui.label(link_text(&snapshot.link, now));
        let Some(status) = &snapshot.status else {
            return;
        };
        ui.separator();
        ui.label(status_line(status));
        ui.separator();
        widgets::meter(ui, "in", peak_fraction(status, "input_peak_db"));
        widgets::meter(ui, "out", peak_fraction(status, "output_peak_db"));
    });
}
