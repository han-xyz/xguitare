use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use egui::{Button, CentralPanel, Key, Panel, Ui};
use serde_json::Value;
use tracing::error;
use xguitare_core::Config;
use xguitare_gui::model::{
    BankRow, Command, KeyEvent, Link, Snapshot, bank_rows, key_to_command, link_text,
    peak_fraction, status_line,
};

use crate::client::{self, TICK};
use crate::widgets;

const NO_PRESET: &str = "no preset";
const BANK_WIDTH: f32 = 220.0;
const DIGIT_KEYS: [Key; 9] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
];

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
        let rows = snapshot.bank.as_ref().map(bank_rows).unwrap_or_default();
        let mut commands: Vec<Command> = pressed_keys(ui)
            .into_iter()
            .filter_map(|key| key_to_command(key, &rows))
            .collect();
        Panel::bottom("status").show(ui, |ui| status_bar(ui, snapshot, Instant::now()));
        Panel::left("bank")
            .exact_size(BANK_WIDTH)
            .show(ui, |ui| commands.extend(bank_panel(ui, &rows)));
        CentralPanel::default().show(ui, |ui| preset_panel(ui, snapshot));
        for command in commands {
            // An Err means the client thread has ended; on_exit joins it.
            let _ = self.commands.send(command);
        }
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

fn pressed_keys(ui: &Ui) -> Vec<KeyEvent> {
    ui.input(|input| {
        let mut keys: Vec<KeyEvent> = (1..)
            .zip(DIGIT_KEYS)
            .filter(|(_, key)| input.key_pressed(*key))
            .map(|(digit, _)| KeyEvent::Digit(digit))
            .collect();
        if input.modifiers.command && input.key_pressed(Key::S) {
            keys.push(KeyEvent::CtrlS);
        }
        keys
    })
}

fn bank_panel(ui: &mut Ui, rows: &[BankRow]) -> Option<Command> {
    let mut clicked = None;
    for row in rows {
        let text = format!("{:>2} {}", row.program, row.name);
        match &row.error {
            Some(error) => {
                ui.add_enabled(false, Button::selectable(false, text))
                    .on_disabled_hover_text(error);
            }
            None => {
                if ui.selectable_label(row.active, text).clicked() {
                    clicked = Some(Command::Load(row.program.to_string()));
                }
            }
        }
    }
    clicked
}

fn preset_panel(ui: &mut Ui, snapshot: &Snapshot) {
    let preset = snapshot
        .status
        .as_ref()
        .and_then(|status| status["preset"].as_str());
    ui.label(preset.unwrap_or(NO_PRESET));
    if let Some(params) = &snapshot.params {
        for module in module_ids(params) {
            ui.label(module);
        }
    }
}

fn module_ids(params: &Value) -> Vec<&str> {
    let mut ids: Vec<&str> = params["params"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|param| param["module"].as_str())
        .collect();
    ids.dedup();
    ids
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
