use std::collections::HashMap;
use std::time::Instant;

use egui::style::ScrollStyle;
use egui::{
    Align, Align2, CornerRadius, DragValue, Frame, Id, Key, Label, Layout, Margin, Mesh, Rect,
    RichText, Role, ScrollArea, Sense, Shape as PaintShape, Stroke, StrokeKind, TextEdit, Ui,
    WidgetInfo, pos2, vec2,
};
use serde_json::Value;
use xguitare_core::control::SendLevel;
use xguitare_core::render::{RenderBits, RenderRange};

use crate::mixer_model::{
    Audibility, FADER_FLOOR_DB, FADER_TOP_DB, MixerColumn, MixerTarget, RenderSent, RenderView,
    SEND_ROW_HEIGHT, SendView, ShownRack, StereoPeakHold, StripFit, default_render_name, fader_db,
    fader_position, mixer_columns, render_view, send_fraction, send_level, send_text,
    shown_rack_column, strip_fit, strip_ladder, strip_row,
};
use crate::model::{Band, Command, EAR_WIDTH, Snapshot, TrackEdit};
use crate::project::{PAN, knob_with_reset, released_value};
use crate::project_model::{MainView, track_colour, transport_view};
use crate::rack::muted_text;
use crate::theme::{
    self, CONTROL_HEIGHT, RADIUS_PANEL, SPACE_M, SPACE_S, SPACE_XS, bus_colour, colour,
    master_colour, palette,
};
use crate::widgets::{
    self, KnobSize, edged_button, outlined_button, primary_button, switched_button,
};

const STRIP_MARGIN: Margin = Margin {
    left: 6,
    right: 6,
    top: 4,
    bottom: 2,
};
/// The rule between the strips and Master.
const RULE_WIDTH: f32 = 1.0;
const STRIP_GAP: f32 = SPACE_S;
/// The Track, Bus or Master colour along a strip's top.
const STRIP_TAB_HEIGHT: f32 = 3.0;
const SELECTED_EDGE_WIDTH: f32 = 2.0;
/// The faceplate around the strips: an ear and a gap at each side, a gap above and below.
const FACE_MARGIN: Margin = Margin {
    left: EAR_WIDTH as i8 + 8,
    right: EAR_WIDTH as i8 + 8,
    top: 2,
    bottom: 2,
};
const KIND_HEIGHT: f32 = 14.0;
const NAME_HEIGHT: f32 = CONTROL_HEIGHT;
const ROUTING_HEIGHT: f32 = CONTROL_HEIGHT;
const PAN_HEIGHT: f32 = 57.0;
const FADER_CAP: egui::Vec2 = egui::Vec2::new(34.0, 14.0);
const FADER_SLOT_WIDTH: f32 = 8.0;
const FADER_CAP_LINE: f32 = 2.0;
/// The dB marks printed left of the fader; each reads the fader and the ladders beside it.
const FADER_SCALE_DB: [f64; 6] = [6.0, 0.0, -6.0, -12.0, -24.0, -48.0];
const SCALE_WIDTH: f32 = 24.0;
const LADDER_WIDTH: f32 = 7.0;
const LADDER_GAP: f32 = 3.0;
const LED_GAP: f32 = 2.0;
const HOLD_WIDTH: f32 = 2.0;
/// The clip LED sits above the ladders in the half cap height the fader travel leaves there.
const CLIP_LED_HEIGHT: f32 = 5.0;
const VALUE_FIELD_HEIGHT: f32 = 20.0;
const SWITCH_SIZE: egui::Vec2 = egui::Vec2::new(40.0, CONTROL_HEIGHT);
const PRE_SIZE: egui::Vec2 = egui::Vec2::new(34.0, CONTROL_HEIGHT);
/// Between a Send's knob, readout and `pre`: narrow, so that `-12.0` fits a strip beside the Sends' scroll bar.
const SEND_ROW_GAP: f32 = 2.0;
const ROW_GAP: f32 = 1.0;
/// The mixer's scroll bars stay drawn while their content overflows, beside it rather than over it, so that hidden Sends and strips show.
const SCROLL_BAR_WIDTH: f32 = 4.0;
const SCROLL_BAR_MARGIN: f32 = 2.0;
const ADD_BUS_DASH: f32 = 5.0;
const ADD_BUS_GAP: f32 = 4.0;
/// A strip's edge and margins.
const STRIP_FRAME_HEIGHT: f32 = 2.0 + STRIP_MARGIN.top as f32 + STRIP_MARGIN.bottom as f32;
/// The rows of a strip other than its Sends and fader.
const FIXED_ROWS_HEIGHT: f32 = KIND_HEIGHT
    + NAME_HEIGHT
    + ROUTING_HEIGHT
    + PAN_HEIGHT
    + VALUE_FIELD_HEIGHT
    + SWITCH_SIZE.y
    + 7.0 * ROW_GAP;
