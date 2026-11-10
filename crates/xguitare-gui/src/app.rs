use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use egui::{
    Align, Button, CentralPanel, Checkbox, Color32, ComboBox, CursorIcon, Event, Frame, Grid,
    InnerResponse, Key, Label, Layout, Margin, Modifiers, Panel, Rect, Response, RichText,
    ScrollArea, Sense, Stroke, TextEdit, Theme, Ui, UiBuilder, Vec2, pos2, vec2,
};
use serde_json::Value;
use tracing::{error, info};
use xguitare_core::control::SILENCE_DB;
use xguitare_core::{BankKind, Config};
use xguitare_gui::client::{self, TICK};
use xguitare_gui::model::{
    BankRow, ClipLatch, Command, Direction, EAR_WIDTH, Feedback, KeyEvent, KeyModifiers, Link,
    ModuleType, NO_SIGNAL, ParamRow, PeakHold, Rgb, SET_INTERVAL, Snapshot, Throttle, TunerReading,
    add_command, bank_rows, cabinet_ir, cents_fraction, drop_index, engine_readout, filter_bank,
    forget_removed_units, in_tune, key_to_command, knob_fraction, knob_param_value, link_text,
    move_target, open_units_from_string, open_units_to_string, palette, param_rows, peak_db,
    quit_commands, rack_channels, tuner_header_text, tuner_reading, tuner_toggle, unit_suffix,
};
use xguitare_gui::widgets::{self, KnobSize, colour};

