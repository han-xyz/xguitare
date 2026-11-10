use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use egui::{
    Align, Button, Color32, ComboBox, CursorIcon, FontSelection, Frame, Id, InnerResponse, Key,
    Label, Layout, Margin, Modifiers, Rect, Response, RichText, Role, ScrollArea, Sense, Stroke,
    TextEdit, TextWrapMode, Ui, UiBuilder, Vec2, WidgetInfo, WidgetText, pos2, vec2,
};
use xguitare_core::control::SILENCE_DB;

use crate::model::{
    CLOSED_KNOB_WIDTH, ClipLatch, Command, Direction, EAR_WIDTH, Feedback, KnobKey, NO_SIGNAL,
    ParamRow, PeakHold, Rgb, Snapshot, Throttle, TunerReading, autoscroll_delta, cabinet_ir,
    cents_fraction, closed_unit_rows, drop_index, format_value, in_tune, key_step, knob_fraction,
    knob_param_value, module_title, move_target, palette, param_label, param_rows, peak_db,
    peak_text, tuner_header_text, tuner_reading, typed_text, typed_value,
};
use crate::widgets::{self, KnobSize, colour};

const NO_IR: &str = "none";
const CABINET: &str = "cabinet";
const RAIL_WIDTH: i8 = 14;
const RACK_MAX_WIDTH: f32 = 1060.0;
const UNIT_GAP: f32 = 4.0;
pub const UNIT_MARGIN: i8 = 8;
const UNIT_SIDE_MARGIN: i8 = EAR_WIDTH as i8 + HEADER_GAP as i8;
const UNIT_HEIGHT: f32 = 64.0;
const OPEN_UNIT_HEIGHT: f32 = 190.0;
const HEADER_HEIGHT: f32 = UNIT_HEIGHT - 2.0 * UNIT_MARGIN as f32;
const METER_BRIDGE_HEIGHT: f32 = 70.0;
const UNIT_NAME_WIDTH: f32 = 120.0;
const UNIT_NAME_SIZE: f32 = 18.0;
const SMALL_TEXT_SIZE: f32 = 13.0;
const LAMP_SLOT: f32 = 18.0;
const METER_LABEL_WIDTH: f32 = 52.0;
const METER_ROW_HEIGHT: f32 = 16.0;
const METER_ROW_GAP: f32 = 8.0;
const METER_ITEM_GAP: f32 = 12.0;
const METER_TOP: f32 = 7.0;
pub const METERS: [(&str, &str); 2] = [("Input", "input_peak_db"), ("Output", "output_peak_db")];
const CLEAR_CLIP_HINT: &str = "Click to clear the clip lamp";
const MOVE_BUTTON_SIZE: Vec2 = Vec2::splat(36.0);
const MOVE_BUTTON_GAP: f32 = 8.0;
const MOVE_BUTTONS_WIDTH: f32 = 2.0 * MOVE_BUTTON_SIZE.x + MOVE_BUTTON_GAP;
const OPEN_CHEVRON: &str = "▼";
const CLOSED_CHEVRON: &str = "▶";
const DROP_LINE_WIDTH: f32 = 2.0;
const TUNER_LABEL: &str = "Tuner";
const METER_BRIDGE_LABEL: &str = "Meter bridge";
const NOTE_SIZE: f32 = 96.0;
const NO_SIGNAL_SIZE: f32 = 40.0;
const VALUE_FIELD_WIDTH: f32 = 64.0;
const STRIPE_WIDTH: f32 = 3.0;
const STRIPE_INSET: f32 = 10.0;
const HEADER_GAP: f32 = 10.0;
const LARGE_KNOB_COLUMN_WIDTH: f32 = 84.0;
const SMALL_KNOB_TOP: f32 = 4.0;
const FOCUS_PENDING: &str = "focus pending";
const CHEVRON_WIDTH: f32 = 20.0;
const KNOB_FIELD_GAP: f32 = 8.0;
/// The widths of a closed unit's header items other than its knobs, left to right: lamp, name, stripe, chevron, ↑/↓.
const HEADER_FIXED_ITEMS: [f32; 5] = [
    LAMP_SLOT,
    UNIT_NAME_WIDTH,
    STRIPE_WIDTH,
    CHEVRON_WIDTH,
    MOVE_BUTTONS_WIDTH,
];
const KNOB_KEYS: [(Key, KnobKey); 6] = [
    (Key::ArrowUp, KnobKey::Up),
    (Key::ArrowDown, KnobKey::Down),
    (Key::PageUp, KnobKey::PageUp),
    (Key::PageDown, KnobKey::PageDown),
    (Key::Home, KnobKey::Home),
    (Key::End, KnobKey::End),
];

