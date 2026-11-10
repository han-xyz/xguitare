use std::collections::HashMap;
use std::time::Instant;

use egui::{
    Color32, DragValue, Frame, Id, Margin, Modal, ProgressBar, Rect, RichText, Role, ScrollArea,
    Sense, Stroke, TextEdit, Ui, WidgetInfo, pos2, vec2,
};
use serde_json::Value;
use xguitare_core::control::SendLevel;
use xguitare_core::render::{RenderBits, RenderRange};

use crate::mixer_model::{
    Audibility, FADER_FLOOR_DB, FADER_TOP_DB, MixerColumn, MixerTarget, RenderSent, RenderView,
    SendView, ShownRack, StereoPeakHold, default_render_name, fader_db, fader_position,
    mixer_columns, render_view, send_fraction, send_level, send_text,
};
use crate::model::{Command, LADDER_RED_DB, LADDER_YELLOW_DB, Snapshot, TrackEdit, palette};
use crate::project::{PAN, lit_button, released_value, track_knob};
use crate::project_model::{track_colour, transport_view};
use crate::rack::muted_text;
use crate::widgets::{self, KnobSize, colour, outlined_button, primary_button};

const COLUMN_WIDTH: f32 = 118.0;
const COLUMN_GAP: f32 = 4.0;
/// A column's outer width: its content, inner margin and stroke.
const MASTER_WIDTH: f32 = COLUMN_WIDTH + 10.0;
const NAME_HEIGHT: f32 = 26.0;
const SEND_ROW_HEIGHT: f32 = 40.0;
const TO_MASTER_HEIGHT: f32 = 28.0;
const PAN_HEIGHT: f32 = 58.0;
const FADER_SIZE: egui::Vec2 = egui::Vec2::new(26.0, 140.0);
const FADER_CAP: egui::Vec2 = egui::Vec2::new(26.0, 12.0);
const METER_BAR_WIDTH: f32 = 7.0;
const METER_GAP: f32 = 2.0;
const CLIP_LED: egui::Vec2 = egui::Vec2::new(16.0, 8.0);
const SWITCH_SIZE: egui::Vec2 = egui::Vec2::new(30.0, 22.0);
const SMALL_TEXT: f32 = 11.0;
const SOLO_IN_PLACE_ALPHA: u8 = 90;
const PRE_SIZE: egui::Vec2 = egui::Vec2::new(32.0, 20.0);
const DIALOG_WIDTH: f32 = 420.0;
const BITS: [(RenderBits, &str); 3] = [
    (RenderBits::Pcm24, "24"),
    (RenderBits::Pcm16, "16"),
    (RenderBits::Float32, "32f"),
];

/// The mixer panel's own state: whether it is open, the column whose Rack is open below it, the peak holds per column, and the Render dialog while open.
#[derive(Debug, Default)]
pub struct MixerView {
    pub open: bool,
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

    /// The Rack shown under the panel, while it is open.
    pub fn shown_rack(&self) -> Option<ShownRack> {
        self.rack.filter(|_| self.open)
    }

    /// Whether the Rack shown is a Bus or Master Rack.
    pub fn shows_stereo_rack(&self) -> bool {
        matches!(
            self.shown_rack(),
            Some(ShownRack::Bus(_) | ShownRack::Master)
        )
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

/// One column per Track, then per Bus with `Add Bus` after the last, scrolled sideways when they do not fit, and Master at the right edge.
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
    let open = view.rack.and_then(|shown| shown.column(status));
    let show_column = |ui: &mut Ui, column: &MixerColumn| {
        let hold = view.holds.get(&column.target).copied().unwrap_or_default();
        let shape = Shape {
            bus_names: &bus_names,
            send_rows: bus_names.len(),
            selected,
            open: open == Some(column.target),
            rack: ShownRack::of(status, column.target),
        };
        mixer_column(ui, column, hold, &shape)
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = COLUMN_GAP;
        let scrolled_width = ui.available_width() - MASTER_WIDTH - COLUMN_GAP;
        ScrollArea::horizontal()
            .id_salt("mixer")
            .max_width(scrolled_width)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for column in strips {
                        commands.extend(show_column(ui, column));
                    }
                    if ui.add(primary_button("+ Add Bus")).clicked() {
                        commands.push(Command::BusAdd);
                    }
                });
            });
        commands.extend(show_column(ui, master));
    });
    commands
}