const NO_PRESET: &str = "no preset";
const NO_IR: &str = "none";
const CABINET: &str = "cabinet";
const BANK_WIDTH: f32 = 220.0;
const OPEN_UNITS_KEY: &str = "open_units";
const RAIL_WIDTH: i8 = 14;
const UNIT_GAP: f32 = 4.0;
const UNIT_MARGIN: i8 = 8;
const UNIT_SIDE_MARGIN: i8 = EAR_WIDTH as i8 + 14;
const UNIT_HEIGHT: f32 = 66.0;
const OPEN_UNIT_HEIGHT: f32 = 190.0;
const METER_BRIDGE_HEIGHT: f32 = 70.0;
const UNIT_NAME_WIDTH: f32 = 120.0;
const UNIT_NAME_SIZE: f32 = 18.0;
const SMALL_TEXT_SIZE: f32 = 13.0;
const LAMP_SLOT: f32 = 18.0;
const METER_LABEL_WIDTH: f32 = 52.0;
const METERS: [(&str, &str); 2] = [("Input", "input_peak_db"), ("Output", "output_peak_db")];
const CLEAR_CLIP_HINT: &str = "Click to clear the clip lamp";
const BUTTON_SIZE: Vec2 = Vec2::new(64.0, 36.0);
const MOVE_BUTTON_SIZE: Vec2 = Vec2::splat(36.0);
const MOVE_BUTTONS_WIDTH: f32 = 90.0;
const OPEN_CHEVRON: &str = "▼";
const CLOSED_CHEVRON: &str = "▶";
const DROP_LINE_WIDTH: f32 = 2.0;
const TUNER_LABEL: &str = "Tuner";
const ADD_LABEL: &str = "Add a Module";
const PRESET_NAME_SIZE: f32 = 30.0;
const SIDEBAR_BUTTON_HEIGHT: f32 = 40.0;
const UNSAVED: &str = "unsaved changes";
const SAVE_AS_LABEL: &str = "Save as new";
const SAVE_AS_HINT: &str = "New Preset name";
const SEARCH_HINT: &str = "Search by name or number";
const ADD_COMBO_WIDTH: f32 = 320.0;
const NOTE_SIZE: f32 = 96.0;
const NO_SIGNAL_SIZE: f32 = 40.0;
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
    tuner_open: bool,
    muted_by_us: bool,
    tuner_mute: bool,
    clips: [ClipLatch; 2],
    holds: [PeakHold; 2],
    open_units: BTreeSet<String>,
    bank_query: String,
    save_as_name: Option<String>,
    bank_heading: &'static str,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        config: Config,
        config_path: Option<PathBuf>,
        stop: Arc<AtomicBool>,
    ) -> App {
        let ctx = cc.egui_ctx.clone();
        let open_units = cc
            .storage
            .and_then(|storage| storage.get_string(OPEN_UNITS_KEY))
            .map(|stored| open_units_from_string(&stored))
            .unwrap_or_default();
        ctx.set_theme(Theme::Dark);
        ctx.set_visuals_of(Theme::Dark, widgets::visuals());
        let tuner_mute = config.tuner_mute;
        let bank_heading = match config.bank_kind {
            BankKind::Guitar => "Guitar Bank",
            BankKind::Bass => "Bass Bank",
        };
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
                tuner: None,
                module_types: Vec::new(),
                irs: Vec::new(),
                last_message: None,
                failed_set: None,
            },
            stop,
            throttle: Throttle::default(),
            locals: HashMap::new(),
            feedback: Feedback::default(),
            tuner_open: false,
            muted_by_us: false,
            tuner_mute,
            clips: [ClipLatch::default(); 2],
            holds: [PeakHold::default(); 2],
            open_units,
            bank_query: String::new(),
            save_as_name: None,
            bank_heading,
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
        let mut latest = None;
        let now = Instant::now();
        for snapshot in self.snapshots.try_iter() {
            if let Some(status) = &snapshot.status {
                for (((_, key), clip), hold) in
                    METERS.iter().zip(&mut self.clips).zip(&mut self.holds)
                {
                    clip.observe(peak_db(status, key));
                    hold.observe(peak_db(status, key), now);
                }
            }
            latest = Some(snapshot);
        }
        if let Some(snapshot) = latest {
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
        Panel::bottom("status").show(ui, |ui| footer(ui, snapshot, &self.feedback, now));
        let mut bank = BankView {
            rows: &rows,
            heading: self.bank_heading,
            query: &mut self.bank_query,
            save_as_name: &mut self.save_as_name,
        };
        Panel::left("bank")
            .exact_size(BANK_WIDTH)
            .show(ui, |ui| commands.extend(sidebar(ui, snapshot, &mut bank)));
        let mut knobs = Knobs {
            throttle: &mut self.throttle,
            locals: &mut self.locals,
            revision: revision(snapshot),
            now,
        };
        let mut tuner = Tuner {
            open: &mut self.tuner_open,
            muted_by_us: &mut self.muted_by_us,
            mute_default: self.tuner_mute,
        };
        let mut view = RackView {
            knobs: &mut knobs,
            tuner: &mut tuner,
            clips: &mut self.clips,
            holds: &self.holds,
            open_units: &mut self.open_units,
        };
        CentralPanel::default().show(ui, |ui| commands.extend(rack_view(ui, snapshot, &mut view)));
        forget_removed_units(&mut self.open_units, &commands);
        for command in commands {
            // An Err means the client thread has ended; on_exit joins it.
            let _ = self.commands.send(command);
        }
        ui.ctx().request_repaint_after(TICK);
        if self.throttle.has_pending() {
            ui.ctx().request_repaint_after(SET_INTERVAL);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(OPEN_UNITS_KEY, open_units_to_string(&self.open_units));
    }

    fn on_exit(&mut self) {
        // An Err means the client thread has already ended; joining it is still right.
        let commands = quit_commands(self.tuner_open, self.muted_by_us);
        info!("quit: queued {commands:?}");
        for command in commands {
            let _ = self.commands.send(command);
        }
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

struct BankView<'a> {
    rows: &'a [BankRow],
    heading: &'static str,
    query: &'a mut String,
    save_as_name: &'a mut Option<String>,
}

/// The Preset, saving, the Bank heading and search at the top, the engine readout at the bottom, the Bank list between.
fn sidebar(ui: &mut Ui, snapshot: &Snapshot, bank: &mut BankView) -> Vec<Command> {
    preset_heading(ui, snapshot);
    let mut commands: Vec<Command> = save_buttons(ui, bank.save_as_name).into_iter().collect();
    ui.add_space(UNIT_MARGIN.into());
    ui.heading(bank.heading);
    ui.add(TextEdit::singleline(bank.query).hint_text(SEARCH_HINT));
    Panel::bottom("engine_readout").show(ui, |ui| engine_readout_grid(ui, snapshot));
    ScrollArea::vertical().show(ui, |ui| {
        commands.extend(bank_panel(ui, &filter_bank(bank.rows, bank.query)))
    });
    commands
}

fn preset_heading(ui: &mut Ui, snapshot: &Snapshot) {
    let status = snapshot.status.as_ref();
    let preset = status.and_then(|status| status["preset"].as_str());
    ui.label(
        RichText::new(preset.unwrap_or(NO_PRESET))
            .size(PRESET_NAME_SIZE)
            .strong(),
    );
    let program = status
        .and_then(|status| status["program"].as_u64())
        .map(|program| format!("Program {program}"));
    let modified = status.is_some_and(|status| status["modified"].as_bool() == Some(true));
    let line: Vec<String> = program
        .into_iter()
        .chain(modified.then(|| UNSAVED.to_string()))
        .collect();
    if !line.is_empty() {
        ui.label(muted_text(line.join(", ")));
    }
}

/// `Save as new` opens a name field; `Save` beside it sends the name, so that the Daemon's error for an empty or taken name reaches the footer.
fn save_buttons(ui: &mut Ui, save_as_name: &mut Option<String>) -> Option<Command> {
    let button_size = vec2(
        (ui.available_width() - ui.spacing().item_spacing.x) / 2.0,
        SIDEBAR_BUTTON_HEIGHT,
    );
    let mut command = None;
    ui.horizontal(|ui| {
        if ui.add(Button::new("Save").min_size(button_size)).clicked() {
            command = Some(Command::Save);
        }
        if ui
            .add(Button::new(SAVE_AS_LABEL).min_size(button_size))
            .clicked()
        {
            *save_as_name = save_as_name.is_none().then(String::new);
        }
    });
    let Some(name) = save_as_name else {
        return command;
    };
    let field = ui.add(TextEdit::singleline(name).hint_text(SAVE_AS_HINT));
    let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    let (confirm, cancel) = ui
        .horizontal(|ui| (ui.button("Save").clicked(), ui.button("Cancel").clicked()))
        .inner;
    if entered || confirm {
        command = Some(Command::SaveAs(name.trim().to_string()));
        *save_as_name = None;
    } else if cancel {
        *save_as_name = None;
    }
    command
}

fn engine_readout_grid(ui: &mut Ui, snapshot: &Snapshot) {
    let rows = engine_readout(snapshot.status.as_ref());
    Grid::new("engine_readout_rows").show(ui, |ui| {
        for (label, value) in rows {
            ui.label(muted_text(label));
            ui.label(value);
            ui.end_row();
        }
    });
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

struct Tuner<'a> {
    open: &'a mut bool,
    muted_by_us: &'a mut bool,
    mute_default: bool,
}

impl Tuner<'_> {
    fn toggle(&mut self) -> Option<Command> {
        let (open, command, muted_by_us) =
            tuner_toggle(*self.open, self.mute_default, *self.muted_by_us);
        *self.open = open;
        *self.muted_by_us = muted_by_us;
        command
    }
}

