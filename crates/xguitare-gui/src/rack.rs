use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use egui::{
    Align, Align2, Button, Color32, ComboBox, CursorIcon, DragAndDrop, FontId, FontSelection,
    Frame, Id, InnerResponse, Key, Label, Layout, Margin, Modifiers, Painter, Rect, Response,
    RichText, Role, ScrollArea, Sense, Stroke, TextEdit, TextWrapMode, Ui, UiBuilder, Vec2,
    WidgetInfo, WidgetText, pos2, vec2,
};
use xguitare_core::control::SILENCE_DB;

use crate::eq_model::{
    EQ, EQ_AXIS_LABEL_HEIGHT, EQ_BAND_TITLE_HEIGHT, EQ_CURVE_POINTS, EQ_MINI_HEIGHT,
    EQ_MINI_POINTS, EQ_MINI_WIDTH, EQ_PLOT_HEIGHT, EQ_SECTION_GAP, EqBand, EqDot, eq_bands,
    eq_curve, eq_curve_name, eq_dot_of_param, eq_dots, eq_open_height,
};
use crate::eq_view::{EqPainters, PlotLines, paint_mini, paint_plot};
use crate::model::{
    CLOSED_KNOB_WIDTH, ChipClick, ChoiceKey, ClipLatch, Command, Direction, Dragged, EAR_WIDTH,
    Feedback, KnobKey, NO_SIGNAL, OPEN_KNOB_PITCH, OPEN_UNIT_HEIGHT, ParamRow, PeakHold,
    ScreenRect, Snapshot, Throttle, TunerReading, autoscroll_delta, bypassed, cabinet_ir,
    cents_fraction, chip_click, choice_buttons, choice_step, choice_text, closed_unit_rows,
    drag_outcome, drop_slot, format_value, in_tune, key_step, knob_fraction, knob_param_value,
    module_title, move_target, mute_banner, open_knob_rows, open_unit_height, param_label,
    param_rows, peak_db, peak_text, tuner_header_text, tuner_reading, typed_text, typed_value,
    unit_style,
};
use crate::project_model::sample_rate;
use crate::theme::{self, Rgb, TEXT_DISPLAY, colour, palette};
use crate::widgets::{self, KnobSize};

const NO_IR: &str = "none";
const CABINET: &str = "cabinet";
const RAIL_WIDTH: i8 = 14;
const RACK_MAX_WIDTH: f32 = 1060.0;
const UNIT_GAP: f32 = 4.0;
pub const UNIT_MARGIN: i8 = 8;
const UNIT_SIDE_MARGIN: i8 = EAR_WIDTH as i8 + HEADER_GAP as i8;
const UNIT_HEIGHT: f32 = 64.0;
const HEADER_HEIGHT: f32 = UNIT_HEIGHT - 2.0 * UNIT_MARGIN as f32;
const METER_BRIDGE_HEIGHT: f32 = 70.0;
const UNIT_NAME_WIDTH: f32 = 120.0;
const LAMP_SLOT: f32 = 18.0;
const BANNER_HEIGHT: f32 = 40.0;
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
const DROP_LINE_WIDTH: f32 = 3.0;
const GRIP: &str = "⠿";
const TUNER_LABEL: &str = "Tuner";
const METER_BRIDGE_LABEL: &str = "Meter bridge";
const VALUE_FIELD_WIDTH: f32 = 64.0;
const STRIPE_WIDTH: f32 = 3.0;
const STRIPE_INSET: f32 = 10.0;
const HEADER_GAP: f32 = 10.0;
const LARGE_KNOB_COLUMN_WIDTH: f32 = 84.0;
const SMALL_KNOB_TOP: f32 = 4.0;
const FOCUS_PENDING: &str = "focus pending";
const CHEVRON_WIDTH: f32 = 20.0;
const KNOB_FIELD_GAP: f32 = 8.0;
/// The widths of a closed unit's header items other than its knobs, left to right: On/Off switch with its LED, name, stripe, chevron, ↑/↓.
const HEADER_FIXED_ITEMS: [f32; 5] = [
    widgets::FOOTSWITCH_SIZE.x,
    UNIT_NAME_WIDTH,
    STRIPE_WIDTH,
    CHEVRON_WIDTH,
    MOVE_BUTTONS_WIDTH,
];
const CHIP_SIZE: Vec2 = Vec2::new(56.0, 24.0);
const CHOICE_KEYS: [(Key, ChoiceKey); 4] = [
    (Key::ArrowLeft, ChoiceKey::Previous),
    (Key::ArrowUp, ChoiceKey::Previous),
    (Key::ArrowRight, ChoiceKey::Next),
    (Key::ArrowDown, ChoiceKey::Next),
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
    /// Whether the Tuner view turned Mute on, so that closing it turns Mute off.
    pub muted_by_tuner: bool,
    pub clips: &'a mut [ClipLatch; 2],
    pub holds: &'a [PeakHold; 2],
    pub open_units: &'a mut BTreeSet<String>,
}