/// `feedback` takes the footer message for a typed value that is no number.
pub struct Knobs<'a> {
    pub throttle: &'a mut Throttle,
    pub locals: &'a mut HashMap<String, f64>,
    pub feedback: &'a mut Feedback,
    pub revision: u64,
    pub now: Instant,
}

pub struct RackView<'a, 'b> {
    pub knobs: &'a mut Knobs<'b>,
    pub tuner_open: bool,
    pub clips: &'a mut [ClipLatch; 2],
    pub holds: &'a [PeakHold; 2],
    pub open_units: &'a mut BTreeSet<String>,
}

pub fn rack_view(ui: &mut Ui, snapshot: &Snapshot, view: &mut RackView) -> Vec<Command> {
    let mut commands = Vec::new();
    let modules = snapshot.params.as_ref().map(param_rows).unwrap_or_default();
    ScrollArea::vertical().show(ui, |ui| {
        let width = ui.available_width().min(RACK_MAX_WIDTH);
        let left = ui.cursor().left() + (ui.available_width() - width) / 2.0;
        let column = Rect::from_min_size(pos2(left, ui.cursor().top()), vec2(width, f32::INFINITY));
        ui.scope_builder(UiBuilder::new().max_rect(column), |ui| {
            Frame::new()
                .fill(colour(palette().rail))
                .stroke(Stroke::new(1.0, colour(palette().outline)))
                .inner_margin(Margin::symmetric(RAIL_WIDTH, 0))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = UNIT_GAP;
                    meter_bridge(ui, snapshot, view.clips, view.holds);
                    tuner_unit(ui, snapshot, view.tuner_open);
                    commands.extend(module_units(
                        ui,
                        snapshot,
                        &modules,
                        view.knobs,
                        view.open_units,
                    ));
                });
        });
    });
    commands
}

/// A unit exactly `height` tall whatever its content, which is clipped to it; `label` names it in the accessibility tree.
fn rack_unit<R>(
    ui: &mut Ui,
    label: &str,
    fill: Rgb,
    height: f32,
    content: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let colours = palette();
    let rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), height));
    let mut unit_ui = ui.new_child(UiBuilder::new().id_salt(label).max_rect(rect));
    unit_ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    let inner = Frame::new()
        .fill(colour(fill))
        .inner_margin(Margin::symmetric(UNIT_SIDE_MARGIN, UNIT_MARGIN))
        .show(&mut unit_ui, |ui| {
            ui.set_min_size(ui.available_size());
            content(ui)
        })
        .inner;
    let response = ui.allocate_rect(rect, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Group, true, label));
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
    InnerResponse::new(inner, response)
}

/// The unit's top row, items centred in it: one closed unit's content tall, or all of a closed unit's knob rows.
fn unit_header<R>(
    ui: &mut Ui,
    sense: Sense,
    height: f32,
    content: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), height));
    let header = UiBuilder::new()
        .max_rect(rect)
        .layout(Layout::left_to_right(Align::Center))
        .sense(sense);
    ui.scope_builder(header, content)
}

/// What a closed unit in `ui` leaves its knobs: the unit's width less its margins, the fixed header items and one header gap per item.
fn closed_knobs_width(ui: &Ui) -> f32 {
    let fixed: f32 = HEADER_FIXED_ITEMS
        .iter()
        .map(|width| width + HEADER_GAP)
        .sum();
    (ui.available_width() - 2.0 * f32::from(UNIT_SIDE_MARGIN) - fixed).max(0.0)
}

fn unit_lamp(ui: &mut Ui, lit_colour: Color32, lit: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(LAMP_SLOT), Sense::hover());
    widgets::lamp(ui.painter(), rect.center(), lit_colour, lit);
}

/// The unit's name, its `(type)` after it in small text; the name is truncated first so that the type stays.
fn unit_name(ui: &mut Ui, name: &str, type_name: Option<&str>) {
    let name = Label::new(RichText::new(name).strong().size(UNIT_NAME_SIZE)).truncate();
    let Some(type_name) = type_name else {
        fixed_width_label(ui, UNIT_NAME_WIDTH, name);
        return;
    };
    let type_part = WidgetText::from(muted_text(format!(" ({type_name})"))).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        UNIT_NAME_WIDTH,
        FontSelection::Default,
    );
    let slot = vec2(UNIT_NAME_WIDTH, ui.available_height());
    ui.allocate_ui_with_layout(slot, Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(slot);
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.scope(|ui| {
            ui.set_max_width(UNIT_NAME_WIDTH - type_part.size().x);
            ui.add(name);
        });
        ui.add(Label::new(type_part));
    });
}

/// `label` at the left of a slot `width` wide, centred in the row's height.
fn fixed_width_label(ui: &mut Ui, width: f32, label: Label) {
    let slot = vec2(width, ui.available_height());
    ui.allocate_ui_with_layout(slot, Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(slot);
        ui.add(label);
    });
}