struct RackView<'a, 'b> {
    knobs: &'a mut Knobs<'b>,
    tuner: &'a mut Tuner<'b>,
    clips: &'a mut [ClipLatch; 2],
    holds: &'a [PeakHold; 2],
    open_units: &'a mut BTreeSet<String>,
}

fn rack_view(ui: &mut Ui, snapshot: &Snapshot, view: &mut RackView) -> Vec<Command> {
    let mut commands = Vec::new();
    let modules = snapshot.params.as_ref().map(param_rows).unwrap_or_default();
    ScrollArea::vertical().show(ui, |ui| {
        Frame::new()
            .fill(colour(palette().rail))
            .inner_margin(Margin::symmetric(RAIL_WIDTH, 0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = UNIT_GAP;
                meter_bridge(ui, snapshot, view.clips, view.holds);
                commands.extend(tuner_unit(ui, snapshot, view.tuner));
                commands.extend(module_units(
                    ui,
                    snapshot,
                    &modules,
                    view.knobs,
                    view.open_units,
                ));
                let rack = rack_channels(&modules, &snapshot.module_types);
                commands.extend(add_row(ui, &snapshot.module_types, &rack));
            });
    });
    commands
}

fn rack_unit<R>(
    ui: &mut Ui,
    fill: Rgb,
    min_height: f32,
    content: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let colours = palette();
    let inner = Frame::new()
        .fill(colour(fill))
        .inner_margin(Margin::symmetric(UNIT_SIDE_MARGIN, UNIT_MARGIN))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(min_height - 2.0 * f32::from(UNIT_MARGIN));
            content(ui)
        });
    let rect = inner.response.rect;
    let painter = ui.painter();
    painter.hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0, colour(colours.edge_light)),
    );
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, colour(colours.edge_dark)),
    );
    widgets::rack_ears(painter, rect);
    inner
}

fn unit_lamp(ui: &mut Ui, lit_colour: Color32, lit: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(LAMP_SLOT), Sense::hover());
    widgets::lamp(ui.painter(), rect.center(), lit_colour, lit);
}

