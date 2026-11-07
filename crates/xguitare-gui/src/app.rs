use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use egui::{
    Button, CentralPanel, Color32, ComboBox, CornerRadius, Event, Frame, Key, Margin, Modifiers,
    Panel, Rect, RichText, ScrollArea, Sides, Theme, Ui, Visuals, vec2,
};
use serde_json::Value;
use tracing::error;
use xguitare_core::Config;
use xguitare_gui::client::{self, TICK};
use xguitare_gui::model::{
    BankRow, Command, Direction, Feedback, KeyEvent, KeyModifiers, Link, ModuleType, ParamRow,
    SET_INTERVAL, Snapshot, Throttle, accent_for, add_command, bank_rows, key_to_command,
    link_text, move_target, param_default, param_rows, peak_fraction, rack_channels, status_line,
    unit_suffix,
};

use crate::widgets::{self, KnobSpec};

const NO_PRESET: &str = "no preset";
const BANK_WIDTH: f32 = 220.0;
const PANEL_FILL: Color32 = Color32::from_rgb(28, 26, 24);
const CARD_FILL: Color32 = Color32::from_rgb(40, 37, 34);
const CARD_RADIUS: u8 = 8;
const CARD_MARGIN: i8 = 8;
const CARD_GAP: f32 = 8.0;
const BAND_HEIGHT: f32 = 4.0;
const AMP_BAND_HEIGHT: f32 = 12.0;
const PLATE_FILL: Color32 = Color32::from_rgb(16, 15, 14);
const PLATE_TEXT: Color32 = Color32::from_rgb(230, 225, 215);
const PLATE_RADIUS: u8 = 4;
const AMP_LABEL: &str = "amp";
const AMP_TYPES: [&str; 2] = ["preamp", "tonestack"];
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
    throttle: Throttle,
    locals: HashMap<String, f64>,
    feedback: Feedback,
}

impl App {
    pub fn new(
        ctx: egui::Context,
        config: Config,
        config_path: Option<PathBuf>,
        stop: Arc<AtomicBool>,
    ) -> App {
        ctx.set_theme(Theme::Dark);
        ctx.set_visuals_of(
            Theme::Dark,
            Visuals {
                panel_fill: PANEL_FILL,
                ..Visuals::dark()
            },
        );
        let (commands, snapshots, client) =
            client::spawn(config, config_path, Arc::clone(&stop), move || {
                ctx.request_repaint()
            });
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
                module_types: Vec::new(),
                last_message: None,
                failed_set: None,
            },
            stop,
            throttle: Throttle::default(),
            locals: HashMap::new(),
            feedback: Feedback::default(),
        }
    }
}

impl App {
    fn release_stale_holds(&mut self, snapshot: &Snapshot) {
        if snapshot.failed_set != self.snapshot.failed_set
            && let Some((address, _)) = &snapshot.failed_set
        {
            self.throttle.forget(address);
        }
        if active_preset(snapshot) != active_preset(&self.snapshot) {
            self.throttle = Throttle::default();
            return;
        }
        let present: HashSet<String> = snapshot
            .params
            .as_ref()
            .map(param_rows)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|(_, rows)| rows.into_iter().map(|row| row.address))
            .collect();
        self.throttle.retain(|address| present.contains(address));
    }
}