fn unit_stripe(ui: &mut Ui) {
    let size = vec2(STRIPE_WIDTH, ui.available_height() - 2.0 * STRIPE_INSET);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, 0, colour(palette().edge_light));
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
    rack_unit(
        ui,
        METER_BRIDGE_LABEL,
        palette().meter_bridge,
        METER_BRIDGE_HEIGHT,
        |ui| {
            ui.spacing_mut().item_spacing = vec2(METER_ITEM_GAP, METER_ROW_GAP);
            ui.add_space(METER_TOP);
            for (((label, key), clip), hold) in METERS.iter().zip(clips.iter()).zip(holds) {
                let row = vec2(ui.available_width(), METER_ROW_HEIGHT);
                ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
                    fixed_width_label(ui, METER_LABEL_WIDTH, Label::new(muted_text(*label)));
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
        },
    );
    if cleared {
        clips.iter_mut().for_each(ClipLatch::reset);
    }
}

/// The Tuner's readout; the left panel's Tuner toggle opens it to the Tuner view.
fn tuner_unit(ui: &mut Ui, snapshot: &Snapshot, open: bool) {
    let (fill, height) = if open {
        (palette().unit_open, OPEN_UNIT_HEIGHT)
    } else {
        (palette().unit, UNIT_HEIGHT)
    };
    let reading = snapshot.tuner.as_ref().and_then(tuner_reading);
    rack_unit(ui, "Tuner unit", fill, height, |ui| {
        unit_header(ui, Sense::hover(), HEADER_HEIGHT, |ui| {
            ui.spacing_mut().item_spacing.x = HEADER_GAP;
            unit_lamp(ui, colour(palette().green), open);
            unit_name(ui, TUNER_LABEL, None);
            unit_stripe(ui);
            ui.label(RichText::new(tuner_header_text(reading.as_ref())).strong());
        });
        if open {
            tuner_view(ui, reading.as_ref());
        }
    });
}

fn tuner_view(ui: &mut Ui, reading: Option<&TunerReading>) {
    ui.horizontal(|ui| match reading {
        Some(reading) => {
            let colour = widgets::tune_colour(in_tune(reading.cents));
            ui.label(RichText::new(&reading.note).size(NOTE_SIZE).color(colour));
            ui.vertical(|ui| {
                widgets::cents_arc(ui, Some(cents_fraction(reading.cents)), colour);
                ui.label(RichText::new(format!("{:.1} Hz", reading.hz)).color(colour));
            });
        }
        None => {
            ui.label(RichText::new(NO_SIGNAL).size(NO_SIGNAL_SIZE));
            widgets::cents_arc(ui, None, Color32::GRAY);
        }
    });
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
    let knobs_width = closed_knobs_width(ui);
    let rows = closed_unit_rows(unit.rows.len(), knobs_width) as f32;
    let (fill, height, header_height) = if open {
        (palette().unit_open, OPEN_UNIT_HEIGHT, HEADER_HEIGHT)
    } else {
        let height = UNIT_HEIGHT * rows;
        (
            palette().unit,
            height,
            height - 2.0 * f32::from(UNIT_MARGIN),
        )
    };
    let mut toggled = false;
    let inner = rack_unit(ui, &format!("{module} unit"), fill, height, |ui| {
        let mut commands = Vec::new();
        let header = unit_header(ui, Sense::click(), header_height, |ui| {
            ui.spacing_mut().item_spacing.x = HEADER_GAP;
            unit_lamp(ui, accent, true);
            let (name, type_part) = module_title(module, type_name);
            unit_name(ui, &name, type_part);
            unit_stripe(ui);
            let chevron = if open { OPEN_CHEVRON } else { CLOSED_CHEVRON };
            let chevron = Button::new(chevron).small().frame(false);
            toggled |= ui
                .add_sized(vec2(CHEVRON_WIDTH, CHEVRON_WIDTH), chevron)
                .clicked();
            let rows = Layout::left_to_right(Align::Min).with_main_wrap(true);
            ui.allocate_ui_with_layout(vec2(knobs_width, header_height), rows, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 2.0 * f32::from(UNIT_MARGIN));
                if !open {
                    commands.extend(param_knobs(ui, unit.rows, KnobSize::Small, accent, knobs));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = MOVE_BUTTON_GAP;
                commands.extend(move_buttons(ui, module, unit.place));
            });
        });
        toggled |= header.response.clicked();
        if open {
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                commands.extend(param_knobs(ui, unit.rows, KnobSize::Large, accent, knobs));
                ui.with_layout(Layout::top_down(Align::Max), |ui| {
                    if let Some(ir_choice) = &unit.ir_choice {
                        commands.extend(ir_row(ui, module, ir_choice));
                    }
                    if ui
                        .add(
                            widgets::warning_button("Remove")
                                .min_size(vec2(0.0, MOVE_BUTTON_SIZE.y)),
                        )
                        .clicked()
                    {
                        commands.push(Command::RackRemove {
                            id: module.to_string(),
                        });
                    }
                });
            });
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
        let label = ui.label("IR");
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
        if combo.response.labelled_by(label.id).clicked() {
            commands.push(Command::RefreshIrs);
        }
    });
    commands
}