const DIALOG_WIDTH: f32 = 420.0;
/// What the Render dialog's row with the progress bar leaves for `Cancel` and the gap before it.
const PROGRESS_CANCEL_WIDTH: f32 = 72.0;
const BITS: [(RenderBits, &str); 3] = [
    (RenderBits::Pcm24, "24"),
    (RenderBits::Pcm16, "16"),
    (RenderBits::Float32, "32f"),
];

/// The mixer panel's own state: the Rack a strip named for the window to show (`None` for the selected Track's), the peak holds per column, and the Render dialog while open.
#[derive(Debug, Default)]
pub struct MixerView {
    pub rack: Option<ShownRack>,
    pub holds: HashMap<MixerTarget, StereoPeakHold>,
    pub render: Option<RenderDraft>,
}

impl MixerView {
    pub fn observe(&mut self, status: &Value, now: Instant) {
        for column in mixer_columns(status) {
            self.holds
                .entry(column.target)
                .or_default()
                .observe(column.peak_db, now);
        }
    }

    /// Whether the Rack shown is a Bus or Master Rack.
    pub fn shows_stereo_rack(&self) -> bool {
        matches!(self.rack, Some(ShownRack::Bus(_) | ShownRack::Master))
    }
}

/// The Render dialog's fields: `name` empty sends none, so that the Daemon names the file after the Project folder; `sent` is the Render it last sent.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderDraft {
    pub range: RenderRange,
    pub bits: RenderBits,
    pub name: String,
    pub sent: Option<RenderSent>,
}

impl RenderDraft {
    pub fn new(snapshot: &Snapshot) -> RenderDraft {
        let name = snapshot
            .status
            .as_ref()
            .and_then(|status| default_render_name(snapshot.projects.as_ref(), status));
        RenderDraft {
            range: RenderRange::Project,
            bits: RenderBits::Pcm24,
            name: name.unwrap_or_default(),
            sent: None,
        }
    }
}

/// A faceplate between two rack ears: one strip per Track, then per Bus with the Add Bus slot after the last, scrolled sideways when they do not fit, and Master pinned at the right edge; the strips take the height ([`strip_fit`]) and scroll up and down in a window too short for them.
pub fn mixer_panel(ui: &mut Ui, snapshot: &Snapshot, view: &mut MixerView) -> Vec<Command> {
    let mut commands = Vec::new();
    let Some(status) = snapshot.status.as_ref() else {
        return commands;
    };
    let columns = mixer_columns(status);
    let Some((master, strips)) = columns.split_last() else {
        return commands;
    };
    let bus_names: Vec<String> = strips
        .iter()
        .filter(|column| matches!(column.target, MixerTarget::Bus(_)))
        .map(|column| column.name.clone())
        .collect();
    let selected = status["selected"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok());
    let open = shown_rack_column(view.rack, status);
    let strips_height = ui.available_height()
        - FACE_MARGIN.sum().y
        - SCROLL_BAR_WIDTH
        - SCROLL_BAR_MARGIN
        - STRIP_FRAME_HEIGHT
        - FIXED_ROWS_HEIGHT;
    let fit = strip_fit(strips_height, bus_names.len());
    let height = strip_height(fit);
    let show_column = |ui: &mut Ui, column: &MixerColumn, width: f32| {
        let hold = view.holds.get(&column.target).copied().unwrap_or_default();
        let shape = Shape {
            bus_names: &bus_names,
            fit,
            width: width - STRIP_MARGIN.sum().x - 2.0,
            selected,
            open: open == Some(column.target),
            rack: ShownRack::of(status, column.target),
        };
        mixer_column(ui, column, hold, &shape)
    };
    let colours = palette();
    ui.spacing_mut().scroll = scroll_style();
    let face = ScrollArea::vertical()
        .id_salt("mixer_columns")
        .show(ui, |ui| {
            Frame::new()
                .fill(colour(colours.panel))
                .inner_margin(FACE_MARGIN)
                .show(ui, |ui| {
                    ui.set_min_height(height + SCROLL_BAR_WIDTH + SCROLL_BAR_MARGIN);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = STRIP_GAP;
                        let row_width = ui.available_width() - RULE_WIDTH;
                        let row = strip_row(row_width, STRIP_GAP, strips.len() + 1);
                        let shown = row.shown as f32;
                        let scrolled_width = shown * row.width + (shown - 1.0) * STRIP_GAP;
                        ScrollArea::horizontal()
                            .id_salt("mixer")
                            .max_width(scrolled_width)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.horizontal_top(|ui| {
                                    for column in strips {
                                        commands.extend(show_column(ui, column, row.width));
                                    }
                                    if add_bus_slot(ui, vec2(row.width, height)) {
                                        commands.push(Command::BusAdd);
                                    }
                                });
                            });
                        ui.add_space(row_width - (shown + 1.0) * (row.width + STRIP_GAP));
                        let (rule, _) =
                            ui.allocate_exact_size(vec2(RULE_WIDTH, height), Sense::hover());
                        ui.painter()
                            .rect_filled(rule, 0, colour(colours.knob_highlight));
                        commands.extend(show_column(ui, master, row.width));
                    });
                })
                .response
                .rect
        });
    let rect = face.inner.intersect(face.inner_rect);
    let painter = ui.painter();
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, colour(colours.hover)),
    );
    widgets::rack_ears(painter, rect, None);
    commands
}