fn unit_name(ui: &mut Ui, name: &str) {
    let text = RichText::new(name).strong().size(UNIT_NAME_SIZE);
    ui.add_sized(vec2(UNIT_NAME_WIDTH, 0.0), Label::new(text));
}

fn muted_text(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .size(SMALL_TEXT_SIZE)
        .color(colour(palette().muted))
}

/// A click on either ladder clears both clip lamps.
fn meter_bridge(
    ui: &mut Ui,
    snapshot: &Snapshot,
    clips: &mut [ClipLatch; 2],
    holds: &[PeakHold; 2],
) {
    let mut cleared = false;
    rack_unit(ui, palette().meter_bridge, METER_BRIDGE_HEIGHT, |ui| {
        for (((label, key), clip), hold) in METERS.iter().zip(clips.iter()).zip(holds) {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(METER_LABEL_WIDTH, 0.0), Label::new(muted_text(*label)));
                let db = snapshot
                    .status
                    .as_ref()
                    .map_or(SILENCE_DB, |status| peak_db(status, key));
                let ladder = widgets::led_ladder(ui, db, clip.is_clipped())
                    .interact(Sense::click())
                    .on_hover_text(CLEAR_CLIP_HINT);
                cleared |= ladder.clicked();
                ui.label(muted_text(peak_text(hold.held_db(), clip.is_clipped())));
            });
        }
    });
    if cleared {
        clips.iter_mut().for_each(ClipLatch::reset);
    }
}

fn peak_text(db: f64, clipped: bool) -> String {
    if clipped {
        format!("peak {db:+.1} dB, clipped")
    } else {
        format!("peak {db:+.1} dB")
    }
}

fn tuner_unit(ui: &mut Ui, snapshot: &Snapshot, tuner: &mut Tuner) -> Vec<Command> {
    let open = *tuner.open;
    let (fill, height) = if open {
        (palette().unit_open, OPEN_UNIT_HEIGHT)
    } else {
        (palette().unit, UNIT_HEIGHT)
    };
    let reading = snapshot.tuner.as_ref().and_then(tuner_reading);
    rack_unit(ui, fill, height, |ui| {
        let mut commands = Vec::new();
        ui.horizontal(|ui| {
            unit_lamp(ui, colour(palette().green), open);
            unit_name(ui, TUNER_LABEL);
            ui.label(RichText::new(tuner_header_text(reading.as_ref())).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if open { "Close" } else { TUNER_LABEL };
                if ui.add(Button::new(label).min_size(BUTTON_SIZE)).clicked() {
                    commands.extend(tuner.toggle());
                }
            });
        });
        if open {
            commands.extend(tuner_view(ui, snapshot, reading.as_ref(), tuner));
        }
        commands
    })
    .inner
}

fn tuner_view(
    ui: &mut Ui,
    snapshot: &Snapshot,
    reading: Option<&TunerReading>,
    tuner: &mut Tuner,
) -> Option<Command> {
    let muted = snapshot
        .status
        .as_ref()
        .and_then(|status| status["muted"].as_bool());
    ui.vertical_centered(|ui| {
        match reading {
            Some(reading) => {
                let colour = widgets::tune_colour(in_tune(reading.cents));
                ui.label(RichText::new(&reading.note).size(NOTE_SIZE).color(colour));
                widgets::cents_arc(ui, Some(cents_fraction(reading.cents)), colour);
                ui.label(RichText::new(format!("{:.1} Hz", reading.hz)).color(colour));
            }
            None => {
                ui.label(RichText::new(NO_SIGNAL).size(NO_SIGNAL_SIZE));
                widgets::cents_arc(ui, None, Color32::GRAY);
            }
        }
        let mut checked = muted.unwrap_or_default();
        let changed = ui
            .add_enabled(muted.is_some(), Checkbox::new(&mut checked, "Mute"))
            .changed();
        if !changed {
            return None;
        }
        if !checked {
            *tuner.muted_by_us = false;
        }
        Some(Command::Mute(checked))
    })
    .inner
}