/// Added right to left, so that they read ↑ ↓ at the right of the unit.
fn move_buttons(ui: &mut Ui, module: &str, (index, len): (usize, usize)) -> Option<Command> {
    let mut clicked = None;
    for (text, direction) in [("↓", Direction::Down), ("↑", Direction::Up)] {
        let to = move_target(index, direction, len);
        if ui
            .add_enabled(
                to.is_some(),
                widgets::outlined_button(text).min_size(MOVE_BUTTON_SIZE),
            )
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

/// A double-click reset, a key on the focused knob and a typed value are final values, like a release. Ctrl+click or a right-click focuses the knob's value field.
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
    let field_id = Id::unique(("value", &row.address));
    let label = muted_text(param_label(&row.param));
    let knob_and_field = |ui: &mut Ui, knobs: &mut Knobs| {
        let knob = widgets::rack_knob(ui, size, fraction, accent);
        let ctrl = ui.input(|input| input.modifiers.ctrl);
        if knob.response.secondary_clicked() || (knob.response.clicked() && ctrl) {
            ui.data_mut(|data| data.insert_temp(field_id_pending(field_id), true));
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
        let released = knob.reset || knob.response.drag_stopped() || !keys.is_empty();
        (value, released, knobs.now)
    };
    let (value, released, typed) = match size {
        KnobSize::Small => {
            let group = Layout::left_to_right(Align::Min);
            ui.allocate_ui_with_layout(vec2(CLOSED_KNOB_WIDTH, HEADER_HEIGHT), group, |ui| {
                ui.set_min_size(vec2(CLOSED_KNOB_WIDTH, HEADER_HEIGHT));
                ui.spacing_mut().item_spacing = vec2(KNOB_FIELD_GAP, UNIT_GAP);
                let (value, released, now) = ui
                    .vertical(|ui| {
                        ui.add_space(SMALL_KNOB_TOP);
                        knob_and_field(ui, knobs)
                    })
                    .inner;
                let typed = ui
                    .vertical(|ui| {
                        let label = ui.label(label);
                        let field = (field_id, VALUE_FIELD_WIDTH);
                        value_field(ui, field, label.id, row, value, (knobs.feedback, now))
                    })
                    .inner;
                (value, released, typed)
            })
            .inner
        }
        KnobSize::Large => {
            let column = Layout::top_down(Align::Center);
            ui.allocate_ui_with_layout(vec2(LARGE_KNOB_COLUMN_WIDTH, 0.0), column, |ui| {
                let label = ui.label(label);
                let (value, released, now) = knob_and_field(ui, knobs);
                let field = (field_id, KnobSize::Large.diameter());
                let typed = value_field(ui, field, label.id, row, value, (knobs.feedback, now));
                (value, released, typed)
            })
            .inner
        }
    };
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
}

fn field_id_pending(field_id: Id) -> Id {
    field_id.with(FOCUS_PENDING)
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

/// Shows the formatted value; while focused it holds the bare number to edit. A focus asked for by a click on the knob waits for a frame without a click, which would take the focus away again. Enter returns the typed number; Esc or a click elsewhere drops the edit, and text that is no number also puts a footer message.
fn value_field(
    ui: &mut Ui,
    (id, width): (Id, f32),
    label: Id,
    row: &ParamRow,
    value: f64,
    (feedback, now): (&mut Feedback, Instant),
) -> Option<f64> {
    let editing = ui.data_mut(|data| data.remove_temp::<String>(id));
    let mut text = editing.clone().unwrap_or_else(|| format_value(row, value));
    let edit = ui
        .add(TextEdit::singleline(&mut text).id(id).desired_width(width))
        .labelled_by(label);
    let pending = field_id_pending(id);
    let clicking = ui.input(|input| input.pointer.any_pressed() || input.pointer.any_click());
    if !clicking
        && ui
            .data_mut(|data| data.remove_temp::<bool>(pending))
            .is_some()
    {
        edit.request_focus();
    }
    if edit.has_focus() {
        let text = if editing.is_some() {
            text
        } else {
            typed_text(row, value)
        };
        ui.data_mut(|data| data.insert_temp(id, text));
        return None;
    }
    let entered = edit.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    if !entered {
        return None;
    }
    typed_value(&text)
        .map_err(|message| feedback.show(message, now))
        .ok()
}