struct Shape<'a> {
    bus_names: &'a [String],
    send_rows: usize,
    selected: Option<usize>,
    open: bool,
    rack: Option<ShownRack>,
}

fn mixer_column(
    ui: &mut Ui,
    column: &MixerColumn,
    hold: StereoPeakHold,
    shape: &Shape,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let colours = palette();
    let fill = if shape.open {
        colours.row_active
    } else {
        colours.unit
    };
    Frame::new()
        .fill(colour(fill))
        .stroke(Stroke::new(1.0, colour(colours.outline)))
        .inner_margin(Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(COLUMN_WIDTH);
            ui.vertical_centered(|ui| {
                commands.extend(name_button(ui, column, shape));
                let sends_height = TO_MASTER_HEIGHT + SEND_ROW_HEIGHT * shape.send_rows as f32;
                ui.allocate_ui(vec2(COLUMN_WIDTH, sends_height), |ui| {
                    ui.set_min_height(sends_height);
                    commands.extend(track_routing(ui, column, shape.bus_names));
                });
                ui.allocate_ui(vec2(COLUMN_WIDTH, PAN_HEIGHT), |ui| {
                    ui.set_min_height(PAN_HEIGHT);
                    commands.extend(pan_knob(ui, column));
                });
                commands.extend(fader_and_meter(ui, column, hold));
                commands.extend(volume_field(ui, column));
                commands.extend(mute_and_solo(ui, column));
            });
        });
    commands
}

/// Opens the column's Rack below the panel; a Bus's name also has `Remove` in its context menu.
fn name_button(ui: &mut Ui, column: &MixerColumn, shape: &Shape) -> Vec<Command> {
    let mut commands = Vec::new();
    let colours = palette();
    let text_colour = if column.audibility == Audibility::Silent {
        colours.muted
    } else {
        colours.text
    };
    let mut text = RichText::new(&column.name)
        .strong()
        .color(colour(text_colour));
    if let MixerTarget::Track(index) = column.target {
        text = text.color(colour(track_colour(index)));
    }
    let button = egui::Button::new(text)
        .selected(shape.open)
        .min_size(vec2(COLUMN_WIDTH, NAME_HEIGHT))
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
    }
    if let MixerTarget::Bus(index) = column.target {
        response.context_menu(|ui| {
            if ui.button("Remove").clicked() {
                commands.push(Command::BusRemove(index));
                ui.close();
            }
        });
    }
    commands
}

/// `to Master` and one Send knob with a `pre` toggle per Bus; nothing for a Bus or Master.
fn track_routing(ui: &mut Ui, column: &MixerColumn, bus_names: &[String]) -> Vec<Command> {
    let mut commands = Vec::new();
    let (MixerTarget::Track(track), Some(to_master)) = (column.target, column.to_master) else {
        return commands;
    };
    let colours = palette();
    let toggle = lit_button("to Master", to_master, colour(colours.green))
        .min_size(vec2(COLUMN_WIDTH - 8.0, TO_MASTER_HEIGHT - 6.0));
    let response = ui.add(toggle);
    response.widget_info(|| {
        WidgetInfo::labeled(Role::Button, true, format!("to Master {}", column.name))
    });
    if response.clicked() {
        commands.push(Command::TrackSet {
            track,
            edit: TrackEdit {
                to_master: Some(!to_master),
                ..TrackEdit::default()
            },
        });
    }
    for send in &column.sends {
        let bus = bus_names.get(send.bus).map_or("", String::as_str);
        commands.extend(send_row(ui, track, (&column.name, bus), send));
    }
    commands
}