/// Solid bars, the handle in the text colour on a well track: the mixer's scroll areas and the one around the panel.
fn scroll_style() -> ScrollStyle {
    ScrollStyle {
        bar_width: SCROLL_BAR_WIDTH,
        bar_inner_margin: SCROLL_BAR_MARGIN,
        foreground_color: true,
        ..ScrollStyle::solid()
    }
}

/// A strip's height for `fit`, so that every strip and the Add Bus slot line up.
fn strip_height(fit: StripFit) -> f32 {
    STRIP_FRAME_HEIGHT
        + FIXED_ROWS_HEIGHT
        + SEND_ROW_HEIGHT * fit.send_rows as f32
        + fit.fader_height
}

struct Shape<'a> {
    bus_names: &'a [String],
    fit: StripFit,
    /// The strip's content width, inside its edge and margins.
    width: f32,
    selected: Option<usize>,
    open: bool,
    rack: Option<ShownRack>,
}

/// The colour along a strip's top, which its pan and Send arcs share.
fn strip_colour(target: MixerTarget) -> theme::Rgb {
    match target {
        MixerTarget::Track(index) => track_colour(index),
        MixerTarget::Bus(_) => bus_colour(),
        MixerTarget::Master => master_colour(),
    }
}

/// `Track 1`, `Bus 1` or `Master` above the strip's name.
fn kind_text(target: MixerTarget) -> String {
    match target {
        MixerTarget::Track(index) => format!("Track {}", index + 1),
        MixerTarget::Bus(index) => format!("Bus {}", index + 1),
        MixerTarget::Master => "Master".to_string(),
    }
}

/// One strip's faceplate: a coloured top tab, the open strip edged in the lit colour.
fn mixer_column(
    ui: &mut Ui,
    column: &MixerColumn,
    hold: StereoPeakHold,
    shape: &Shape,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let colours = palette();
    let strip = Frame::new()
        .fill(colour(colours.panel))
        .stroke(Stroke::new(1.0, colour(colours.hover)))
        .corner_radius(RADIUS_PANEL)
        .inner_margin(STRIP_MARGIN)
        .show(ui, |ui| {
            ui.set_width(shape.width);
            ui.set_min_height(strip_height(shape.fit) - STRIP_FRAME_HEIGHT);
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                kind_label(ui, column.target);
                commands.extend(name_button(ui, column, shape));
                ui.allocate_ui(vec2(shape.width, ROUTING_HEIGHT), |ui| {
                    ui.set_min_height(ROUTING_HEIGHT);
                    commands.extend(routing(ui, column));
                });
                let sends_height = SEND_ROW_HEIGHT * shape.fit.send_rows as f32;
                ui.allocate_ui(vec2(shape.width, sends_height), |ui| {
                    ui.set_min_height(sends_height);
                    commands.extend(sends(ui, column, shape.bus_names, sends_height));
                });
                ui.allocate_ui(vec2(shape.width, PAN_HEIGHT), |ui| {
                    ui.set_min_height(PAN_HEIGHT);
                    commands.extend(pan_knob(ui, column));
                });
                commands.extend(fader_and_meter(ui, column, hold, shape.fit.fader_height));
                commands.extend(volume_field(ui, column));
                commands.extend(mute_and_solo(ui, column));
            });
        });
    let rect = strip.response.rect;
    let painter = ui.painter();
    if shape.open {
        painter.rect_stroke(
            rect,
            RADIUS_PANEL,
            Stroke::new(SELECTED_EDGE_WIDTH, colour(colours.lit)),
            StrokeKind::Inside,
        );
    }
    let tab = Rect::from_min_size(rect.min, vec2(rect.width(), STRIP_TAB_HEIGHT));
    let rounded_top = CornerRadius {
        nw: RADIUS_PANEL,
        ne: RADIUS_PANEL,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(tab, rounded_top, colour(strip_colour(column.target)));
    commands
}