pub fn rack_view(ui: &mut Ui, snapshot: &Snapshot, view: &mut RackView) -> Vec<Command> {
    let mut commands = Vec::new();
    let modules = snapshot.params.as_ref().map(param_rows).unwrap_or_default();
    if let Some(text) = mute_banner(snapshot.status.as_ref(), view.muted_by_tuner) {
        mute_banner_strip(ui, &text);
    }
    ScrollArea::vertical().show(ui, |ui| {
        let width = ui.available_width().min(RACK_MAX_WIDTH);
        let left = ui.cursor().left() + (ui.available_width() - width) / 2.0;
        let column = Rect::from_min_size(pos2(left, ui.cursor().top()), vec2(width, f32::INFINITY));
        ui.scope_builder(UiBuilder::new().max_rect(column), |ui| {
            let rack = Frame::new()
                .fill(colour(palette().case))
                .stroke(Stroke::new(1.0, colour(palette().hover)))
                .inner_margin(Margin::symmetric(RAIL_WIDTH, 0))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = UNIT_GAP;
                    meter_bridge(ui, snapshot, view.clips, view.holds);
                    tuner_unit(ui, snapshot, view.tuner_open);
                    let (unit_commands, placed) =
                        module_units(ui, snapshot, &modules, view.knobs, view.open_units);
                    commands.extend(unit_commands);
                    placed
                });
            commands.extend(drag_and_drop(ui, &modules, &rack.inner));
        });
    });
    commands
}

/// Red, as wide as the rack and above it, outside its scroll area so that it stays in view.
fn mute_banner_strip(ui: &mut Ui, text: &str) {
    let width = ui.available_width().min(RACK_MAX_WIDTH);
    let left = ui.cursor().left() + (ui.available_width() - width) / 2.0;
    let rect = Rect::from_min_size(pos2(left, ui.cursor().top()), vec2(width, BANNER_HEIGHT));
    let response = ui.allocate_rect(rect, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Alert, true, text));
    let painter = ui.painter();
    painter.rect_filled(rect, theme::RADIUS_CONTROL, colour(palette().danger));
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme::heading(),
        colour(palette().on_accent),
    );
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
        Stroke::new(1.0, colour(colours.hover)),
    );
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, colour(colours.well)),
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

/// What an open unit in `ui` leaves its knob rows: the unit's width less its margins and the column of ↑/↓ and Remove at its right.
fn open_knobs_width(ui: &Ui) -> f32 {
    let side_column = MOVE_BUTTONS_WIDTH + HEADER_GAP;
    (ui.available_width() - 2.0 * f32::from(UNIT_SIDE_MARGIN) - side_column).max(0.0)
}

fn unit_lamp(ui: &mut Ui, lit_colour: Color32, lit: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(LAMP_SLOT), Sense::hover());
    widgets::lamp(ui.painter(), rect.center(), lit_colour, lit);
}