/// A unit dropped by its ears moves to the unit under the pointer; while it is dragged, a line marks where it will land.
fn module_units(
    ui: &mut Ui,
    snapshot: &Snapshot,
    modules: &[(String, Vec<ParamRow>)],
    knobs: &mut Knobs,
    open_units: &mut BTreeSet<String>,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut placed = Vec::with_capacity(modules.len());
    for (index, (module, rows)) in modules.iter().enumerate() {
        let is_cabinet = rows.first().is_some_and(|row| row.module_type == CABINET);
        let unit = ModuleUnit {
            module,
            rows,
            place: (index, modules.len()),
            ir_choice: snapshot
                .params
                .as_ref()
                .filter(|_| is_cabinet)
                .map(|params| IrChoice {
                    current: cabinet_ir(params, module),
                    irs: &snapshot.irs,
                }),
        };
        let (unit_commands, rect, ears) = module_unit(ui, &unit, knobs, open_units);
        commands.extend(unit_commands);
        placed.push((rect, ears));
    }
    let tops: Vec<f32> = placed.iter().map(|(rect, _)| rect.top()).collect();
    let pointer_y = ui
        .input(|input| input.pointer.latest_pos())
        .map(|pos| pos.y);
    for (from, (_, ears)) in placed.iter().enumerate() {
        let to = drop_index(from, pointer_y, &tops);
        if ears.dragged() {
            let (target, _) = &placed[to];
            let y = if to > from {
                target.bottom()
            } else {
                target.top()
            };
            ui.painter().hline(
                target.x_range(),
                y,
                Stroke::new(DROP_LINE_WIDTH, colour(palette().text)),
            );
        }
        if ears.drag_stopped() && to != from {
            commands.push(Command::RackMove {
                id: modules[from].0.clone(),
                to,
            });
        }
    }
    commands
}

struct ModuleUnit<'a> {
    module: &'a str,
    rows: &'a [ParamRow],
    place: (usize, usize),
    ir_choice: Option<IrChoice<'a>>,
}

/// The unit's commands, its rect, and the drag response of its two ears.
fn module_unit(
    ui: &mut Ui,
    unit: &ModuleUnit,
    knobs: &mut Knobs,
    open_units: &mut BTreeSet<String>,
) -> (Vec<Command>, Rect, Response) {
    let module = unit.module;
    let open = open_units.contains(module);
    let type_name = unit.rows.first().map_or("", |row| row.module_type.as_str());
    let accent = widgets::accent(type_name);
    let (fill, height) = if open {
        (palette().unit_open, OPEN_UNIT_HEIGHT)
    } else {
        (palette().unit, UNIT_HEIGHT)
    };
    let mut toggled = false;
    let inner = rack_unit(ui, fill, height, |ui| {
        let mut commands = Vec::new();
        let header = UiBuilder::new()
            .layout(Layout::left_to_right(Align::Center))
            .sense(Sense::click());
        let header = ui.scope_builder(header, |ui| {
            unit_lamp(ui, accent, true);
            unit_name(ui, &module_label(module, unit.rows));
            let chevron = if open { OPEN_CHEVRON } else { CLOSED_CHEVRON };
            toggled |= ui.small_button(chevron).clicked();
            let knobs_width = (ui.available_width() - MOVE_BUTTONS_WIDTH).max(0.0);
            let wrapped = Layout::left_to_right(Align::Center).with_main_wrap(true);
            ui.allocate_ui_with_layout(vec2(knobs_width, 0.0), wrapped, |ui| {
                if !open {
                    commands.extend(param_knobs(ui, unit.rows, KnobSize::Small, accent, knobs));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                commands.extend(move_buttons(ui, module, unit.place));
            });
        });
        toggled |= header.response.clicked();
        if open {
            ui.horizontal_wrapped(|ui| {
                commands.extend(param_knobs(ui, unit.rows, KnobSize::Large, accent, knobs));
            });
            if let Some(ir_choice) = &unit.ir_choice {
                commands.extend(ir_row(ui, module, ir_choice));
            }
            if ui.button("Remove").clicked() {
                commands.push(Command::RackRemove {
                    id: module.to_string(),
                });
            }
        }
        commands
    });
    if toggled && !open_units.remove(module) {
        open_units.insert(module.to_string());
    }
    let rect = inner.response.rect;
    let ear_size = vec2(EAR_WIDTH, rect.height());
    let ears = [
        Rect::from_min_size(rect.min, ear_size),
        Rect::from_min_size(pos2(rect.right() - EAR_WIDTH, rect.top()), ear_size),
    ]
    .into_iter()
    .enumerate()
    .map(|(side, ear)| {
        ui.interact(
            ear,
            ui.scope_id().with(("ears", module, side)),
            Sense::drag(),
        )
    })
    .reduce(|left, right| left.union(right))
    .expect("a unit has two ears")
    .on_hover_cursor(CursorIcon::Grab);
    (inner.inner, rect, ears)
}
struct IrChoice<'a> {
    current: Option<String>,
    irs: &'a [String],
}