fn kind_label(ui: &mut Ui, target: MixerTarget) {
    let row = vec2(ui.available_width(), KIND_HEIGHT);
    ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(row);
        ui.label(muted_text(kind_text(target)).font(theme::legend()));
    });
}

/// Makes the column's Rack the shown Rack, and Enter on it also shows the Rack view; a Bus's name also has `Remove` in its context menu.
fn name_button(ui: &mut Ui, column: &MixerColumn, shape: &Shape) -> Vec<Command> {
    let mut commands = Vec::new();
    let colours = palette();
    let text_colour = if column.audibility == Audibility::Silent {
        colours.muted
    } else {
        colours.text
    };
    let text = RichText::new(&column.name)
        .font(theme::unit_name())
        .color(colour(text_colour));
    let button = outlined_button(text)
        .selected(shape.open)
        .min_size(vec2(shape.width, NAME_HEIGHT))
        .truncate();
    let response = ui.add(button);
    response.widget_info(|| {
        WidgetInfo::labeled(Role::Button, true, format!("Rack of {}", column.name))
    });
    if response.clicked() {
        if let MixerTarget::Track(index) = column.target
            && shape.selected != Some(index)
        {
            commands.push(Command::TrackSelect(index));
        }
        commands.extend(shape.rack.map(Command::ShowRack));
        if response.has_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
            commands.push(Command::ShowView(MainView::Rack));
        }
    }
    if let MixerTarget::Bus(index) = column.target {
        response.context_menu(|ui| {
            if ui.add(outlined_button("Remove")).clicked() {
                commands.push(Command::BusRemove(index));
                ui.close();
            }
        });
    }
    commands
}

/// A Track's `to Master` switch, lit while routed; Master's `Main out`; nothing for a Bus.
fn routing(ui: &mut Ui, column: &MixerColumn) -> Option<Command> {
    if column.target == MixerTarget::Master {
        ui.label(muted_text("Main out"));
        return None;
    }
    let (MixerTarget::Track(track), Some(to_master)) = (column.target, column.to_master) else {
        return None;
    };
    let toggle = switched_button("to Master", to_master, palette().lit)
        .min_size(vec2(ui.available_width(), ROUTING_HEIGHT));
    let response = ui.add(toggle);
    response.widget_info(|| {
        WidgetInfo::labeled(Role::Button, true, format!("to Master {}", column.name))
    });
    response.clicked().then(|| Command::TrackSet {
        track,
        edit: TrackEdit {
            to_master: Some(!to_master),
            ..TrackEdit::default()
        },
    })
}

/// One Send knob with a `pre` switch per Bus, in their own scroll past `height`; nothing for a Bus or Master.
fn sends(ui: &mut Ui, column: &MixerColumn, bus_names: &[String], height: f32) -> Vec<Command> {
    let mut commands = Vec::new();
    let MixerTarget::Track(track) = column.target else {
        return commands;
    };
    ScrollArea::vertical()
        .id_salt(("sends", track))
        .max_height(height)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let row_width = ui.available_width();
            for send in &column.sends {
                let bus = bus_names.get(send.bus).map_or("", String::as_str);
                commands.extend(send_row(ui, (track, row_width), (&column.name, bus), send));
            }
        });
    commands
}

fn send_row(
    ui: &mut Ui,
    (track, row_width): (usize, f32),
    (name, bus): (&str, &str),
    send: &SendView,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let colours = palette();
    ui.allocate_ui(vec2(row_width, SEND_ROW_HEIGHT), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SEND_ROW_GAP;
            let accent = if send.level_db.is_some() {
                bus_colour()
            } else {
                colours.muted
            };
            let knob = widgets::rack_knob(
                ui,
                KnobSize::Strip,
                (send_fraction(send.level_db), 0.0),
                colour(accent),
            );
            knob.response.widget_info(|| {
                let mut info =
                    WidgetInfo::labeled(Role::Slider, true, format!("Send {name} to {bus}"));
                info.current_text_value = Some(send_text(send.level_db));
                info
            });
            let level = knob_release(ui, &knob).map(send_level);
            let level = if knob.reset {
                Some(SendLevel::Db(0.0))
            } else {
                level
            };
            let dragged = knob
                .response
                .dragged()
                .then(|| match send_level(knob.fraction) {
                    SendLevel::Db(db) => Some(f64::from(db)),
                    SendLevel::Off => None,
                });
            let readout_width =
                row_width - KnobSize::Strip.diameter() - PRE_SIZE.x - 2.0 * SEND_ROW_GAP;
            ui.allocate_ui(vec2(readout_width, SEND_ROW_HEIGHT), |ui| {
                ui.set_width(readout_width);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add(Label::new(muted_text(bus).font(theme::legend())).truncate());
                    let shown = send_text(dragged.unwrap_or(send.level_db));
                    let shown = RichText::new(fitted_send_text(ui, shown)).font(theme::scale());
                    ui.add(Label::new(shown).truncate());
                });
            });
            let pre = switched_button("pre", send.pre, colours.lit).min_size(PRE_SIZE);
            let pre = ui.add(pre);
            pre.widget_info(|| {
                WidgetInfo::labeled(Role::Button, true, format!("pre {name} {bus}"))
            });
            let pre = pre.clicked().then_some(!send.pre);
            if level.is_some() || pre.is_some() {
                commands.push(Command::SendSet {
                    track,
                    bus: send.bus,
                    level,
                    pre,
                });
            }
        });
    });
    commands
}