/// The unit's name, its `(type)` after it in small text; the name is truncated first so that the type stays.
fn unit_name(ui: &mut Ui, name: &str, type_name: Option<&str>) {
    let name = Label::new(RichText::new(name).font(theme::unit_name())).truncate();
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
    ui.painter().rect_filled(rect, 0, colour(palette().hover));
}

pub fn muted_text(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .font(theme::body())
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
        palette().panel,
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
        (palette().panel_open, OPEN_UNIT_HEIGHT)
    } else {
        (palette().panel, UNIT_HEIGHT)
    };
    let reading = snapshot.tuner.as_ref().and_then(tuner_reading);
    rack_unit(ui, "Tuner unit", fill, height, |ui| {
        unit_header(ui, Sense::hover(), HEADER_HEIGHT, |ui| {
            ui.spacing_mut().item_spacing.x = HEADER_GAP;
            unit_lamp(ui, colour(palette().go), open);
            unit_name(ui, TUNER_LABEL, None);
            unit_stripe(ui);
            ui.label(RichText::new(tuner_header_text(reading.as_ref())).font(theme::note()));
        });
        if open {
            tuner_view(ui, reading.as_ref());
        }
    });
}

fn tuner_view(ui: &mut Ui, reading: Option<&TunerReading>) {
    ui.horizontal(|ui| {
        if let Some(reading) = reading {
            let colour = widgets::tune_colour(in_tune(reading.cents));
            ui.label(
                RichText::new(&reading.note)
                    .font(FontId::monospace(TEXT_DISPLAY))
                    .color(colour),
            );
            ui.vertical(|ui| {
                widgets::cents_arc(ui, Some(cents_fraction(reading.cents)), colour);
                ui.label(RichText::new(format!("{:.1} Hz", reading.hz)).color(colour));
            });
        } else {
            ui.label(RichText::new(NO_SIGNAL).font(theme::heading()));
            widgets::cents_arc(ui, None, colour(palette().muted));
        }
    });
}

/// Each unit's commands, and its rect with the drag response of its header and ears.
fn module_units(
    ui: &mut Ui,
    snapshot: &Snapshot,
    modules: &[(String, Vec<ParamRow>)],
    knobs: &mut Knobs,
    open_units: &mut BTreeSet<String>,
) -> (Vec<Command>, Vec<(Rect, Response)>) {
    let mut commands = Vec::new();
    let mut placed = Vec::with_capacity(modules.len());
    for (index, (module, rows)) in modules.iter().enumerate() {
        let is_cabinet = rows.first().is_some_and(|row| row.module_type == CABINET);
        let unit = ModuleUnit {
            module,
            rows,
            bypassed: snapshot
                .params
                .as_ref()
                .is_some_and(|params| bypassed(params, module)),
            place: (index, modules.len()),
            ir_choice: snapshot
                .params
                .as_ref()
                .filter(|_| is_cabinet)
                .map(|params| IrChoice {
                    current: cabinet_ir(params, module),
                    irs: &snapshot.irs,
                }),
            sample_rate: snapshot
                .status
                .as_ref()
                .map(|status| sample_rate(status) as f32),
        };
        let (unit_commands, rect, handle) = module_unit(ui, &unit, knobs, open_units);
        commands.extend(unit_commands);
        placed.push((rect, handle));
    }
    (commands, placed)
}

