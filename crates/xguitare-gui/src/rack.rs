use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use egui::{
    Align, Button, Checkbox, Color32, ComboBox, CursorIcon, Frame, Id, InnerResponse, Key, Label,
    Layout, Margin, Modifiers, Rect, Response, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui,
    UiBuilder, Vec2, pos2, vec2,
};
use xguitare_core::control::SILENCE_DB;
use xguitare_gui::model::{
    ClipLatch, Command, Direction, EAR_WIDTH, KnobKey, ModuleType, NO_SIGNAL, ParamRow, PeakHold,
    Rgb, Snapshot, Throttle, TunerReading, add_command, autoscroll_delta, cabinet_ir,
    cents_fraction, drop_index, in_tune, key_step, knob_fraction, knob_param_value, move_target,
    palette, param_rows, parse_typed_value, peak_db, peak_text, rack_channels, tuner_header_text,
    tuner_reading, tuner_toggle, unit_suffix,
};
use xguitare_gui::widgets::{self, KnobSize, colour};

const NO_IR: &str = "none";
const CABINET: &str = "cabinet";
const RAIL_WIDTH: i8 = 14;
const UNIT_GAP: f32 = 4.0;
pub const UNIT_MARGIN: i8 = 8;
const UNIT_SIDE_MARGIN: i8 = EAR_WIDTH as i8 + 14;
const UNIT_HEIGHT: f32 = 66.0;
const OPEN_UNIT_HEIGHT: f32 = 190.0;
const METER_BRIDGE_HEIGHT: f32 = 70.0;
const UNIT_NAME_WIDTH: f32 = 120.0;
const UNIT_NAME_SIZE: f32 = 18.0;
const SMALL_TEXT_SIZE: f32 = 13.0;
const LAMP_SLOT: f32 = 18.0;
const METER_LABEL_WIDTH: f32 = 52.0;
pub const METERS: [(&str, &str); 2] = [("Input", "input_peak_db"), ("Output", "output_peak_db")];
const CLEAR_CLIP_HINT: &str = "Click to clear the clip lamp";
const BUTTON_SIZE: Vec2 = Vec2::new(64.0, 36.0);
const MOVE_BUTTON_SIZE: Vec2 = Vec2::splat(36.0);
const MOVE_BUTTONS_WIDTH: f32 = 90.0;
const OPEN_CHEVRON: &str = "▼";
const CLOSED_CHEVRON: &str = "▶";
const DROP_LINE_WIDTH: f32 = 2.0;
const TUNER_LABEL: &str = "Tuner";
const ADD_LABEL: &str = "Add a Module";
const ADD_COMBO_WIDTH: f32 = 320.0;
const NOTE_SIZE: f32 = 96.0;
const NO_SIGNAL_SIZE: f32 = 40.0;
const TYPED_FIELD_WIDTH: f32 = 64.0;
const KNOB_KEYS: [(Key, KnobKey); 6] = [
    (Key::ArrowUp, KnobKey::Up),
    (Key::ArrowDown, KnobKey::Down),
    (Key::PageUp, KnobKey::PageUp),
    (Key::PageDown, KnobKey::PageDown),
    (Key::Home, KnobKey::Home),
    (Key::End, KnobKey::End),
];

pub struct Knobs<'a> {
    pub throttle: &'a mut Throttle,
    pub locals: &'a mut HashMap<String, f64>,
    pub revision: u64,
    pub now: Instant,
}

pub struct Tuner<'a> {
    pub open: &'a mut bool,
    pub muted_by_us: &'a mut bool,
    pub mute_default: bool,
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

pub struct RackView<'a, 'b> {
    pub knobs: &'a mut Knobs<'b>,
    pub tuner: &'a mut Tuner<'b>,
    pub clips: &'a mut [ClipLatch; 2],
    pub holds: &'a [PeakHold; 2],
    pub open_units: &'a mut BTreeSet<String>,
}