/// The Send level as [`send_text`] gives it, without its ` dB` in a strip too narrow for that.
fn fitted_send_text(ui: &Ui, text: String) -> String {
    let width = ui
        .painter()
        .layout_no_wrap(text.clone(), theme::scale(), colour(palette().text))
        .size()
        .x;
    if width <= ui.available_width() {
        return text;
    }
    text.trim_end_matches(" dB").to_string()
}

/// The knob's position once its drag ends.
fn knob_release(ui: &Ui, knob: &widgets::RackKnob) -> Option<f32> {
    let dragged_id = knob.response.id.with("dragged");
    if knob.response.dragged() {
        ui.data_mut(|data| data.insert_temp(dragged_id, knob.fraction));
    }
    if knob.response.drag_stopped() {
        return ui.data_mut(|data| data.remove_temp::<f32>(dragged_id));
    }
    None
}

/// The pan knob, its arc from the centre in the strip's colour.
fn pan_knob(ui: &mut Ui, column: &MixerColumn) -> Option<Command> {
    let pan = column.pan?;
    let accent = colour(strip_colour(column.target));
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a pan between -1 and 1 fits an f32"
    )]
    let changed = knob_with_reset(ui, ("Pan", &column.name), pan, PAN, "", 0.0, accent)? as f32;
    Some(match column.target {
        MixerTarget::Track(track) => Command::TrackSet {
            track,
            edit: TrackEdit {
                pan: Some(changed),
                ..TrackEdit::default()
            },
        },
        MixerTarget::Bus(bus) => Command::BusSet {
            bus,
            volume: None,
            pan: Some(changed),
            mute: None,
            solo: None,
        },
        MixerTarget::Master => return None,
    })
}

/// The volume `column` sends for `db`, raised to the lowest the Daemon takes.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a volume between -60 and 6 dB fits an f32"
)]
fn volume_command(column: &MixerColumn, db: f64) -> Command {
    let volume = db.clamp(FADER_FLOOR_DB, FADER_TOP_DB) as f32;
    match column.target {
        MixerTarget::Track(track) => Command::TrackSet {
            track,
            edit: TrackEdit {
                volume: Some(volume),
                ..TrackEdit::default()
            },
        },
        MixerTarget::Bus(bus) => Command::BusSet {
            bus,
            volume: Some(volume),
            pan: None,
            mute: None,
            solo: None,
        },
        MixerTarget::Master => Command::MasterSet(volume),
    }
}

/// The printed dB scale, the fader (drag it; sent on release, a double-click for 0 dB), and the twin LED ladders with their peak hold under the latching clip LED (click to reset), all on the fader's travel.
fn fader_and_meter(
    ui: &mut Ui,
    column: &MixerColumn,
    hold: StereoPeakHold,
    height: f32,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let meter_width = 2.0 * LADDER_WIDTH + LADDER_GAP;
    let zone_width = SCALE_WIDTH + FADER_CAP.x + meter_width + 2.0 * theme::SPACE_XS;
    let strip_width = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SPACE_XS;
        ui.add_space((strip_width - zone_width) / 2.0 - theme::SPACE_XS);
        fader_scale(ui, height);
        commands.extend(fader(ui, column, height));
        commands.extend(stereo_meter(ui, column, hold, vec2(meter_width, height)));
    });
    commands
}

/// The y of a fader position inside `rect`: the cap's centre travels from half a cap below the top to half a cap above the bottom.
fn travel_y(rect: Rect, position: f32) -> f32 {
    let travel = rect.height() - FADER_CAP.y;
    rect.top() + FADER_CAP.y / 2.0 + travel * (1.0 - position)
}