/// A unit dragged by its header or ears, or a Module tile dragged from the left panel: while the pointer is over the Rack's view, a line in the dragged type's colour marks the gap it drops into and the Rack scrolls near the top or bottom of the view; the release sends its [`drag_outcome`].
fn drag_and_drop(
    ui: &mut Ui,
    modules: &[(String, Vec<ParamRow>)],
    placed: &[(Rect, Response)],
) -> Option<Command> {
    let area = ui.clip_rect();
    let dragged = placed
        .iter()
        .position(|(_, handle)| handle.dragged() || handle.drag_stopped())
        .map(Dragged::Unit)
        .or_else(|| DragAndDrop::payload::<Dragged>(ui.ctx()).map(|tile| (*tile).clone()))?;
    let (type_name, released) = match &dragged {
        Dragged::Unit(index) => (
            modules[*index]
                .1
                .first()
                .map_or("", |row| row.module_type.as_str()),
            placed[*index].1.drag_stopped(),
        ),
        Dragged::Tile(type_name) => (
            type_name.as_str(),
            ui.input(|input| input.pointer.any_released()),
        ),
    };
    let edges: Vec<f32> = placed
        .iter()
        .map(|(rect, _)| rect.top())
        .chain(placed.last().map(|(rect, _)| rect.bottom()))
        .collect();
    let pointer = ui.input(|input| input.pointer.latest_pos());
    if let Some(pos) = pointer.filter(|pos| area.contains(*pos)) {
        let delta = autoscroll_delta(pos.y, area.top(), area.bottom());
        if delta != 0.0 {
            ui.scroll_with_delta(vec2(0.0, -delta));
            ui.ctx().request_repaint();
        }
        insert_line(
            ui,
            placed,
            drop_slot(pos.y, &edges),
            widgets::accent(type_name),
        );
    }
    if !released {
        return None;
    }
    let ids: Vec<String> = modules.iter().map(|(id, _)| id.clone()).collect();
    drag_outcome(
        &dragged,
        pointer.map(|pos| (pos.x, pos.y)),
        &screen_rect(ui.ctx().content_rect()),
        &screen_rect(area),
        &edges,
        &ids,
    )
}

fn screen_rect(rect: Rect) -> ScreenRect {
    ScreenRect {
        left: rect.left(),
        top: rect.top(),
        right: rect.right(),
        bottom: rect.bottom(),
    }
}

/// In the middle of the gap above the unit at `slot`, or below the last unit.
fn insert_line(ui: &Ui, placed: &[(Rect, Response)], slot: usize, accent: Color32) {
    let half_gap = UNIT_GAP / 2.0;
    let y = match placed.get(slot) {
        Some((rect, _)) => rect.top() - half_gap,
        None => match placed.last() {
            Some((rect, _)) => rect.bottom() + half_gap,
            None => return,
        },
    };
    let x_range = placed[0].0.x_range();
    ui.painter()
        .hline(x_range, y, Stroke::new(DROP_LINE_WIDTH, accent));
}

struct ModuleUnit<'a> {
    module: &'a str,
    rows: &'a [ParamRow],
    bypassed: bool,
    place: (usize, usize),
    ir_choice: Option<IrChoice<'a>>,
    /// The Daemon's rate; `None` while it is disconnected.
    sample_rate: Option<f32>,
}