fn active_preset(snapshot: &Snapshot) -> (Option<&Value>, Option<&Value>) {
    let status = snapshot.status.as_ref();
    (
        status.map(|status| &status["preset"]),
        status.map(|status| &status["program"]),
    )
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.stop.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(snapshot) = self.snapshots.try_iter().last() {
            if snapshot.last_message != self.snapshot.last_message
                && let Some((text, at)) = &snapshot.last_message
            {
                self.feedback.show(text.clone(), *at);
            }
            self.release_stale_holds(&snapshot);
            self.snapshot = snapshot;
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let snapshot = &self.snapshot;
        let now = Instant::now();
        let rows = snapshot
            .bank
            .as_ref()
            .map(|list| bank_rows(list).0)
            .unwrap_or_default();
        let mut commands: Vec<Command> = pressed_keys(ui)
            .into_iter()
            .filter_map(|key| key_to_command(key, &rows))
            .collect();
        commands.extend(self.throttle.tick(now));
        Panel::bottom("status").show(ui, |ui| status_bar(ui, snapshot, &self.feedback, now));
        Panel::left("bank")
            .exact_size(BANK_WIDTH)
            .show(ui, |ui| commands.extend(bank_panel(ui, &rows)));
        let mut knobs = Knobs {
            throttle: &mut self.throttle,
            locals: &mut self.locals,
            revision: revision(snapshot),
            now,
        };
        CentralPanel::default().show(ui, |ui| {
            commands.extend(preset_panel(ui, snapshot, &mut knobs))
        });
        for command in commands {
            // An Err means the client thread has ended; on_exit joins it.
            let _ = self.commands.send(command);
        }
        ui.ctx().request_repaint_after(TICK);
        if self.throttle.has_pending() {
            ui.ctx().request_repaint_after(SET_INTERVAL);
        }
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

/// No shortcut while a widget takes typing. Keys match the key event's own modifiers: an injected or fast press releases Ctrl within the same frame, so the frame's modifier state reads none.
fn pressed_keys(ui: &Ui) -> Vec<KeyEvent> {
    if ui.ctx().egui_wants_keyboard_input() {
        return Vec::new();
    }
    ui.input_mut(|input| {
        let mut keys: Vec<KeyEvent> = input
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => (1..)
                    .zip(DIGIT_KEYS)
                    .find(|(_, digit_key)| digit_key == key)
                    .map(|(digit, _)| {
                        KeyEvent::Digit(
                            digit,
                            KeyModifiers {
                                ctrl: modifiers.ctrl,
                                alt: modifiers.alt,
                            },
                        )
                    }),
                _ => None,
            })
            .collect();
        if input.consume_key(Modifiers::COMMAND, Key::S) {
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

struct Knobs<'a> {
    throttle: &'a mut Throttle,
    locals: &'a mut HashMap<String, f64>,
    revision: u64,
    now: Instant,
}

fn revision(snapshot: &Snapshot) -> u64 {
    snapshot
        .status
        .as_ref()
        .and_then(|status| status["revision"].as_u64())
        .unwrap_or_default()
}

fn preset_panel(ui: &mut Ui, snapshot: &Snapshot, knobs: &mut Knobs) -> Vec<Command> {
    let mut commands = Vec::new();
    let preset = snapshot
        .status
        .as_ref()
        .and_then(|status| status["preset"].as_str());
    let modules = snapshot.params.as_ref().map(param_rows).unwrap_or_default();
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            commands.push(Command::Save);
        }
        ui.label(preset.unwrap_or(NO_PRESET));
        let rack = rack_channels(&modules, &snapshot.module_types);
        commands.extend(add_combo(ui, &snapshot.module_types, &rack));
    });
    ScrollArea::vertical().show(ui, |ui| {
        for (index, (module, rows)) in modules.iter().enumerate() {
            let place = (index, modules.len());
            commands.extend(module_card(ui, module, rows, place, snapshot, knobs));
            ui.add_space(CARD_GAP);
        }
    });
    commands
}