fn fader_scale(ui: &mut Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(SCALE_WIDTH, height), Sense::hover());
    let painter = ui.painter();
    for db in FADER_SCALE_DB {
        let text = if db > 0.0 {
            format!("+{db:.0}")
        } else {
            format!("{db:.0}").replace('-', "\u{2212}")
        };
        painter.text(
            pos2(rect.right(), travel_y(rect, fader_position(db))),
            Align2::RIGHT_CENTER,
            text,
            theme::scale(),
            colour(palette().muted),
        );
    }
}

fn fader(ui: &mut Ui, column: &MixerColumn, height: f32) -> Option<Command> {
    let (rect, response) =
        ui.allocate_exact_size(vec2(FADER_CAP.x, height), Sense::click_and_drag());
    response
        .widget_info(|| WidgetInfo::labeled(Role::Slider, true, format!("Volume {}", column.name)));
    let id = response.id.with("position");
    let travel = rect.height() - FADER_CAP.y;
    let position_at =
        |y: f32| (1.0 - (y - rect.top() - FADER_CAP.y / 2.0) / travel).clamp(0.0, 1.0);
    let mut position = ui
        .data(|data| data.get_temp::<f32>(id))
        .unwrap_or_else(|| fader_position(column.volume_db));
    let mut command = None;
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        position = position_at(pointer.y);
        ui.data_mut(|data| data.insert_temp(id, position));
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove_temp::<f32>(id));
        command = Some(volume_command(column, fader_db(position)));
    }
    if response.double_clicked() {
        command = Some(volume_command(column, 0.0));
    }
    let colours = palette();
    let painter = ui.painter();
    let slot = Rect::from_center_size(
        rect.center(),
        vec2(FADER_SLOT_WIDTH, travel + FADER_SLOT_WIDTH),
    );
    painter.rect(
        slot,
        FADER_SLOT_WIDTH / 2.0,
        colour(colours.well),
        Stroke::new(1.0, colour(colours.boundary)),
        StrokeKind::Inside,
    );
    let cap_y = travel_y(rect, position);
    let cap = Rect::from_center_size(pos2(rect.center().x, cap_y), FADER_CAP);
    paint_fader_cap(painter, cap);
    if response.hovered() || response.dragged() {
        painter.rect_stroke(
            cap,
            0,
            Stroke::new(1.0, colour(colours.hover_boundary)),
            StrokeKind::Outside,
        );
    }
    command
}

/// A steel cap, light at the top and darker below, with a dark edge and centre line.
fn paint_fader_cap(painter: &egui::Painter, cap: Rect) {
    let colours = palette();
    let mut mesh = Mesh::default();
    for (y, shade) in [
        (cap.top(), colours.ear_centre),
        (cap.bottom(), colours.ear_edge),
    ] {
        mesh.colored_vertex(pos2(cap.left(), y), colour(shade));
        mesh.colored_vertex(pos2(cap.right(), y), colour(shade));
    }
    for first in [0, 1] {
        mesh.add_triangle(first, first + 1, first + 2);
    }
    painter.add(mesh);
    let dark = colour(colours.on_accent);
    painter.rect_stroke(cap, 0, Stroke::new(1.0, dark), StrokeKind::Inside);
    painter.hline(
        cap.x_range(),
        cap.center().y,
        Stroke::new(FADER_CAP_LINE, dark),
    );
}

/// Two LED ladders on the fader's travel with the peak hold across each, under one clip LED; a click on the LED or the ladders' top resets the clip.
fn stereo_meter(
    ui: &mut Ui,
    column: &MixerColumn,
    hold: StereoPeakHold,
    size: egui::Vec2,
) -> Option<Command> {
    let width = size.x;
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let [left, right] = column.peak_db;
    response.widget_info(|| {
        WidgetInfo::labeled(
            Role::Meter,
            true,
            format!("{} {left:.1} / {right:.1} dB", column.name),
        )
    });
    let colours = palette();
    let led = Rect::from_min_size(rect.min, vec2(width, CLIP_LED_HEIGHT));
    let target = Rect::from_min_size(
        led.min - vec2(theme::SPACE_XS, 0.0),
        vec2(width + 2.0 * theme::SPACE_XS, CONTROL_HEIGHT),
    );
    let clip = ui.interact(target, response.id.with("clip"), Sense::click());
    clip.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Clip {}", column.name)));
    let painter = ui.painter();
    let (fill, edge) = if column.clip {
        (colours.danger, colours.danger)
    } else {
        (colours.clip_off, colours.boundary)
    };
    painter.rect(
        led,
        theme::RADIUS_LED,
        colour(fill),
        Stroke::new(1.0, colour(edge)),
        StrokeKind::Inside,
    );
    let travel = Rect::from_x_y_ranges(rect.x_range(), travel_y(rect, 1.0)..=travel_y(rect, 0.0));
    for side in 0..2 {
        let ladder_left = rect.left() + side as f32 * (LADDER_WIDTH + LADDER_GAP);
        let ladder =
            Rect::from_x_y_ranges(ladder_left..=ladder_left + LADDER_WIDTH, travel.y_range());
        paint_ladder(painter, ladder, column.peak_db[side]);
        let held_db = hold.held_db()[side];
        if held_db > FADER_FLOOR_DB {
            let hold_y = travel_y(rect, fader_position(held_db));
            painter.hline(
                ladder.x_range(),
                hold_y,
                Stroke::new(HOLD_WIDTH, colour(colours.text)),
            );
        }
    }
    clip.clicked().then_some(Command::ClipReset(column.target))
}