/// The unit's commands, its rect, and the drag response of its header and its two ears.
#[allow(
    clippy::too_many_lines,
    reason = "one unit laid out top to bottom in drawing order"
)]
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
    let style = unit_style(unit.bypassed);
    let is_eq = type_name == EQ;
    let knobs_width = closed_knobs_width(ui);
    let rows = if is_eq {
        1.0
    } else {
        closed_unit_rows(unit.rows.len(), knobs_width) as f32
    };
    let open_width = open_knobs_width(ui);
    let open_rows = open_knob_rows(type_name, unit.rows.len(), open_width);
    let open_height = if is_eq {
        eq_open_height(&eq_bands(unit.rows))
    } else {
        open_unit_height(open_rows.len())
    };
    let (fill, height, header_height) = if open {
        (palette().panel_open, open_height, HEADER_HEIGHT)
    } else {
        let height = UNIT_HEIGHT * rows;
        (
            palette().panel,
            height,
            height - 2.0 * f32::from(UNIT_MARGIN),
        )
    };
    let mut toggled = false;
    let mut open_asked = false;
    let mut header_drag = None;
    let inner = rack_unit(ui, &format!("{module} unit"), fill, height, |ui| {
        let mut commands = Vec::new();
        let header = unit_header(ui, Sense::click_and_drag(), header_height, |ui| {
            ui.spacing_mut().item_spacing.x = HEADER_GAP;
            ui.style_mut().interaction.selectable_labels = false;
            if widgets::footswitch(ui, style.led_lit, accent).clicked() {
                commands.push(Command::Bypass {
                    id: module.to_string(),
                    on: !unit.bypassed,
                });
            }
            let (name, type_part) = module_title(module, type_name);
            ui.scope(|ui| {
                ui.multiply_opacity(style.opacity);
                unit_name(ui, &name, type_part);
            });
            unit_stripe(ui);
            let chevron = if open { OPEN_CHEVRON } else { CLOSED_CHEVRON };
            let chevron = Button::new(chevron).small().frame(false);
            toggled |= ui
                .add_sized(vec2(CHEVRON_WIDTH, CHEVRON_WIDTH), chevron)
                .clicked();
            let rows = Layout::left_to_right(Align::Min).with_main_wrap(true);
            ui.allocate_ui_with_layout(vec2(knobs_width, header_height), rows, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 2.0 * f32::from(UNIT_MARGIN));
                ui.multiply_opacity(style.opacity);
                if !open && is_eq {
                    eq_mini(ui, unit, accent, knobs);
                } else if !open {
                    let (closed_commands, asked) = closed_params(ui, unit.rows, accent, knobs);
                    commands.extend(closed_commands);
                    open_asked = asked;
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = MOVE_BUTTON_GAP;
                commands.extend(move_buttons(ui, module, unit.place));
            });
        });
        toggled |= header.response.clicked();
        grip(ui, header.response.rect);
        header_drag = Some(header.response);
        if open {
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.scope(|ui| {
                    ui.multiply_opacity(style.opacity);
                    if is_eq {
                        commands.extend(eq_editor(ui, unit, open_width, accent, knobs));
                    } else {
                        commands.extend(open_params(ui, unit.rows, &open_rows, accent, knobs));
                    }
                });
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
    if (toggled || open_asked) && !open_units.remove(module) {
        open_units.insert(module.to_string());
    }
    let rect = inner.response.rect;
    let side = f32::from(UNIT_SIDE_MARGIN);
    let side_size = vec2(side, rect.height());
    let handle = [
        Rect::from_min_size(rect.min, side_size),
        Rect::from_min_size(pos2(rect.right() - side, rect.top()), side_size),
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
    .chain(header_drag)
    .reduce(|left, right| left.union(right))
    .expect("a unit has two ears")
    .on_hover_cursor(CursorIcon::Grab);
    if handle.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    (inner.inner, rect, handle)
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

/// In the gap between the left ear and the header, which drags with the ears.
fn grip(ui: &Ui, header: Rect) {
    ui.painter().text(
        pos2(header.left() - HEADER_GAP / 2.0, header.center().y),
        Align2::CENTER_CENTER,
        GRIP,
        theme::body(),
        colour(palette().text),
    );
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

/// Small knobs, and a chip for each choice Parameter; also whether a chip asked to open the unit.
fn closed_params(
    ui: &mut Ui,
    rows: &[ParamRow],
    accent: Color32,
    knobs: &mut Knobs,
) -> (Vec<Command>, bool) {
    let mut open_asked = false;
    let commands = rows
        .iter()
        .filter_map(|row| {
            if row.labels.is_empty() {
                return param_knob(ui, row, KnobSize::Small, accent, knobs);
            }
            let (command, asked) = choice_chip(ui, row, accent, knobs);
            open_asked |= asked;
            command
        })
        .collect();
    (commands, open_asked)
}

/// Large knobs and choice columns in rows of `per_row` counts.
fn open_params(
    ui: &mut Ui,
    rows: &[ParamRow],
    per_row: &[usize],
    accent: Color32,
    knobs: &mut Knobs,
) -> Vec<Command> {
    let mut commands = Vec::new();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = f32::from(UNIT_MARGIN);
        let mut rest = rows;
        for &count in per_row {
            let (row_params, after) = rest.split_at(count.min(rest.len()));
            rest = after;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = OPEN_KNOB_PITCH - LARGE_KNOB_COLUMN_WIDTH;
                for row in row_params {
                    let command = if row.labels.is_empty() {
                        param_knob(ui, row, KnobSize::Large, accent, knobs)
                    } else {
                        choice_column(ui, row, accent, knobs)
                    };
                    commands.extend(command);
                }
            });
        }
    });
    commands
}