pub fn rack_view(ui: &mut Ui, snapshot: &Snapshot, view: &mut RackView) -> Vec<Command> {
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

pub fn muted_text(text: impl Into<String>) -> RichText {
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
                let held_db = snapshot.status.as_ref().map(|_| hold.held_db());
                ui.label(muted_text(peak_text(held_db, clip.is_clipped())));
            });
        }
    });
    if cleared {
        clips.iter_mut().for_each(ClipLatch::reset);
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

/// A unit dropped by its ears moves to the unit under the pointer; while it is dragged, a line marks where it will land, and the Rack scrolls when the pointer nears the top or bottom of the view.
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
    if let Some(y) = pointer_y
        && placed.iter().any(|(_, ears)| ears.dragged())
    {
        let view = ui.clip_rect();
        let delta = autoscroll_delta(y, view.top(), view.bottom());
        if delta != 0.0 {
            ui.scroll_with_delta(vec2(0.0, -delta));
            ui.ctx().request_repaint();
        }
    }
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

/// A double-click reset, a key on the focused knob and a typed value are final values, like a release. Ctrl+click or a right-click opens the typed-value field in place of the value.
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
        let field_id = knob.response.id.with("typed");
        let ctrl = ui.input(|input| input.modifiers.ctrl);
        if knob.response.secondary_clicked() || (knob.response.clicked() && ctrl) {
            let field = TypedField {
                text: value_number_text(shown, row),
                focused: false,
            };
            ui.data_mut(|data| data.insert_temp(field_id, field));
        }
        let keys = if knob.response.has_focus() {
            knob_keys(ui)
        } else {
            Vec::new()
        };
        let dragged = if knob.reset || knob.response.dragged() {
            knob_param_value(row, knob.fraction, knob.reset)
        } else {
            shown
        };
        let value = keys
            .iter()
            .fold(dragged, |value, &key| key_step(row, value, key));
        let typed = ui
            .vertical(|ui| {
                ui.label(muted_text(&row.param));
                value_field(ui, field_id, value, row)
            })
            .inner;
        let released = knob.reset || knob.response.drag_stopped() || !keys.is_empty();
        if let Some(value) = typed.or(released.then_some(value)) {
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

fn knob_keys(ui: &mut Ui) -> Vec<KnobKey> {
    ui.input_mut(|input| {
        KNOB_KEYS
            .iter()
            .flat_map(|&(key, knob_key)| {
                std::iter::repeat_n(knob_key, input.count_and_consume_key(Modifiers::NONE, key))
            })
            .collect()
    })
}

/// `focused` turns true once the field has asked for focus, a frame after it opens: the click that opened it would take the focus away again.
#[derive(Clone)]
struct TypedField {
    text: String,
    focused: bool,
}

/// The value label, or the open typed-value field; the field closes when it loses focus and returns its number only when Enter closed it.
fn value_field(ui: &mut Ui, id: Id, value: f64, row: &ParamRow) -> Option<f64> {
    let Some(mut field) = ui.data(|data| data.get_temp::<TypedField>(id)) else {
        ui.label(RichText::new(value_text(value, row)).strong());
        return None;
    };
    let edit = ui.add(
        TextEdit::singleline(&mut field.text)
            .id(id)
            .desired_width(TYPED_FIELD_WIDTH),
    );
    if field.focused && !edit.has_focus() {
        ui.data_mut(|data| data.remove::<TypedField>(id));
        let entered = ui.input(|input| input.key_pressed(Key::Enter));
        return entered.then(|| parse_typed_value(&field.text)).flatten();
    }
    if !field.focused && !ui.input(|input| input.pointer.any_click()) {
        edit.request_focus();
        field.focused = true;
    }
    ui.data_mut(|data| data.insert_temp(id, field));
    None
}

fn value_text(value: f64, row: &ParamRow) -> String {
    format!(
        "{}{}",
        value_number_text(value, row),
        unit_suffix(&row.unit)
    )
}

fn value_number_text(value: f64, row: &ParamRow) -> String {
    if row.integer {
        format!("{}", value.round())
    } else {
        format!("{value:.1}")
    }
}