/// A click on the combo, opening or closing it, refetches `ir_list`, so that a Cabinet IR copied into the IR directory shows without a reconnect.
fn ir_row(ui: &mut Ui, module: &str, ir_choice: &IrChoice) -> Vec<Command> {
    let mut commands = Vec::new();
    ui.horizontal(|ui| {
        ui.label("IR");
        let current = ir_choice.current.as_deref();
        let combo = ComboBox::from_id_salt(("ir", module))
            .selected_text(current.unwrap_or(NO_IR))
            .show_ui(ui, |ui| {
                for name in ir_choice.irs {
                    if ui
                        .selectable_label(current == Some(name.as_str()), name)
                        .clicked()
                    {
                        commands.push(Command::RackIr {
                            id: module.to_string(),
                            ir: name.clone(),
                        });
                    }
                }
            });
        if combo.response.clicked() {
            commands.push(Command::RefreshIrs);
        }
    });
    commands
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
        .selected_text(ADD_LABEL)
        .width(ADD_COMBO_WIDTH)
        .show_ui(ui, |ui| {
            for module_type in module_types {
                if ui.selectable_label(false, module_type.label()).clicked() {
                    added = Some(add_command(rack, module_type));
                }
            }
        });
    added
}

fn add_row(ui: &mut Ui, module_types: &[ModuleType], rack: &[(String, String)]) -> Option<Command> {
    let colours = palette();
    Frame::new()
        .fill(colour(colours.knob_body))
        .stroke(Stroke::new(1.0, colour(colours.button_border)))
        .inner_margin(Margin::same(UNIT_MARGIN))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical_centered(|ui| add_combo(ui, module_types, rack))
                .inner
        })
        .inner
}

/// Added right to left, so that they read ↑ ↓ at the right of the unit.
fn move_buttons(ui: &mut Ui, module: &str, (index, len): (usize, usize)) -> Option<Command> {
    let mut clicked = None;
    for (text, direction) in [("↓", Direction::Down), ("↑", Direction::Up)] {
        let to = move_target(index, direction, len);
        if ui
            .add_enabled(to.is_some(), Button::new(text).min_size(MOVE_BUTTON_SIZE))
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

fn param_knobs(
    ui: &mut Ui,
    rows: &[ParamRow],
    size: KnobSize,
    accent: Color32,
    knobs: &mut Knobs,
) -> Vec<Command> {
    rows.iter()
        .filter_map(|row| param_knob(ui, row, size, accent, knobs))
        .collect()
}

/// A double-click reset is a final value, like a release.
fn param_knob(
    ui: &mut Ui,
    row: &ParamRow,
    size: KnobSize,
    accent: Color32,
    knobs: &mut Knobs,
) -> Option<Command> {
    let held = knobs.throttle.hold_local(&row.address, knobs.revision);
    let shown = match knobs.locals.get(&row.address) {
        Some(local) if held => *local,
        _ => row.value,
    };
    let fraction = knob_fraction(shown as f32, row.min as f32, row.max as f32);
    ui.horizontal(|ui| {
        let knob = widgets::rack_knob(ui, size, fraction, accent);
        let value = if knob.reset || knob.response.dragged() {
            knob_param_value(row, knob.fraction, knob.reset)
        } else {
            shown
        };
        ui.vertical(|ui| {
            ui.label(muted_text(&row.param));
            ui.label(RichText::new(value_text(value, row)).strong());
        });
        if knob.reset || knob.response.drag_stopped() {
            knobs.locals.insert(row.address.clone(), value);
            return Some(
                knobs
                    .throttle
                    .on_release(&row.address, value, knobs.revision),
            );
        }
        if value == shown {
            return None;
        }
        knobs.locals.insert(row.address.clone(), value);
        knobs.throttle.on_drag(&row.address, value, knobs.now)
    })
    .inner
}

fn value_text(value: f64, row: &ParamRow) -> String {
    let suffix = unit_suffix(&row.unit);
    if row.integer {
        format!("{}{suffix}", value.round())
    } else {
        format!("{value:.1}{suffix}")
    }
}

fn footer(ui: &mut Ui, snapshot: &Snapshot, feedback: &Feedback, now: Instant) {
    ui.horizontal(|ui| {
        if snapshot.link != Link::Connected {
            ui.label(link_text(&snapshot.link, now));
        }
        if let Some(text) = feedback.text(now) {
            ui.label(text);
        }
    });
}