/// The 0 dB line of a bypassed unit's eq plot stays at full strength while the rest dims.
fn eq_painters(ui: &Ui) -> (Painter, Painter) {
    let painter = ui.painter().clone();
    let mut zero_line = painter.clone();
    zero_line.set_opacity(1.0);
    (painter, zero_line)
}

/// A closed eq's curve in place of its knobs; a click on it opens the unit like a click on the header.
fn eq_mini(ui: &mut Ui, unit: &ModuleUnit, accent: Color32, knobs: &Knobs) {
    let shown: Vec<ParamRow> = unit.rows.iter().map(|row| shown_row(row, knobs)).collect();
    let (slot, _) = ui.allocate_exact_size(vec2(EQ_MINI_WIDTH, HEADER_HEIGHT), Sense::hover());
    let rect = Rect::from_center_size(slot.center(), vec2(EQ_MINI_WIDTH, EQ_MINI_HEIGHT));
    let response = ui.interact(rect, Id::unique(("eq mini", unit.module)), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, eq_curve_name(&shown)));
    let curve = unit
        .sample_rate
        .map(|rate| eq_curve(&shown, rate, EQ_MINI_POINTS));
    let (painter, zero_line) = eq_painters(ui);
    let painters = EqPainters {
        painter: &painter,
        zero_line: &zero_line,
    };
    paint_mini(&painters, rect, curve.as_deref(), accent);
}

/// An open eq: the response curve over four band columns of small knobs; the dot of the band whose knob is hovered, dragged or focused is ringed.
fn eq_editor(
    ui: &mut Ui,
    unit: &ModuleUnit,
    width: f32,
    accent: Color32,
    knobs: &mut Knobs,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut active = None;
    let area = ui
        .vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(EQ_SECTION_GAP);
            let plot_size = vec2(width, EQ_PLOT_HEIGHT + EQ_AXIS_LABEL_HEIGHT);
            let (area, _) = ui.allocate_exact_size(plot_size, Sense::hover());
            ui.add_space(EQ_SECTION_GAP);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for band in eq_bands(unit.rows) {
                    let (band_commands, in_use) =
                        eq_band_column(ui, unit.rows, &band, accent, knobs);
                    commands.extend(band_commands);
                    active = active.or(in_use);
                }
            });
            area
        })
        .inner;
    let shown: Vec<ParamRow> = unit.rows.iter().map(|row| shown_row(row, knobs)).collect();
    let plot = ui.interact(area, Id::unique(("eq plot", unit.module)), Sense::hover());
    plot.widget_info(|| WidgetInfo::labeled(Role::Image, true, eq_curve_name(&shown)));
    let curve = unit.sample_rate.map(|rate| {
        (
            eq_curve(&shown, rate, EQ_CURVE_POINTS),
            eq_dots(&shown, rate, active),
        )
    });
    let (painter, zero_line) = eq_painters(ui);
    let painters = EqPainters {
        painter: &painter,
        zero_line: &zero_line,
    };
    let lines = curve
        .as_ref()
        .map(|(curve, dots)| PlotLines { curve, dots });
    paint_plot(&painters, area, lines, accent);
    commands
}