fn paint_ladder(painter: &egui::Painter, ladder: Rect, db: f64) {
    let colours = palette();
    let segments = strip_ladder(db);
    let pitch = ladder.height() / segments.len() as f32;
    for (index, segment) in segments.iter().enumerate() {
        let bottom = ladder.bottom() - index as f32 * pitch;
        let led = Rect::from_x_y_ranges(ladder.x_range(), bottom - pitch + LED_GAP..=bottom);
        let fill = match (segment.lit, segment.band) {
            (false, _) => colours.arc_track,
            (true, Band::Green) => colours.go,
            (true, Band::Yellow) => colours.lit,
            (true, Band::Red) => colours.danger,
        };
        painter.rect_filled(led, theme::RADIUS_LED, colour(fill));
    }
}

fn volume_field(ui: &mut Ui, column: &MixerColumn) -> Option<Command> {
    let id = Id::unique(("mixer_volume", column.target));
    let shown_db = column.volume_db.max(FADER_FLOOR_DB);
    let (typed, field) = released_value(ui, id, shown_db, |db| {
        DragValue::new(db)
            .range(FADER_FLOOR_DB..=FADER_TOP_DB)
            .speed(0.2)
            .fixed_decimals(1)
            .suffix(" dB")
    });
    field.widget_info(|| {
        WidgetInfo::labeled(
            Role::SpinButton,
            true,
            format!("Volume {} value", column.name),
        )
    });
    typed.map(|db| volume_command(column, db))
}

/// The Add Bus slot: a dashed outline as tall as a strip, its name at the top.
fn add_bus_slot(ui: &mut Ui, size: egui::Vec2) -> bool {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "+ Add Bus"));
    let colours = palette();
    let hovered = response.hovered() || response.is_pointer_button_down_on();
    let edge = if hovered {
        colours.hover_boundary
    } else {
        colours.boundary
    };
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, RADIUS_PANEL, colour(colours.hover));
    }
    let outline = rect.shrink(0.5);
    let corners = [
        outline.left_top(),
        outline.right_top(),
        outline.right_bottom(),
        outline.left_bottom(),
        outline.left_top(),
    ];
    painter.extend(PaintShape::dashed_line(
        &corners,
        Stroke::new(1.0, colour(edge)),
        ADD_BUS_DASH,
        ADD_BUS_GAP,
    ));
    painter.text(
        pos2(rect.center().x, rect.top() + CONTROL_HEIGHT),
        Align2::CENTER_CENTER,
        "+ Add Bus",
        theme::body(),
        colour(colours.text),
    );
    response.clicked()
}

/// M lit in the lit colour, S in the go colour; S is edged in go on a column that sounds only through solo in place; Master has neither.
fn mute_and_solo(ui: &mut Ui, column: &MixerColumn) -> Option<Command> {
    if column.target == MixerTarget::Master {
        return None;
    }
    let colours = palette();
    let mut mute = None;
    let mut solo = None;
    let strip_width = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SPACE_XS;
        ui.add_space((strip_width - 2.0 * SWITCH_SIZE.x - theme::SPACE_XS) / 2.0 - theme::SPACE_XS);
        let response = ui.add(switched_button("M", column.mute, colours.lit).min_size(SWITCH_SIZE));
        response.widget_info(|| {
            WidgetInfo::labeled(Role::Button, true, format!("Mute {}", column.name))
        });
        if response.clicked() {
            mute = Some(!column.mute);
        }
        let solo_button = if column.audibility == Audibility::SoloInPlace && !column.solo {
            edged_button("S", colours.go)
        } else {
            switched_button("S", column.solo, colours.go)
        };
        let response = ui.add(solo_button.min_size(SWITCH_SIZE));
        response.widget_info(|| {
            WidgetInfo::labeled(Role::Button, true, format!("Solo {}", column.name))
        });
        if response.clicked() {
            solo = Some(!column.solo);
        }
    });
    if mute.is_none() && solo.is_none() {
        return None;
    }
    Some(match column.target {
        MixerTarget::Track(track) => Command::TrackSet {
            track,
            edit: TrackEdit {
                mute,
                solo,
                ..TrackEdit::default()
            },
        },
        MixerTarget::Bus(bus) => Command::BusSet {
            bus,
            volume: None,
            pan: None,
            mute,
            solo,
        },
        MixerTarget::Master => return None,
    })
}