fn module_card(
    ui: &mut Ui,
    module: &str,
    rows: &[ParamRow],
    (index, len): (usize, usize),
    snapshot: &Snapshot,
    knobs: &mut Knobs,
) -> Vec<Command> {
    let type_name = rows.first().map_or("", |row| row.module_type.as_str());
    let is_amp = AMP_TYPES.contains(&type_name);
    let band = if is_amp { AMP_BAND_HEIGHT } else { BAND_HEIGHT };
    let card = Frame::new()
        .fill(CARD_FILL)
        .corner_radius(CARD_RADIUS)
        .inner_margin(Margin {
            top: band as i8 + CARD_MARGIN,
            ..Margin::same(CARD_MARGIN)
        })
        .show(ui, |ui| {
            let (_, clicked) = Sides::new().show(
                ui,
                |ui| {
                    label_plate(ui, &module_label(module, rows));
                    if is_amp {
                        ui.label(AMP_LABEL);
                    }
                },
                |ui| module_buttons(ui, module, index, len),
            );
            let mut commands: Vec<Command> = clicked.into_iter().collect();
            ui.horizontal_wrapped(|ui| {
                commands.extend(
                    rows.iter()
                        .filter_map(|row| param_knob(ui, row, &snapshot.module_types, knobs)),
                );
            });
            commands
        });
    let rect = card.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_size(rect.min, vec2(rect.width(), band)),
        CornerRadius {
            nw: CARD_RADIUS,
            ne: CARD_RADIUS,
            sw: 0,
            se: 0,
        },
        accent_for(type_name),
    );
    card.inner
}

fn label_plate(ui: &mut Ui, text: &str) {
    Frame::new()
        .fill(PLATE_FILL)
        .corner_radius(PLATE_RADIUS)
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(PLATE_TEXT).strong())
        });
}

fn module_label(module: &str, rows: &[ParamRow]) -> String {
    match rows.first() {
        Some(row) if row.module_type != module => format!("{module} ({})", row.module_type),
        _ => module.to_string(),
    }
}

fn add_combo(
    ui: &mut Ui,
    module_types: &[ModuleType],
    rack: &[(String, String)],
) -> Option<Command> {
    let mut added = None;
    ComboBox::from_id_salt("add_module")
        .selected_text("Add")
        .show_ui(ui, |ui| {
            for module_type in module_types {
                if ui.selectable_label(false, module_type.label()).clicked() {
                    added = Some(add_command(rack, module_type));
                }
            }
        });
    added
}

/// Added right to left, so that they read ▲ ▼ ✕ at the right of the card header.
fn module_buttons(ui: &mut Ui, module: &str, index: usize, len: usize) -> Option<Command> {
    let mut clicked = None;
    if ui.small_button("✕").clicked() {
        clicked = Some(Command::RackRemove {
            id: module.to_string(),
        });
    }
    for (text, direction) in [("▼", Direction::Down), ("▲", Direction::Up)] {
        let to = move_target(index, direction, len);
        if ui
            .add_enabled(to.is_some(), Button::new(text).small())
            .clicked()
            && let Some(to) = to
        {
            clicked = Some(Command::RackMove {
                id: module.to_string(),
                to,
            });
        }
    }
    clicked
}

/// A double-click reset is a final value, like a release.
fn param_knob(
    ui: &mut Ui,
    row: &ParamRow,
    module_types: &[ModuleType],
    knobs: &mut Knobs,
) -> Option<Command> {
    let held = knobs.throttle.hold_local(&row.address, knobs.revision);
    let shown = match knobs.locals.get(&row.address) {
        Some(local) if held => *local,
        _ => row.value,
    };
    let mut value = shown as f32;
    let spec = KnobSpec {
        min: row.min as f32,
        max: row.max as f32,
        default: param_default(module_types, &row.module_type, &row.param)
            .map(|default| default as f32),
        integer: row.integer,
        label: &row.param,
        suffix: unit_suffix(&row.unit),
    };
    let response = widgets::knob(ui, &mut value, &spec);
    if !response.changed && !response.drag_stopped {
        return None;
    }
    let value = if response.changed {
        f64::from(value)
    } else {
        shown
    };
    knobs.locals.insert(row.address.clone(), value);
    if response.drag_stopped || response.reset {
        return Some(
            knobs
                .throttle
                .on_release(&row.address, value, knobs.revision),
        );
    }
    knobs.throttle.on_drag(&row.address, value, knobs.now)
}

fn status_bar(ui: &mut Ui, snapshot: &Snapshot, feedback: &Feedback, now: Instant) {
    ui.horizontal(|ui| {
        ui.label(link_text(&snapshot.link, now));
        if let Some(text) = feedback.text(now) {
            ui.separator();
            ui.label(text);
        }
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