/// A band's title over its small knobs; also the band's dot when one of its knobs is in use.
fn eq_band_column(
    ui: &mut Ui,
    rows: &[ParamRow],
    band: &EqBand,
    accent: Color32,
    knobs: &mut Knobs,
) -> (Vec<Command>, Option<EqDot>) {
    let mut commands = Vec::new();
    let mut active = None;
    ui.vertical(|ui| {
        ui.set_width(CLOSED_KNOB_WIDTH);
        ui.spacing_mut().item_spacing.y = UNIT_GAP;
        let (title, _) = ui.allocate_exact_size(
            vec2(CLOSED_KNOB_WIDTH, EQ_BAND_TITLE_HEIGHT),
            Sense::hover(),
        );
        let colours = palette();
        ui.painter().text(
            title.left_center(),
            Align2::LEFT_CENTER,
            band.title,
            theme::body(),
            colour(colours.text),
        );
        ui.painter().hline(
            title.left()..=title.right() - KNOB_FIELD_GAP,
            title.bottom(),
            Stroke::new(1.0, colour(colours.hover)),
        );
        for knob in &band.knobs {
            let row = &rows[knob.index];
            let (command, in_use) = param_knob_named(
                ui,
                row,
                (knob.label, knob.name),
                KnobSize::Small,
                accent,
                knobs,
            );
            commands.extend(command);
            if in_use {
                active = eq_dot_of_param(&row.param);
            }
        }
    });
    (commands, active)
}

/// The row as shown: its value, or the one sent last while the Daemon has not answered it yet.
fn shown_row(row: &ParamRow, knobs: &Knobs) -> ParamRow {
    let held = knobs.throttle.hold_local(&row.address, knobs.revision);
    let value = match knobs.locals.get(&row.address) {
        Some(local) if held => *local,
        _ => row.value,
    };
    ParamRow {
        value,
        ..row.clone()
    }
}

fn send_choice(row: &ParamRow, value: f64, knobs: &mut Knobs) -> Command {
    knobs.locals.insert(row.address.clone(), value);
    knobs
        .throttle
        .on_release(&row.address, value, knobs.revision)
}

/// A choice Parameter in an open unit: its label over one button per choice, filling a knob column; a click or an arrow key on a focused button sends the choice.
fn choice_column(
    ui: &mut Ui,
    row: &ParamRow,
    accent: Color32,
    knobs: &mut Knobs,
) -> Option<Command> {
    let shown = shown_row(row, knobs);
    let buttons = choice_buttons(&shown, LARGE_KNOB_COLUMN_WIDTH);
    let height = buttons
        .iter()
        .map(|button| button.cell.1 + button.cell.3)
        .fold(0.0, f32::max);
    let column = Layout::top_down(Align::Center);
    ui.allocate_ui_with_layout(vec2(LARGE_KNOB_COLUMN_WIDTH, 0.0), column, |ui| {
        ui.label(muted_text(param_label(&row.param)));
        let (area, _) =
            ui.allocate_exact_size(vec2(LARGE_KNOB_COLUMN_WIDTH, height), Sense::hover());
        let id = |index: usize| Id::unique(("choice", &row.address, index));
        let mut chosen = None;
        let mut focused = false;
        for (index, button) in buttons.iter().enumerate() {
            let (left, top, width, height) = button.cell;
            let rect = Rect::from_min_size(area.min + vec2(left, top), vec2(width, height));
            let label = (button.text.as_str(), button.lit);
            let response = widgets::choice_button(ui, rect, id(index), label, accent);
            response.widget_info(|| {
                WidgetInfo::selected(Role::RadioButton, true, button.lit, &button.accessible_name)
            });
            if response.clicked() {
                chosen = Some(button.value);
            }
            focused |= response.has_focus();
        }
        if focused {
            let keys = choice_keys(ui);
            let stepped = keys
                .into_iter()
                .fold(shown.clone(), |row, key| ParamRow {
                    value: choice_step(&row, key),
                    ..row
                })
                .value;
            if keys_moved(stepped, shown.value) {
                ui.memory_mut(|memory| memory.request_focus(id(stepped as usize)));
                chosen = Some(stepped);
            }
        }
        chosen
            .filter(|&value| keys_moved(value, shown.value))
            .map(|value| send_choice(row, value, knobs))
    })
    .inner
}

fn keys_moved(value: f64, shown: f64) -> bool {
    (value - shown).abs() > f64::EPSILON
}