/// The Render dialog over the window; `false` once it is closed (`Close`, Esc or a click outside).
pub fn render_dialog(
    ctx: &egui::Context,
    snapshot: &Snapshot,
    draft: &mut RenderDraft,
) -> (Vec<Command>, bool) {
    let mut commands = Vec::new();
    let mut open = true;
    let Some(status) = snapshot.status.as_ref() else {
        return (commands, false);
    };
    let has_loop = transport_view(status)
        .is_some_and(|transport| transport.loop_region.start < transport.loop_region.end);
    if !has_loop {
        draft.range = RenderRange::Project;
    }
    let state = render_view(
        status,
        snapshot.refused_render.as_ref(),
        draft.sent.as_ref(),
    );
    let running = matches!(state, RenderView::Running(_));
    let modal = widgets::modal(Id::unique("render_dialog")).show(ctx, |ui| {
        ui.set_width(DIALOG_WIDTH);
        ui.spacing_mut().interact_size.y = CONTROL_HEIGHT;
        ui.label(RichText::new("Render").font(theme::dialog_title()));
        ui.add_space(SPACE_XS);
        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("render_fields")
                .num_columns(2)
                .spacing(vec2(SPACE_M, SPACE_S))
                .show(ui, |ui| {
                    ui.label(field_label("Range"));
                    widgets::lit_radios(ui, |ui| {
                        ui.radio_value(&mut draft.range, RenderRange::Project, "Project");
                        ui.add_enabled_ui(has_loop, |ui| {
                            ui.radio_value(&mut draft.range, RenderRange::Loop, "Loop");
                        })
                        .response
                        .on_disabled_hover_text("no loop region");
                    });
                    ui.end_row();
                    ui.label(field_label("Bits"));
                    widgets::lit_radios(ui, |ui| {
                        for (bits, text) in BITS {
                            ui.radio_value(&mut draft.bits, bits, text);
                        }
                    });
                    ui.end_row();
                    let label = ui.label(field_label("Name"));
                    ui.add(widgets::text_field(
                        TextEdit::singleline(&mut draft.name).desired_width(f32::INFINITY),
                    ))
                    .labelled_by(label.id);
                    ui.end_row();
                });
        });
        ui.add_space(SPACE_S);
        commands.extend(render_state(ui, &state));
        ui.add_space(SPACE_S);
        ui.horizontal(|ui| {
            if ui.add_enabled(!running, primary_button("Render")).clicked() {
                draft.sent = Some(RenderSent {
                    at: Instant::now(),
                    previous: status["render"].clone(),
                });
                let name = draft.name.trim();
                commands.push(Command::Render {
                    range: draft.range,
                    bits: draft.bits,
                    name: (!name.is_empty()).then(|| name.to_string()),
                });
            }
            if ui.add(outlined_button("Close")).clicked() {
                open = false;
            }
        });
    });
    if modal.should_close() {
        open = false;
    }
    (commands, open)
}

/// A field's name in the legend role, muted.
fn field_label(text: &str) -> RichText {
    RichText::new(text)
        .font(theme::legend())
        .color(colour(palette().muted))
}

/// The progress bar with `Cancel` while a Render runs, else how the last one ended.
fn render_state(ui: &mut Ui, state: &RenderView) -> Option<Command> {
    let colours = palette();
    match state {
        RenderView::Idle => None,
        RenderView::Running(progress) => {
            let mut cancel = false;
            ui.horizontal(|ui| {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "a progress in 0..=1 fits an f32"
                )]
                let progress = *progress as f32;
                widgets::progress_bar(ui, progress, DIALOG_WIDTH - PROGRESS_CANCEL_WIDTH);
                cancel = ui.add(outlined_button("Cancel")).clicked();
            });
            cancel.then_some(Command::RenderCancel)
        }
        RenderView::Done(file) => {
            ui.label(RichText::new(format!("rendered {file}")).color(colour(colours.go)));
            None
        }
        RenderView::Failed(error) => {
            ui.label(RichText::new(format!("\u{26a0} {error}")).color(colour(colours.danger_text)))
                .widget_info(|| WidgetInfo::labeled(Role::Alert, true, error));
            None
        }
        RenderView::Cancelled => {
            ui.label(muted_text("render cancelled"));
            None
        }
    }
}