fn send_row(ui: &mut Ui, track: usize, (name, bus): (&str, &str), send: &SendView) -> Vec<Command> {
    let mut commands = Vec::new();
    let colours = palette();
    ui.allocate_ui(vec2(COLUMN_WIDTH, SEND_ROW_HEIGHT), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let accent = if send.level_db.is_some() {
                colours.yellow
            } else {
                colours.muted
            };
            let knob = widgets::rack_knob(
                ui,
                KnobSize::Small,
                send_fraction(send.level_db),
                colour(accent),
            );
            knob.response.widget_info(|| {
                WidgetInfo::labeled(Role::Slider, true, format!("Send {name} to {bus}"))
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
            ui.vertical(|ui| {
                ui.label(RichText::new(bus).size(SMALL_TEXT).strong());
                let shown = dragged.unwrap_or(send.level_db);
                ui.label(muted_text(send_text(shown)).size(SMALL_TEXT));
            });
            let pre = lit_button("pre", send.pre, colour(colours.yellow)).min_size(PRE_SIZE);
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

fn pan_knob(ui: &mut Ui, column: &MixerColumn) -> Option<Command> {
    let pan = column.pan?;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a pan between -1 and 1 fits an f32"
    )]
    let changed = track_knob(ui, ("Pan", &column.name), pan, PAN, "")? as f32;
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

/// The fader (drag it; sent on release, a double-click for 0 dB), the clip LED (click to reset) and the stereo meter with its peak hold, on one scale.
fn fader_and_meter(ui: &mut Ui, column: &MixerColumn, hold: StereoPeakHold) -> Vec<Command> {
    let mut commands = Vec::new();
    ui.horizontal(|ui| {
        let meter_width = 2.0 * METER_BAR_WIDTH + METER_GAP;
        ui.add_space((COLUMN_WIDTH - FADER_SIZE.x - meter_width - 16.0) / 2.0);
        commands.extend(fader(ui, column));
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let (led, response) =
                ui.allocate_exact_size(vec2(meter_width, CLIP_LED.y), Sense::click());
            response.widget_info(|| {
                WidgetInfo::labeled(Role::Button, true, format!("Clip {}", column.name))
            });
            let colours = palette();
            let lit = if column.clip {
                colours.red
            } else {
                colours.led_off
            };
            ui.painter().rect_filled(led, 1, colour(lit));
            if response.clicked() {
                commands.push(Command::ClipReset(column.target));
            }
            stereo_meter(ui, column, hold, meter_width);
        });
    });
    commands
}

fn fader(ui: &mut Ui, column: &MixerColumn) -> Option<Command> {
    let (rect, response) = ui.allocate_exact_size(FADER_SIZE, Sense::click_and_drag());
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
    let slot = Rect::from_center_size(rect.center(), vec2(4.0, travel));
    painter.rect_filled(slot, 2, colour(colours.knob_body));
    let unity_y = rect.top() + FADER_CAP.y / 2.0 + travel * (1.0 - fader_position(0.0));
    painter.hline(
        rect.x_range(),
        unity_y,
        Stroke::new(1.0, colour(colours.button_border)),
    );
    let cap_y = rect.top() + FADER_CAP.y / 2.0 + travel * (1.0 - position);
    let cap = Rect::from_center_size(pos2(rect.center().x, cap_y), FADER_CAP);
    let rim = if response.hovered() || response.dragged() {
        colours.text
    } else {
        colours.knob_rim
    };
    painter.rect(
        cap,
        2,
        colour(colours.ear_centre),
        Stroke::new(1.0, colour(rim)),
        egui::StrokeKind::Inside,
    );
    painter.hline(
        cap.x_range(),
        cap_y,
        Stroke::new(1.0, colour(colours.edge_dark)),
    );
    command
}

fn stereo_meter(ui: &mut Ui, column: &MixerColumn, hold: StereoPeakHold, width: f32) {
    let height = FADER_SIZE.y - CLIP_LED.y - 2.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let [left, right] = column.peak_db;
    response.widget_info(|| {
        WidgetInfo::labeled(
            Role::Meter,
            true,
            format!("{} {left:.1} / {right:.1} dB", column.name),
        )
    });
    let colours = palette();
    let painter = ui.painter();
    for side in 0..2 {
        let left_edge = rect.left() + side as f32 * (METER_BAR_WIDTH + METER_GAP);
        let bar = Rect::from_min_size(pos2(left_edge, rect.top()), vec2(METER_BAR_WIDTH, height));
        painter.rect_filled(bar, 1, colour(colours.led_off));
        let db = column.peak_db[side];
        let lit = if db >= LADDER_RED_DB {
            colours.red
        } else if db >= LADDER_YELLOW_DB {
            colours.yellow
        } else {
            colours.green
        };
        let top = bar.bottom() - bar.height() * fader_position(db);
        painter.rect_filled(
            Rect::from_min_max(pos2(bar.left(), top), bar.max),
            1,
            colour(lit),
        );
        let hold_y = bar.bottom() - bar.height() * fader_position(hold.held_db()[side]);
        painter.hline(
            bar.x_range(),
            hold_y,
            Stroke::new(2.0, colour(colours.text)),
        );
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

/// M and S; S is drawn dimmer-lit on a column that sounds only through solo in place.
fn mute_and_solo(ui: &mut Ui, column: &MixerColumn) -> Option<Command> {
    if column.target == MixerTarget::Master {
        return None;
    }
    let colours = palette();
    let mut mute = None;
    let mut solo = None;
    ui.horizontal(|ui| {
        ui.add_space((COLUMN_WIDTH - 2.0 * SWITCH_SIZE.x - 4.0) / 2.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        let response =
            ui.add(lit_button("M", column.mute, colour(colours.yellow)).min_size(SWITCH_SIZE));
        response.widget_info(|| {
            WidgetInfo::labeled(Role::Button, true, format!("Mute {}", column.name))
        });
        if response.clicked() {
            mute = Some(!column.mute);
        }
        let response = if column.audibility == Audibility::SoloInPlace && !column.solo {
            let (r, g, b) = colours.green;
            let dim = Color32::from_rgba_unmultiplied(r, g, b, SOLO_IN_PLACE_ALPHA);
            ui.add(
                egui::Button::new(RichText::new("S").strong())
                    .fill(dim)
                    .stroke(Stroke::new(1.0, colour(colours.green)))
                    .min_size(SWITCH_SIZE),
            )
        } else {
            ui.add(lit_button("S", column.solo, colour(colours.green)).min_size(SWITCH_SIZE))
        };
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
    let modal = Modal::new(Id::unique("render_dialog")).show(ctx, |ui| {
        Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(DIALOG_WIDTH);
            ui.label(RichText::new("Render").size(18.0).strong());
            ui.add_enabled_ui(!running, |ui| {
                egui::Grid::new("render_fields")
                    .num_columns(2)
                    .spacing(vec2(12.0, 8.0))
                    .show(ui, |ui| {
                        ui.label("Range");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut draft.range, RenderRange::Project, "Project");
                            ui.add_enabled_ui(has_loop, |ui| {
                                ui.radio_value(&mut draft.range, RenderRange::Loop, "Loop");
                            })
                            .response
                            .on_disabled_hover_text("no loop region");
                        });
                        ui.end_row();
                        ui.label("Bits");
                        ui.horizontal(|ui| {
                            for (bits, text) in BITS {
                                ui.radio_value(&mut draft.bits, bits, text);
                            }
                        });
                        ui.end_row();
                        let label = ui.label("Name");
                        ui.add(TextEdit::singleline(&mut draft.name).desired_width(240.0))
                            .labelled_by(label.id);
                        ui.end_row();
                    });
            });
            ui.add_space(8.0);
            commands.extend(render_state(ui, &state));
            ui.add_space(8.0);
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
    });
    if modal.should_close() {
        open = false;
    }
    (commands, open)
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
                let bar = ProgressBar::new(*progress as f32)
                    .show_percentage()
                    .desired_width(DIALOG_WIDTH - 90.0);
                ui.add(bar);
                cancel = ui.add(outlined_button("Cancel")).clicked();
            });
            cancel.then_some(Command::RenderCancel)
        }
        RenderView::Done(file) => {
            ui.label(RichText::new(format!("rendered {file}")).color(colour(colours.green)));
            None
        }
        RenderView::Failed(error) => {
            ui.label(RichText::new(error).color(colour(colours.red)).strong());
            None
        }
        RenderView::Cancelled => {
            ui.label(muted_text("render cancelled"));
            None
        }
    }
}