fn choice_keys(ui: &mut Ui) -> Vec<ChoiceKey> {
    ui.input_mut(|input| {
        CHOICE_KEYS
            .iter()
            .flat_map(|&(key, choice_key)| {
                std::iter::repeat_n(
                    choice_key,
                    input.count_and_consume_key(Modifiers::NONE, key),
                )
            })
            .collect()
    })
}

/// A choice Parameter in a closed unit: its label over a chip with the chosen label; a click switches two choices or asks to open the unit.
fn choice_chip(
    ui: &mut Ui,
    row: &ParamRow,
    accent: Color32,
    knobs: &mut Knobs,
) -> (Option<Command>, bool) {
    let shown = shown_row(row, knobs);
    let lit = shown.value.round().max(0.0) as usize;
    let text = shown
        .labels
        .get(lit)
        .map(|label| choice_text(label))
        .unwrap_or_default();
    let name = format!("{}, {text}, selected", param_label(&row.param));
    let group = Layout::top_down(Align::Min);
    let clicked = ui
        .allocate_ui_with_layout(vec2(CLOSED_KNOB_WIDTH, HEADER_HEIGHT), group, |ui| {
            ui.set_min_size(vec2(CLOSED_KNOB_WIDTH, HEADER_HEIGHT));
            ui.spacing_mut().item_spacing.y = UNIT_GAP;
            ui.label(muted_text(param_label(&row.param)));
            let (rect, _) = ui.allocate_exact_size(CHIP_SIZE, Sense::hover());
            let id = Id::unique(("chip", &row.address));
            let chip = widgets::choice_button(ui, rect, id, (&text, true), accent);
            chip.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
            chip.clicked()
        })
        .inner;
    if !clicked {
        return (None, false);
    }
    match chip_click(&shown) {
        ChipClick::Set(value) => (Some(send_choice(row, value, knobs)), false),
        ChipClick::Open => (None, true),
    }
}

fn param_knob(
    ui: &mut Ui,
    row: &ParamRow,
    size: KnobSize,
    accent: Color32,
    knobs: &mut Knobs,
) -> Option<Command> {
    let label = param_label(&row.param);
    param_knob_named(ui, row, (&label, &label), size, accent, knobs).0
}

/// A knob showing `label` whose value field is named `name`; also whether the knob is hovered, dragged or focused. A double-click reset, a key on the focused knob and a typed value are final values, like a release. Ctrl+click or a right-click focuses the knob's value field.
#[allow(
    clippy::float_cmp,
    reason = "an untouched knob returns the exact value it was shown"
)]
fn param_knob_named(
    ui: &mut Ui,
    row: &ParamRow,
    (label, name): (&str, &str),
    size: KnobSize,
    accent: Color32,
    knobs: &mut Knobs,
) -> (Option<Command>, bool) {
    let held = knobs.throttle.hold_local(&row.address, knobs.revision);
    let shown = match knobs.locals.get(&row.address) {
        Some(local) if held => *local,
        _ => row.value,
    };
    let fraction = knob_fraction(shown as f32, row.min as f32, row.max as f32);
    let field_id = Id::unique(("value", &row.address));
    let label = muted_text(label);
    let name_label = |ui: &mut Ui, label: RichText| {
        let response = ui.label(label);
        response.widget_info(|| WidgetInfo::labeled(Role::Label, true, name));
        response
    };
    let mut in_use = false;
    let mut knob_and_field = |ui: &mut Ui, knobs: &mut Knobs| {
        let knob = widgets::rack_knob(ui, size, fraction, accent);
        knob.response
            .widget_info(|| WidgetInfo::slider(true, shown, name));
        in_use = knob.response.hovered() || knob.response.dragged() || knob.response.has_focus();
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
                        let label = name_label(ui, label);
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
                let label = name_label(ui, label);
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
        let command = knobs
            .throttle
            .on_release(&row.address, value, knobs.revision);
        return (Some(command), in_use);
    }
    if value == shown {
        return (None, in_use);
    }
    knobs.locals.insert(row.address.clone(), value);
    (
        knobs.throttle.on_drag(&row.address, value, knobs.now),
        in_use,
    )
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
