use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Button, Color32, ComboBox, CursorIcon, DragValue, EventFilter, Frame, Id, Key, Margin,
    Modifiers, Painter, Popup, Rect, Response, RichText, Role, ScrollArea, Sense, SetOpenCommand,
    Stroke, TextEdit, TextStyle, TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText, pos2, vec2,
};
use serde_json::Value;
use xguitare_core::control::TransportAction;
use xguitare_core::project::LoopRegion;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::mixer_model::ShownRack;
use crate::model::{
    Command, EAR_WIDTH, Feedback, KeyEvent, LADDER_RED_DB, LADDER_YELLOW_DB, METER_FLOOR_DB,
    Message, Snapshot, TrackEdit, knob_origin, port_choices,
};
use crate::project_model::{
    EDGE_GRAB_PX, GridLine, ItemBlock, ItemEdge, ItemHit, ItemKey, ItemKeyPress, ItemNodeAction,
    ItemSelection, LoopEdge, MainView, MoveDrag, MovePointer, MovePreview, Tempo, TimelineView,
    TrackRow, TransportView, TrimDrag, TrimPreview, bars_beats_text, dragged_loop, grid_lines,
    grid_step, hit_test, item_blocks, item_name, item_node_action, items_in_box, move_preview,
    nearest_line, nudge_command, parse_meter, project_key_to_command, project_tempo,
    recording_span, sample_rate, snap_to_beat, take_names, time_text, track_colour, track_rows,
    track_step_command, transport_view, trim_preview, trim_step_command, visible_stretches,
    waveform_columns,
};
use crate::rack::muted_text;
use crate::theme::{
    self, FOCUS_RING_GAP, FOCUS_RING_WIDTH, RADIUS_CONTROL, Rgb, UNIT_EDGE_WIDTH, colour, palette,
    translucent,
};
use crate::widgets::{
    self, KnobSize, StyledButton, outlined_button, primary_button, switched_button,
};

pub const TRACK_LIST_WIDTH: f32 = 400.0;
pub const TIMELINE_HEIGHT: f32 = 360.0;
const TRACK_ROW_HEIGHT: f32 = 72.0;
const RULER_HEIGHT: f32 = 26.0;
const TRANSPORT_HEIGHT: f32 = 44.0;
const TRANSPORT_BUTTON: egui::Vec2 = egui::Vec2::new(44.0, 32.0);
const PANEL_BUTTON: egui::Vec2 = egui::Vec2::new(56.0, 32.0);
const VIEWS: [(&str, MainView); 3] = [
    ("Rack", MainView::Rack),
    ("Arrange", MainView::Arrange),
    ("Mixer", MainView::Mixer),
];
/// The transport faceplate: an ear and a gap at each side.
const TRANSPORT_FACE_MARGIN: Margin = Margin {
    left: EAR_WIDTH as i8 + 6,
    right: EAR_WIDTH as i8 + 6,
    top: 4,
    bottom: 4,
};
/// The space between the transport's groups of controls.
const GROUP_GAP: f32 = theme::SPACE_S;
const LCD_MARGIN: Margin = Margin::symmetric(8, 0);
const TAKE_DASH: f32 = 4.0;
const TAKE_GAP: f32 = 3.0;
const NUMBER_WIDTH: f32 = 20.0;
const LEVEL_BAR_HEIGHT: f32 = 4.0;
const LOOP_BAND_HEIGHT: f32 = 3.0;
const VIEW_BUTTON: egui::Vec2 = egui::Vec2::new(0.0, 32.0);
/// The Track header's name, switches, input and level bar column.
const HEADER_WIDTH: f32 = 168.0;
const SWITCH_SIZE: egui::Vec2 = egui::Vec2::new(26.0, 24.0);
const SWITCH_GAP: f32 = 3.0;
const KNOB_COLUMN_WIDTH: f32 = 64.0;
const SWATCH_WIDTH: f32 = 5.0;
const ITEM_INSET: f32 = 6.0;
/// The `well` gap around an Item that lies over another, so that its edge reads against the plate pieces beside it.
const COVER_GAP: f32 = 1.0;
/// The fewest characters of an Item's name worth showing on a plate piece; a narrower piece shows no name.
const MIN_NAME_CHARS: usize = 4;
/// The mark after the active Take in an Item's menu.
const TAKE_MARK: &str = "✔";
/// Where a Take's waveform starts below its plate's top: under the top edge and the Take's name.
const WAVE_TOP: f32 = UNIT_EDGE_WIDTH + theme::SPACE_XS + theme::SPACE_L;
/// The loop region's lit tint over the lanes (project mockup: loop overlay, 13 %).
const LOOP_ALPHA_ON: u8 = 33;
const LOOP_ALPHA_OFF: u8 = 16;
const SELECTED_OUTLINE: f32 = 2.0;
/// A selected Item's corners: this much thicker than its ring, and this long along each side.
const SELECTED_CORNER_WIDTH: f32 = 2.0 * SELECTED_OUTLINE;
const SELECTED_CORNER_LENGTH: f32 = theme::SPACE_S;
const SNAP_BUTTON_WIDTH: f32 = 56.0;
/// The selection box's outline, on a [`HALO_WIDTH`] halo each side.
const BOX_EDGE_WIDTH: f32 = 1.0;
const PLAYHEAD_WIDTH: f32 = 2.0;
/// The `well` margin each side of the playhead and the loop edges down a lane.
const HALO_WIDTH: f32 = 1.0;
/// The Take plate's border, inside its rect.
const TAKE_EDGE_WIDTH: f32 = 1.0;
const VOLUME_DB: (f64, f64) = (-60.0, 6.0);
pub(crate) const PAN: (f64, f64) = (-1.0, 1.0);
const NO_INPUT: &str = "no input";
/// After the Track name on the Take being recorded: the Daemon names its file only when recording stops.
const RECORDING: &str = "recording";
const NO_TRACKS: &str = "No Tracks yet: Add Track";
const COUNT_IN_BARS: [u32; 3] = [0, 1, 2];
const COUNT_IN_WIDTH: f32 = 76.0;
const METER_WIDTH: f32 = 36.0;
const AUTOSAVE_PROMPT: &str = "A newer autosave exists";

/// An edge of the loop region being dragged: the region when the drag began and as it is now.
#[derive(Debug, Clone, Copy)]
pub struct LoopDrag {
    edge: LoopEdge,
    from: LoopRegion,
    now: LoopRegion,
}

/// A drag on the lanes: the selected Items being moved, an Item's edge being trimmed, or a selection box from one corner (a timeline frame and a screen y) to the other.
#[derive(Debug, Clone)]
enum LaneDrag {
    Move {
        drag: MoveDrag,
        preview: MovePreview,
    },
    Trim {
        drag: TrimDrag,
        preview: Option<TrimPreview>,
    },
    Box {
        from: (f64, f32),
        to: (f64, f32),
    },
}

/// The Track and timeline view's own state: the visible stretch of the timeline, a loop edge or lane drag under way, the selected Items and the Snap switch.
#[derive(Debug)]
pub struct ProjectView {
    pub timeline: Option<TimelineView>,
    pub loop_drag: Option<LoopDrag>,
    /// Where the Take being recorded began ([`recording_from`]).
    pub recording_from: Option<u64>,
    pub selection: ItemSelection,
    pub snap: bool,
    /// The frame of a split sent whose halves `items` has not shown yet.
    pub split_at: Option<u64>,
    /// A duplicate or paste was sent: the Items new in the next `items` become the selection.
    pub select_new: bool,
    /// What the lane menu opened on: the Item right-clicked, or none on the empty lane, with the timeline frame and the Track row.
    menu_at: Option<(Option<u64>, u64, usize)>,
    lane_drag: Option<LaneDrag>,
    /// The Item ids of the `items` last drawn.
    seen: Vec<u64>,
}

impl ProjectView {
    pub fn new(snap: bool) -> ProjectView {
        ProjectView {
            timeline: None,
            loop_drag: None,
            recording_from: None,
            selection: ItemSelection::default(),
            snap,
            split_at: None,
            select_new: false,
            menu_at: None,
            lane_drag: None,
            seen: Vec::new(),
        }
    }
}

/// Play, Stop, Record (a second click stops recording) with the count-in beats left, the view switch `Rack`, `Arrange`, `Mixer` (`shown` lit) and `Render`, Loop, Click with its count-in in bars, tempo, meter, then the position, left to right on one row. A meter that is no `<beats>/<note>` goes to `feedback`.
pub fn transport_bar(
    ui: &mut Ui,
    snapshot: &Snapshot,
    shown: MainView,
    (feedback, now): (&mut Feedback, Instant),
) -> Vec<Command> {
    let Some(status) = snapshot.status.as_ref() else {
        return Vec::new();
    };
    let (Some(transport), Some(tempo)) = (transport_view(status), project_tempo(status)) else {
        return Vec::new();
    };
    let colours = palette();
    let face = Frame::new()
        .fill(colour(colours.panel))
        .inner_margin(TRANSPORT_FACE_MARGIN)
        .show(ui, |ui| {
            ui.set_min_height(TRANSPORT_HEIGHT);
            ui.set_min_width(ui.available_width());
            transport_row(ui, status, &transport, (shown, tempo), (feedback, now))
        });
    let painter = ui.painter();
    let rect = face.response.rect;
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, colour(colours.hover)),
    );
    widgets::rack_ears(painter, rect, None);
    face.inner
}

/// The transport faceplate's controls, wrapping onto a second row where the window is too narrow for one.
fn transport_row(
    ui: &mut Ui,
    status: &Value,
    transport: &TransportView,
    (shown, tempo): (MainView, Tempo),
    (feedback, now): (&mut Feedback, Instant),
) -> Vec<Command> {
    let mut commands = Vec::new();
    let colours = palette();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SPACE_XS;
        let buttons = [
            (
                "▶ Play",
                transport.play_lit(),
                colours.go,
                TransportAction::Play,
            ),
            ("■ Stop", false, colours.text, TransportAction::Stop),
            (
                "● Record",
                transport.record_lit(),
                colours.danger,
                if transport.record_lit() {
                    TransportAction::Stop
                } else {
                    TransportAction::Record
                },
            ),
        ];
        for (text, lit, lit_colour, action) in buttons {
            if ui.add(lit_button(text, lit, lit_colour)).clicked() {
                commands.push(Command::Transport(action));
            }
        }
        if let Some(count_in) = transport.count_in_text() {
            ui.label(
                RichText::new(count_in)
                    .color(colour(colours.danger_text))
                    .strong(),
            );
        }
        ui.add_space(GROUP_GAP);
        commands.extend(view_switch(ui, shown));
        ui.add_space(GROUP_GAP);
        if ui
            .add(outlined_button("Render").min_size(PANEL_BUTTON))
            .clicked()
        {
            commands.push(Command::OpenRenderDialog);
        }
        ui.add_space(GROUP_GAP);
        let loop_on = transport.loop_region.on;
        if ui.add(lit_button("Loop", loop_on, colours.lit)).clicked() {
            commands.extend(project_key_to_command(KeyEvent::R, status, &[]));
        }
        if ui
            .add(lit_button("Click", transport.click, colours.lit))
            .clicked()
        {
            commands.push(Command::Click {
                on: Some(!transport.click),
                count_in: None,
            });
        }
        commands.extend(count_in_picker(ui, transport.count_in));
        ui.add_space(GROUP_GAP);
        commands.extend(tempo_fields(ui, tempo, (feedback, now)));
        ui.add_space(GROUP_GAP);
        position_readout(ui, transport, sample_rate(status), tempo);
        if let Some(warning) = transport.disk_warning() {
            ui.label(
                RichText::new(warning)
                    .color(colour(colours.danger_text))
                    .strong(),
            );
        }
    });
    commands
}

fn view_switch(ui: &mut Ui, shown: MainView) -> Option<Command> {
    let mut chosen = None;
    for (text, view) in VIEWS {
        let button = lit_button(text, view == shown, palette().lit).min_size(VIEW_BUTTON);
        if ui.add(button).clicked() {
            chosen = Some(Command::ShowView(view));
        }
    }
    chosen
}

/// A transport or Track switch: filled in `lit_colour` with dark text while lit.
pub(crate) fn lit_button(text: &str, lit: bool, lit_colour: Rgb) -> StyledButton {
    switched_button(text, lit, lit_colour).min_size(TRANSPORT_BUTTON)
}

fn count_in_text(bars: u32) -> String {
    match bars {
        0 => "no count-in".to_string(),
        bars => format!("count-in {bars}"),
    }
}

fn count_in_picker(ui: &mut Ui, count_in: u32) -> Option<Command> {
    let mut chosen = None;
    let combo = ComboBox::from_id_salt("count_in")
        .width(COUNT_IN_WIDTH)
        .selected_text(count_in_text(count_in))
        .show_ui(ui, |ui| {
            for bars in COUNT_IN_BARS {
                if ui
                    .selectable_label(bars == count_in, count_in_text(bars))
                    .clicked()
                {
                    chosen = Some(bars);
                }
            }
        });
    combo
        .response
        .widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, "Count-in"));
    chosen.map(|bars| Command::Click {
        on: None,
        count_in: Some(bars),
    })
}

/// The tempo, a field reading `120.0 bpm`, is sent when typed or when a drag ends; the meter, typed as `3/4`, on Enter.
fn tempo_fields(
    ui: &mut Ui,
    tempo: Tempo,
    (feedback, now): (&mut Feedback, Instant),
) -> Vec<Command> {
    let mut commands = Vec::new();
    let (bpm, field) = released_value(ui, Id::unique("tempo"), tempo.bpm, |bpm| {
        DragValue::new(bpm)
            .range(20.0..=300.0)
            .speed(0.5)
            .fixed_decimals(1)
            .suffix(" bpm")
    });
    field.widget_info(|| WidgetInfo::labeled(Role::SpinButton, true, "Tempo"));
    if let Some(bpm) = bpm {
        commands.push(Command::Tempo {
            bpm: Some(bpm),
            meter: None,
        });
    }
    let shown = format!("{}/{}", tempo.beats_per_bar, tempo.note);
    if let Some(text) = edited_text(ui, Id::unique("meter"), &shown, METER_WIDTH, "Meter") {
        match parse_meter(&text) {
            Some(meter) => commands.push(Command::Tempo {
                bpm: None,
                meter: Some(meter),
            }),
            None => feedback.show(
                Message::Error {
                    title: "Could not set the meter",
                    detail: format!("'{text}' is not a meter like 3/4"),
                },
                now,
            ),
        }
    }
    commands
}

/// A number field that shows its own value while dragged and returns the value once the drag ends or a typed value is entered with Enter or focus loss.
pub(crate) fn released_value(
    ui: &mut Ui,
    id: Id,
    value: f64,
    field: impl FnOnce(&mut f64) -> DragValue<'_>,
) -> (Option<f64>, Response) {
    let mut shown = ui.data(|data| data.get_temp::<f64>(id)).unwrap_or(value);
    let response = ui.add(field(&mut shown).update_while_editing(false));
    if response.dragged() {
        ui.data_mut(|data| data.insert_temp(id, shown));
        return (None, response);
    }
    ui.data_mut(|data| data.remove_temp::<f64>(id));
    let done = response.drag_stopped() || response.changed();
    (done.then_some(shown), response)
}

/// A text field that holds its edit while focused and returns it on Enter.
fn edited_text(ui: &mut Ui, id: Id, shown: &str, width: f32, name: &str) -> Option<String> {
    let draft = ui.data_mut(|data| data.remove_temp::<String>(id));
    let mut text = draft.unwrap_or_else(|| shown.to_string());
    let edit = ui.add(widgets::text_field(
        TextEdit::singleline(&mut text).id(id).desired_width(width),
    ));
    edit.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, name));
    if edit.has_focus() {
        ui.data_mut(|data| data.insert_temp(id, text));
        return None;
    }
    let entered = edit.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    (entered && text.trim() != shown).then(|| text.trim().to_string())
}

fn position_readout(ui: &mut Ui, transport: &TransportView, rate: f64, tempo: Tempo) {
    let bars = bars_beats_text(transport.frames, rate, tempo);
    let time = time_text(transport.frames, rate);
    let colours = palette();
    let bars = RichText::new(bars)
        .font(theme::timecode())
        .color(colour(colours.text));
    let time = RichText::new(time)
        .font(theme::value())
        .color(colour(colours.lit));
    let width = [&bars, &time]
        .map(|text| {
            WidgetText::from((*text).clone())
                .into_galley(
                    ui,
                    Some(TextWrapMode::Extend),
                    f32::INFINITY,
                    TextStyle::Body,
                )
                .size()
                .x
        })
        .iter()
        .sum::<f32>()
        + ui.spacing().item_spacing.x
        + f32::from(LCD_MARGIN.left + LCD_MARGIN.right);
    if ui.available_width() < width {
        ui.end_row();
    }
    Frame::new()
        .fill(colour(colours.well))
        .corner_radius(RADIUS_CONTROL)
        .inner_margin(LCD_MARGIN)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(bars);
                ui.label(time);
            });
        });
}

/// The ruler with the loop region, then one row per Track: its controls on the left, its lane of Items on the right.
pub fn tracks_view(ui: &mut Ui, snapshot: &Snapshot, view: &mut ProjectView) -> Vec<Command> {
    let mut commands = Vec::new();
    let Some(status) = snapshot.status.as_ref() else {
        return commands;
    };
    let (Some(transport), Some(tempo)) = (transport_view(status), project_tempo(status)) else {
        return commands;
    };
    let rate = sample_rate(status);
    let items = snapshot.items.as_ref().map(item_blocks).unwrap_or_default();
    let ids: Vec<u64> = items.iter().map(|item| item.id).collect();
    if ids != view.seen {
        view.selection
            .follow(&view.seen, &items, view.split_at.take());
        if std::mem::take(&mut view.select_new) {
            view.selection.select_new(&view.seen, &items);
        }
        view.seen = ids;
    }
    let face = ui.available_rect_before_wrap();
    let full = face.shrink2(vec2(EAR_WIDTH, 0.0));
    let ruler = Rect::from_min_max(
        pos2(full.left() + TRACK_LIST_WIDTH, full.top()),
        pos2(full.right(), full.top() + RULER_HEIGHT),
    );
    let timeline = *view.timeline.get_or_insert_with(|| TimelineView::new(rate));
    let lanes = Rect::from_min_max(pos2(ruler.left(), ruler.bottom()), full.right_bottom());
    let timeline = navigated(ui, timeline, lanes.union(ruler));
    view.timeline = Some(timeline);
    let scene = Scene {
        timeline,
        grid: grid_lines(timeline, ruler.width(), rate, tempo),
        loop_region: view
            .loop_drag
            .map_or(transport.loop_region, |drag| drag.now),
        playhead: transport.frames,
        recording: view
            .recording_from
            .map(|from| recording_span(from, &transport)),
        selected: view.selection.clone(),
    };
    commands.extend(track_list_head(ui, full, view));
    commands.extend(ruler_strip(
        ui,
        ruler,
        &scene,
        rate,
        tempo,
        view,
        transport.loop_region,
    ));
    let rows = track_rows(status);
    let ports = snapshot
        .ports
        .as_ref()
        .map(|ports| port_choices(ports).capture)
        .unwrap_or_default();
    let body = Rect::from_min_max(pos2(full.left(), ruler.bottom()), full.right_bottom());
    ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
        if rows.is_empty() {
            ui.label(muted_text(NO_TRACKS));
        }
        ScrollArea::vertical().id_salt("tracks").show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut lanes = Vec::new();
            let shown = trim_shown(&items, view.lane_drag.as_ref());
            for row in &rows {
                commands.extend(track(
                    ui,
                    row,
                    &shown,
                    &snapshot.peaks,
                    &ports,
                    &scene,
                    &mut lanes,
                ));
            }
            let menu = MenuContext {
                payload: snapshot.items.as_ref(),
                clipboard: status["clipboard"].as_u64().unwrap_or_default() > 0,
            };
            commands.extend(item_nodes(
                ui,
                (&items, &lanes),
                &scene,
                &menu,
                view,
                (rate, tempo),
            ));
            commands.extend(lane_pointer(ui, &lanes, &items, &scene, view));
            commands.extend(lane_menus(&lanes, &items, &scene, &menu, view));
            paint_lane_drag(ui, &lanes, &items, &scene, view);
        });
    });
    widgets::rack_ears(ui.painter(), face, None);
    ui.allocate_rect(face, Sense::hover());
    commands
}

/// The Items as drawn: during a trim, the trimmed Item at its preview edges.
fn trim_shown(items: &[ItemBlock], drag: Option<&LaneDrag>) -> Vec<ItemBlock> {
    let Some(LaneDrag::Trim {
        preview: Some(preview),
        ..
    }) = drag
    else {
        return items.to_vec();
    };
    items
        .iter()
        .map(|item| {
            if item.id == preview.id {
                preview.apply(item)
            } else {
                item.clone()
            }
        })
        .collect()
}

/// `+ Add Track` and the Snap switch left of the ruler.
fn track_list_head(ui: &mut Ui, full: Rect, view: &mut ProjectView) -> Option<Command> {
    let mut command = None;
    let strip = TRACK_LIST_WIDTH - 8.0;
    let add = Rect::from_min_size(
        full.min,
        vec2(strip - SNAP_BUTTON_WIDTH - theme::SPACE_XS, RULER_HEIGHT),
    );
    if ui.put(add, outlined_button("+ Add Track")).clicked() {
        command = Some(Command::TrackAdd);
    }
    let snap = Rect::from_min_size(
        pos2(full.left() + strip - SNAP_BUTTON_WIDTH, full.top()),
        vec2(SNAP_BUTTON_WIDTH, RULER_HEIGHT),
    );
    if ui
        .put(snap, switched_button("Snap", view.snap, palette().lit))
        .clicked()
    {
        view.snap = !view.snap;
    }
    command
}

/// Ctrl+wheel zooms about the pointer, a sideways wheel (or Shift+wheel) scrolls, while the pointer is over the timeline.
fn navigated(ui: &Ui, timeline: TimelineView, area: Rect) -> TimelineView {
    if !ui.rect_contains_pointer(area) {
        return timeline;
    }
    let (zoom, scroll_x, pointer) = ui.input(|input| {
        (
            input.zoom_delta(),
            input.smooth_scroll_delta.x,
            input.pointer.hover_pos(),
        )
    });
    let mut timeline = timeline;
    if (zoom - 1.0).abs() > f32::EPSILON
        && let Some(pointer) = pointer
    {
        timeline = timeline.zoomed(f64::from(zoom), pointer.x - area.left());
    }
    if scroll_x.abs() > f32::EPSILON {
        timeline = timeline.scrolled(-scroll_x);
    }
    timeline
}

struct Scene {
    timeline: TimelineView,
    grid: Vec<GridLine>,
    loop_region: LoopRegion,
    playhead: u64,
    /// The frames the Take being recorded covers so far.
    recording: Option<(u64, u64)>,
    selected: ItemSelection,
}

impl Scene {
    fn x(&self, rect: Rect, frame: u64) -> f32 {
        rect.left() + self.timeline.x_of(frame as f64)
    }

    /// The plate of an Item `length` frames long from `position` on `lane`, cut to the lane.
    fn plate(&self, lane: Rect, position: u64, length: u64) -> Rect {
        Rect::from_x_y_ranges(
            self.x(lane, position)..=self.x(lane, position.saturating_add(length)),
            lane.top() + ITEM_INSET..=lane.bottom() - ITEM_INSET,
        )
        .intersect(lane)
    }

    fn paint_grid(&self, painter: &Painter, rect: Rect) {
        let colours = palette();
        for line in &self.grid {
            let x = rect.left() + self.timeline.x_of(line.frame);
            let shade = if line.beat == 1 {
                colours.grid_bar
            } else {
                colours.grid_beat
            };
            painter.vline(x, rect.y_range(), Stroke::new(1.0, colour(shade)));
        }
    }

    fn paint_loop(&self, painter: &Painter, rect: Rect) {
        if self.loop_region.start >= self.loop_region.end {
            return;
        }
        let colours = palette();
        let alpha = if self.loop_region.on {
            LOOP_ALPHA_ON
        } else {
            LOOP_ALPHA_OFF
        };
        let span = Rect::from_x_y_ranges(
            self.x(rect, self.loop_region.start)..=self.x(rect, self.loop_region.end),
            rect.y_range(),
        );
        painter.rect_filled(span, 0, translucent(colours.lit, alpha));
    }

    /// The loop region along the ruler's foot: lit while the loop is on, else in the boundary grey.
    fn paint_loop_band(&self, painter: &Painter, ruler: Rect) {
        if self.loop_region.start >= self.loop_region.end {
            return;
        }
        let colours = palette();
        let band = if self.loop_region.on {
            colours.lit
        } else {
            colours.boundary
        };
        let span = Rect::from_x_y_ranges(
            self.x(ruler, self.loop_region.start)..=self.x(ruler, self.loop_region.end),
            ruler.bottom() - LOOP_BAND_HEIGHT..=ruler.bottom(),
        );
        painter.rect_filled(span, 0, colour(band));
    }

    /// The loop region's start and end down a lane, over its Takes: lit while the loop is on, else in the boundary grey.
    fn paint_loop_edges(&self, painter: &Painter, lane: Rect) {
        if self.loop_region.start >= self.loop_region.end {
            return;
        }
        let colours = palette();
        let edge = if self.loop_region.on {
            colours.lit
        } else {
            colours.boundary
        };
        for frame in [self.loop_region.start, self.loop_region.end] {
            paint_haloed_line(painter, self.x(lane, frame), lane, 1.0, edge);
        }
    }

    /// The playhead down a lane, haloed so that it stands out over any waveform.
    fn paint_lane_playhead(&self, painter: &Painter, lane: Rect) {
        let x = self.x(lane, self.playhead);
        paint_haloed_line(painter, x, lane, PLAYHEAD_WIDTH, palette().danger);
    }

    fn paint_playhead(&self, painter: &Painter, rect: Rect) {
        let x = self.x(rect, self.playhead);
        painter.vline(
            x,
            rect.y_range(),
            Stroke::new(PLAYHEAD_WIDTH, colour(palette().danger)),
        );
    }
}

/// Click to locate; drag a loop edge to move it, snapped to the beat, sent as `loop` on release.
fn ruler_strip(
    ui: &mut Ui,
    ruler: Rect,
    scene: &Scene,
    rate: f64,
    tempo: Tempo,
    view: &mut ProjectView,
    loop_region: LoopRegion,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let response = ui.interact(ruler, Id::unique("ruler"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Slider, true, "Timeline ruler"));
    if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let frame = scene.timeline.frame_at(pointer.x - ruler.left());
        commands.push(Command::Locate(frame.round() as u64));
    }
    let painter = ui.painter_at(ruler);
    let colours = palette();
    painter.rect_filled(ruler, 0, colour(colours.ruler));
    scene.paint_loop_band(&painter, ruler);
    for line in scene.grid.iter().filter(|line| line.beat == 1) {
        painter.vline(
            ruler.left() + scene.timeline.x_of(line.frame),
            ruler.y_range(),
            Stroke::new(1.0, colour(colours.grid_bar)),
        );
        painter.text(
            pos2(
                ruler.left() + scene.timeline.x_of(line.frame) + 3.0,
                ruler.top() + 2.0,
            ),
            Align2::LEFT_TOP,
            line.bar.to_string(),
            theme::legend(),
            colour(colours.muted),
        );
    }
    scene.paint_playhead(&painter, ruler);
    if scene.loop_region.start >= scene.loop_region.end {
        return commands;
    }
    for (edge, frame, name) in [
        (LoopEdge::Start, scene.loop_region.start, "Loop start"),
        (LoopEdge::End, scene.loop_region.end, "Loop end"),
    ] {
        let x = scene.x(ruler, frame);
        let grab = Rect::from_x_y_ranges(x - EDGE_GRAB_PX..=x + EDGE_GRAB_PX, ruler.y_range());
        let handle = ui
            .interact(grab, Id::unique(("loop_edge", name)), Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal);
        handle.widget_info(|| WidgetInfo::labeled(Role::Slider, true, name));
        painter.vline(x, ruler.y_range(), Stroke::new(2.0, colour(colours.lit)));
        commands.extend(loop_edge_drag(
            &handle,
            edge,
            ruler,
            scene,
            (rate, tempo),
            view,
            loop_region,
        ));
    }
    commands
}

fn loop_edge_drag(
    handle: &Response,
    edge: LoopEdge,
    ruler: Rect,
    scene: &Scene,
    (rate, tempo): (f64, Tempo),
    view: &mut ProjectView,
    loop_region: LoopRegion,
) -> Option<Command> {
    if handle.drag_started() {
        view.loop_drag = Some(LoopDrag {
            edge,
            from: loop_region,
            now: loop_region,
        });
    }
    let drag = view.loop_drag.as_mut().filter(|drag| drag.edge == edge)?;
    if handle.dragged()
        && let Some(pointer) = handle.interact_pointer_pos()
    {
        let frame = snap_to_beat(
            scene.timeline.frame_at(pointer.x - ruler.left()),
            rate,
            tempo,
        );
        drag.now = dragged_loop(drag.from, edge, frame);
    }
    if !handle.drag_stopped() {
        return None;
    }
    let region = drag.now;
    view.loop_drag = None;
    (region != loop_region).then_some(Command::Loop(region))
}

/// A Track's lane as drawn this frame: its row, its Track's id and name, where it is, its pointer response and the command a click on it sends.
struct Lane {
    row: usize,
    track_id: u64,
    name: String,
    rect: Rect,
    response: Response,
    selects: Command,
}

/// A click anywhere on the row or its lane that no control takes selects the Track; the lane, pushed to `lanes`, takes the pointer for the Items.
fn track(
    ui: &mut Ui,
    row: &TrackRow,
    items: &[ItemBlock],
    peaks: &HashMap<String, Arc<[i8]>>,
    ports: &[String],
    scene: &Scene,
    lanes: &mut Vec<Lane>,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, TRACK_ROW_HEIGHT), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Row, true, format!("Track {}", row.name)));
    let selects = if row.selected {
        Command::ShowRack(ShownRack::Track)
    } else {
        Command::TrackSelect(row.index)
    };
    if response.clicked() {
        commands.push(selects.clone());
    }
    let colours = palette();
    let list = Rect::from_min_size(rect.min, vec2(TRACK_LIST_WIDTH, rect.height()));
    let lane = Rect::from_min_max(pos2(list.right(), rect.top()), rect.max);
    let fill = if row.selected {
        colours.panel_open
    } else {
        colours.panel
    };
    let tab = track_colour(row.index);
    let painter = ui.painter_at(rect);
    painter.rect_filled(list, 0, colour(fill));
    let swatch = Rect::from_min_size(list.min, vec2(SWATCH_WIDTH, list.height()));
    painter.rect_filled(swatch, 0, colour(tab));
    if row.selected {
        painter.rect_stroke(
            list.shrink(SELECTED_OUTLINE / 2.0),
            0,
            Stroke::new(SELECTED_OUTLINE, colour(colours.text)),
            egui::StrokeKind::Inside,
        );
    }
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, colour(colours.well)),
    );
    paint_lane(&ui.painter_at(lane), lane, row, items, peaks, scene);
    let lane_response = ui.interact(
        lane,
        Id::unique(("lane", row.id)),
        Sense::CLICK | Sense::DRAG,
    );
    lane_response
        .widget_info(|| WidgetInfo::labeled(Role::Group, true, format!("Lane {}", row.name)));
    lanes.push(Lane {
        row: row.index,
        track_id: row.id,
        name: row.name.clone(),
        rect: lane,
        response: lane_response,
        selects,
    });
    let controls = list
        .shrink2(vec2(SWATCH_WIDTH + 2.0, 6.0))
        .translate(vec2(SWATCH_WIDTH, 0.0));
    let mut controls_ui = ui.new_child(UiBuilder::new().max_rect(controls));
    commands.extend(track_controls(&mut controls_ui, row, ports));
    commands
}

fn paint_lane(
    painter: &Painter,
    lane: Rect,
    row: &TrackRow,
    items: &[ItemBlock],
    peaks: &HashMap<String, Arc<[i8]>>,
    scene: &Scene,
) {
    let colours = palette();
    painter.rect_filled(lane, 0, colour(colours.well));
    scene.paint_loop(painter, lane);
    scene.paint_grid(painter, lane);
    let rows = lane.top() + ITEM_INSET..=lane.bottom() - ITEM_INSET;
    let track = track_colour(row.index);
    for item in items.iter().filter(|item| item.track_id == row.id) {
        paint_item(painter, lane, (items, item), peaks, scene, track);
    }
    if row.armed
        && let Some((from, to)) = scene.recording
    {
        let span = Rect::from_x_y_ranges(scene.x(lane, from)..=scene.x(lane, to), rows);
        let name = format!("{} {RECORDING}", row.name);
        paint_recording_take(painter, span.intersect(lane), &name);
    }
    scene.paint_loop_edges(painter, lane);
    scene.paint_lane_playhead(painter, lane);
    for item in items
        .iter()
        .filter(|item| item.track_id == row.id && scene.selected.contains(item.id))
    {
        paint_selection(painter, scene.plate(lane, item.position, item.length));
    }
}

/// The waveform of the Take frames `item` plays, in `ink` over `body`, from its peaks once they have come.
fn paint_item_waveform(
    painter: &Painter,
    (lane, body): (Rect, Rect),
    item: &ItemBlock,
    peaks: &HashMap<String, Arc<[i8]>>,
    scene: &Scene,
    ink: Rgb,
) {
    if let Some(visible) = item.visible_take(scene.timeline, lane.width())
        && let Some(peaks) = peaks.get(&item.file)
    {
        let columns = waveform_columns(peaks, visible.frames, visible.width);
        paint_waveform(painter, body, lane.left() + visible.x, &columns, ink);
    }
}

/// An Item as heard: one plate piece per stretch no later Item covers, each with its Track-colour top edge and its part of the waveform, and the Take's name once, in the first piece wide enough for [`MIN_NAME_CHARS`] of it, cut short with an ellipsis. An Item over another has a [`COVER_GAP`] `well` gap around its plate.
fn paint_item(
    painter: &Painter,
    lane: Rect,
    (items, item): (&[ItemBlock], &ItemBlock),
    peaks: &HashMap<String, Arc<[i8]>>,
    scene: &Scene,
    track: Rgb,
) {
    let covers = items
        .iter()
        .take_while(|lower| lower.id != item.id)
        .any(|lower| {
            lower.track_id == item.track_id
                && lower.position < item.end()
                && item.position < lower.end()
        });
    if covers {
        let plate = scene.plate(lane, item.position, item.length);
        painter.rect_stroke(
            plate,
            RADIUS_CONTROL,
            Stroke::new(COVER_GAP, colour(palette().well)),
            egui::StrokeKind::Outside,
        );
    }
    let mut name = Some(item.label());
    for stretch in visible_stretches(items, item.id) {
        let piece = ItemBlock {
            position: stretch.start,
            offset: item.offset + (stretch.start - item.position),
            length: stretch.end - stretch.start,
            ..item.clone()
        };
        let span = scene.plate(lane, piece.position, piece.length);
        if !span.is_positive() {
            continue;
        }
        let shown = name.take_if(|name| fits_name(painter, span, name));
        paint_take(painter, span, shown.as_deref(), track);
        let inner = span.shrink(TAKE_EDGE_WIDTH);
        paint_item_waveform(
            &painter.with_clip_rect(inner),
            (lane, inner),
            &piece,
            peaks,
            scene,
            track,
        );
    }
}

/// Whether a plate piece `span` holds [`MIN_NAME_CHARS`] of `name` beside its inset.
fn fits_name(painter: &Painter, span: Rect, name: &str) -> bool {
    let start: String = name.chars().take(MIN_NAME_CHARS).collect();
    let width = painter
        .layout_no_wrap(start, theme::legend(), Color32::PLACEHOLDER)
        .size()
        .x;
    span.width() >= width + 2.0 * theme::SPACE_XS
}

/// A selected Item's mark around its plate, over the loop edges and the playhead so that it stays one closed shape: a [`SELECTED_OUTLINE`] `lit` ring a [`HALO_WIDTH`] `well` gap outside the plate with thicker corners, all on a `well` halo, so that it holds against the lane and any plate next to it and reads by its shape as well as its colour.
fn paint_selection(painter: &Painter, plate: Rect) {
    let colours = palette();
    let ring = plate.expand(HALO_WIDTH);
    let corner_band = ring.expand(SELECTED_CORNER_WIDTH);
    let corners = [
        (ring.left_top(), corner_band.left_top()),
        (ring.right_top(), corner_band.right_top()),
        (ring.left_bottom(), corner_band.left_bottom()),
        (ring.right_bottom(), corner_band.right_bottom()),
    ];
    let corner_marks = corners.iter().flat_map(|&(inner, outer)| {
        let along_x = if inner.x < plate.center().x {
            1.0
        } else {
            -1.0
        };
        let along_y = if inner.y < plate.center().y {
            1.0
        } else {
            -1.0
        };
        [
            Rect::from_two_pos(
                outer,
                pos2(inner.x + along_x * SELECTED_CORNER_LENGTH, inner.y),
            ),
            Rect::from_two_pos(
                outer,
                pos2(inner.x, inner.y + along_y * SELECTED_CORNER_LENGTH),
            ),
        ]
    });
    let corner_marks: Vec<Rect> = corner_marks.collect();
    let halo = Stroke::new(
        HALO_WIDTH + SELECTED_OUTLINE + HALO_WIDTH,
        colour(colours.well),
    );
    painter.rect_stroke(plate, RADIUS_CONTROL, halo, egui::StrokeKind::Outside);
    for mark in &corner_marks {
        painter.rect_filled(mark.expand(HALO_WIDTH), 0, colour(colours.well));
    }
    let lit = colour(colours.lit);
    painter.rect_stroke(
        ring,
        RADIUS_CONTROL,
        Stroke::new(SELECTED_OUTLINE, lit),
        egui::StrokeKind::Outside,
    );
    for mark in &corner_marks {
        painter.rect_filled(*mark, 0, lit);
    }
}

/// One focusable node per Item, after every Track header and in time order, named by [`item_name`] with its selected state. Focus leaves the selection alone and rings the plate inside its edge, apart from the selection mark outside it. On a focused Item, Enter selects only it, Ctrl+Enter takes it in or out, ← and → move the selected Items by one grid step, Alt+↑ and Alt+↓ to the Track above or below.
fn item_nodes(
    ui: &Ui,
    (items, lanes): (&[ItemBlock], &[Lane]),
    scene: &Scene,
    menu: &MenuContext,
    view: &mut ProjectView,
    (rate, tempo): (f64, Tempo),
) -> Option<Command> {
    let mut placed: Vec<(&ItemBlock, &Lane)> = items
        .iter()
        .filter_map(|item| {
            let lane = lanes.iter().find(|lane| lane.track_id == item.track_id)?;
            Some((item, lane))
        })
        .collect();
    placed.sort_by_key(|(item, lane)| (item.position, lane.row));
    let track_ids: Vec<u64> = lanes.iter().map(|lane| lane.track_id).collect();
    let mut command = None;
    for (item, lane) in placed {
        let plate = scene.plate(lane.rect, item.position, item.length);
        if !plate.is_positive() {
            continue;
        }
        let inside = plate.shrink(FOCUS_RING_GAP + FOCUS_RING_WIDTH);
        let node = if inside.is_positive() { inside } else { plate };
        let id = item_node_id(item.id);
        let response = ui.interact(node, id, Sense::FOCUSABLE);
        let name = item_name(item, items, &lane.name, rate, tempo);
        response.widget_info(|| WidgetInfo::labeled(Role::ListItem, true, &name));
        let selected = view.selection.contains(item.id);
        ui.ctx()
            .accesskit_node_builder(id, |node| node.set_selected(selected));
        let open_menu = response.has_focus()
            && pressed(ui)
                .into_iter()
                .filter_map(item_key_press)
                .any(|press| item_node_action(press) == Some(ItemNodeAction::Menu));
        let shown = Popup::menu(&response)
            .id(id.with("menu"))
            .open_memory(open_menu.then_some(SetOpenCommand::Bool(true)))
            .show(|ui| item_menu(ui, item, (scene, menu), &view.selection, open_menu));
        command = command.or(shown.and_then(|shown| shown.inner));
        if !response.has_focus() {
            continue;
        }
        let arrows_stay = EventFilter {
            horizontal_arrows: true,
            vertical_arrows: true,
            ..EventFilter::default()
        };
        ui.memory_mut(|memory| memory.set_focus_lock_filter(id, arrows_stay));
        let step = grid_step(scene.timeline, rate, tempo);
        for press in pressed(ui).into_iter().filter_map(item_key_press) {
            match item_node_action(press) {
                Some(ItemNodeAction::Select) => view.selection.pick(item.id, false),
                Some(ItemNodeAction::Toggle) => view.selection.pick(item.id, true),
                Some(ItemNodeAction::Nudge(direction)) => {
                    command = nudge_command(items, &view.selection, direction as f64 * step);
                }
                Some(ItemNodeAction::TrackStep(rows)) => {
                    command = track_step_command(items, &view.selection, &track_ids, rows);
                }
                Some(ItemNodeAction::Trim(edge, direction)) => {
                    command = trim_step_command(item, edge, direction as f64 * step);
                }
                Some(ItemNodeAction::Menu) | None => {}
            }
        }
    }
    command
}

/// A Take in an Item's menu; the active one in `lit` with a `lit` check mark after it, so that it is not told by a fill alone.
fn take_entry(name: &str, active: bool) -> Button<'static> {
    if !active {
        return Button::new(name.to_owned());
    }
    let lit = colour(palette().lit);
    Button::new(RichText::new(name).color(lit)).shortcut_text(RichText::new(TAKE_MARK).color(lit))
}

/// What the Item and lane menus need besides the Items: the `items` payload for the Take names, and whether the clipboard holds anything.
struct MenuContext<'a> {
    payload: Option<&'a Value>,
    clipboard: bool,
}

/// An Item's menu: its Takes with the active one marked (another one chosen is sent as `item_take`), then `Split at playhead`, `Duplicate`, `Copy` and `Delete` for the selected Items, or for this Item alone when it is not selected. With `focus_first` the first entry takes the keyboard focus.
fn item_menu(
    ui: &mut Ui,
    item: &ItemBlock,
    (scene, menu): (&Scene, &MenuContext),
    selection: &ItemSelection,
    focus_first: bool,
) -> Option<Command> {
    let names = menu
        .payload
        .map(|payload| take_names(payload, item.id))
        .unwrap_or_default();
    let mut command = None;
    for (take, name) in names.iter().enumerate() {
        let active = take == item.take;
        let entry = ui.add(take_entry(name, active));
        entry.widget_info(|| WidgetInfo::selected(Role::Button, true, active, name));
        if take == 0 && focus_first {
            entry.request_focus();
        }
        if entry.clicked() {
            command = (take != item.take).then_some(Command::ItemTake { id: item.id, take });
            ui.close();
        }
    }
    ui.separator();
    let ids = if selection.contains(item.id) {
        selection.ids()
    } else {
        vec![item.id]
    };
    let edits = [
        (
            "Split at playhead",
            Command::ItemSplit {
                ids: Some(ids.clone()),
                at: scene.playhead,
            },
        ),
        ("Duplicate", Command::ItemDuplicate(ids.clone())),
        ("Copy", Command::ItemCopy(ids.clone())),
        ("Delete", Command::ItemDelete(ids)),
    ];
    for (label, edit) in edits {
        if ui.button(label).clicked() {
            command = Some(edit);
            ui.close();
        }
    }
    command
}

/// The menu a right-click opens on a lane: [`item_menu`] on an Item, else `Paste` at the frame clicked on that Track, enabled while the clipboard holds Items.
fn lane_menus(
    lanes: &[Lane],
    items: &[ItemBlock],
    scene: &Scene,
    menu: &MenuContext,
    view: &ProjectView,
) -> Option<Command> {
    let mut command = None;
    for lane in lanes {
        lane.response.context_menu(|ui| {
            command = match view.menu_at {
                Some((Some(id), ..)) => items
                    .iter()
                    .find(|item| item.id == id)
                    .and_then(|item| item_menu(ui, item, (scene, menu), &view.selection, false)),
                Some((None, at, row)) => {
                    let paste = ui.add_enabled(menu.clipboard, Button::new("Paste"));
                    paste.clicked().then(|| {
                        ui.close();
                        Command::ItemPaste { track: row, at }
                    })
                }
                None => None,
            };
        });
    }
    command
}

/// The focus id of an Item's node.
pub(crate) fn item_node_id(item: u64) -> Id {
    Id::unique(("item", item))
}

/// A key press a focused Item takes, with Ctrl standing for the command key.
fn item_key_press((key, modifiers): (Key, Modifiers)) -> Option<ItemKeyPress> {
    let key = match key {
        Key::Enter => ItemKey::Enter,
        Key::ArrowLeft => ItemKey::Left,
        Key::ArrowRight => ItemKey::Right,
        Key::ArrowUp => ItemKey::Up,
        Key::ArrowDown => ItemKey::Down,
        Key::F10 => ItemKey::F10,
        Key::Menu => ItemKey::Menu,
        _ => return None,
    };
    Some(ItemKeyPress {
        key,
        ctrl: modifiers.command || modifiers.ctrl,
        shift: modifiers.shift,
        alt: modifiers.alt,
    })
}

/// This frame's key presses with the modifiers each was pressed with.
fn pressed(ui: &Ui) -> Vec<(Key, Modifiers)> {
    ui.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect()
    })
}

/// A click on an Item selects it (Ctrl: in or out), on the empty lane clears the selection (Ctrl: keeps it); both select the lane's Track. A drag from an Item moves the selected Items, sent as `item_move` on release; from within [`EDGE_GRAB_PX`] of its edge it trims that edge, sent as `item_trim`, with the resize cursor over the edge; a drag from the empty lane draws a selection box (Ctrl: adds to the selection).
fn lane_pointer(
    ui: &Ui,
    lanes: &[Lane],
    items: &[ItemBlock],
    scene: &Scene,
    view: &mut ProjectView,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let (modifiers, origin, pointer) = ui.input(|input| {
        (
            input.modifiers,
            input.pointer.press_origin(),
            input.pointer.interact_pos(),
        )
    });
    let toggle = modifiers.command;
    for lane in lanes {
        let response = &lane.response;
        if let Some(at) = response.hover_pos() {
            edge_cursor(
                ui,
                hit_test(
                    items,
                    lane.track_id,
                    scene.timeline,
                    at.x - lane.rect.left(),
                ),
            );
        }
        if response.clicked()
            && let Some(at) = response.interact_pointer_pos()
        {
            match hit_test(
                items,
                lane.track_id,
                scene.timeline,
                at.x - lane.rect.left(),
            )
            .item()
            {
                Some(id) => view.selection.pick(id, toggle),
                None if !toggle => view.selection.clear(),
                None => {}
            }
            commands.push(lane.selects.clone());
        }
        if response.secondary_clicked()
            && let Some(at) = response.interact_pointer_pos()
        {
            let x = at.x - lane.rect.left();
            let hit = hit_test(items, lane.track_id, scene.timeline, x).item();
            if let Some(id) = hit {
                view.selection.grab(id, false);
            }
            view.menu_at = Some((hit, menu_frame(scene, x, view.snap), lane.row));
        }
        if response.drag_started()
            && let Some(origin) = origin
        {
            let x = origin.x - lane.rect.left();
            let frame = scene.timeline.frame_at(x);
            let hit = hit_test(items, lane.track_id, scene.timeline, x);
            view.lane_drag = Some(drag_from(hit, (frame, origin.y), lane.row, view, toggle));
        }
    }
    let Some(lane) = lanes
        .iter()
        .find(|lane| lane.response.dragged() || lane.response.drag_stopped())
    else {
        view.lane_drag = None;
        return commands;
    };
    let snap = view.snap != modifiers.alt;
    let track_ids: Vec<u64> = lanes.iter().map(|lane| lane.track_id).collect();
    if let (Some(drag), Some(at)) = (view.lane_drag.as_mut(), pointer) {
        let frame = scene.timeline.frame_at(at.x - lane.rect.left());
        match drag {
            LaneDrag::Move { drag, preview } => {
                let pointer = MovePointer {
                    frame,
                    row: row_at(lanes, at.y),
                    snap,
                    lock_track: modifiers.shift,
                };
                *preview = move_preview(drag, items, &track_ids, &scene.grid, pointer);
            }
            LaneDrag::Trim { drag, preview } => {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                *preview = trim_preview(drag, items, &scene.grid, frame, snap);
            }
            LaneDrag::Box { to, .. } => *to = (frame, at.y),
        }
    }
    if !lane.response.drag_stopped() {
        return commands;
    }
    let drag = view.lane_drag.take();
    commands.extend(released(drag, lanes, items, view, toggle));
    commands
}

/// What a lane drag does on release: the `item_move` or `item_trim` to send, or the selection box taken into the selection (Ctrl: added).
fn released(
    drag: Option<LaneDrag>,
    lanes: &[Lane],
    items: &[ItemBlock],
    view: &mut ProjectView,
    toggle: bool,
) -> Option<Command> {
    match drag {
        Some(LaneDrag::Move { drag, preview }) => preview.command(&drag),
        Some(LaneDrag::Trim { drag, preview }) => {
            let item = items.iter().find(|item| item.id == drag.id);
            preview
                .zip(item)
                .and_then(|(preview, item)| preview.command(item))
        }
        Some(LaneDrag::Box { from, to }) => {
            let (top, bottom) = (from.1.min(to.1), from.1.max(to.1));
            let tracks: Vec<u64> = lanes
                .iter()
                .filter(|lane| {
                    lane.rect.top() + ITEM_INSET < bottom && top < lane.rect.bottom() - ITEM_INSET
                })
                .map(|lane| lane.track_id)
                .collect();
            let frames = from.0.min(to.0)..from.0.max(to.0);
            view.selection
                .select_box(&items_in_box(items, &tracks, frames), toggle);
            None
        }
        None => None,
    }
}

/// The timeline frame `x` pixels into a lane, on the nearest grid line when `snap`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a non-negative frame far below u64::MAX"
)]
fn menu_frame(scene: &Scene, x: f32, snap: bool) -> u64 {
    let frame = scene.timeline.frame_at(x);
    nearest_line(&scene.grid, frame, snap)
        .unwrap_or(frame)
        .max(0.0)
        .round() as u64
}

/// The resize cursor over an Item's edge.
fn edge_cursor(ui: &Ui, hit: ItemHit) {
    if matches!(hit, ItemHit::StartEdge(_) | ItemHit::EndEdge(_)) {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
}

/// The drag a press on `hit` at a timeline frame and screen y starts on the lane of `row`; an Item pressed is grabbed into the selection.
fn drag_from(
    hit: ItemHit,
    (frame, y): (f64, f32),
    row: usize,
    view: &mut ProjectView,
    toggle: bool,
) -> LaneDrag {
    if let Some(id) = hit.item() {
        view.selection.grab(id, toggle);
    }
    let trim = |id, edge| LaneDrag::Trim {
        drag: TrimDrag::new(id, edge, frame),
        preview: None,
    };
    match hit {
        ItemHit::StartEdge(id) => trim(id, ItemEdge::Start),
        ItemHit::EndEdge(id) => trim(id, ItemEdge::End),
        ItemHit::Body(id) => LaneDrag::Move {
            drag: MoveDrag::new(id, &view.selection, frame, row),
            preview: MovePreview::default(),
        },
        ItemHit::Lane => LaneDrag::Box {
            from: (frame, y),
            to: (frame, y),
        },
    }
}

/// The row of the lane under the screen `y`: the first above it, the last below it.
fn row_at(lanes: &[Lane], y: f32) -> usize {
    lanes
        .iter()
        .find(|lane| y < lane.rect.bottom())
        .or(lanes.last())
        .map_or(0, |lane| lane.row)
}

/// Over the lanes during a drag: a ghost plate of each moved Item at its preview place with the snap line down every lane, the snap line of a trimmed edge, or the selection box on a `well` halo.
fn paint_lane_drag(
    ui: &Ui,
    lanes: &[Lane],
    items: &[ItemBlock],
    scene: &Scene,
    view: &ProjectView,
) {
    let Some(drag) = &view.lane_drag else {
        return;
    };
    let Some(area) = lanes.iter().map(|lane| lane.rect).reduce(Rect::union) else {
        return;
    };
    let painter = ui.painter().with_clip_rect(area.intersect(ui.clip_rect()));
    let colours = palette();
    match drag {
        LaneDrag::Move { drag, preview } => {
            for item in items.iter().filter(|item| drag.moves(item.id)) {
                let Some(row) = lanes.iter().position(|lane| lane.track_id == item.track_id) else {
                    continue;
                };
                let (position, row) = preview.place(item, row);
                let Some(lane) = lanes.get(row) else {
                    continue;
                };
                paint_ghost(
                    &painter,
                    scene.plate(lane.rect, position, item.length),
                    item,
                );
            }
            if let Some(line) = preview.snap_line {
                for lane in lanes {
                    paint_haloed_line(
                        &painter,
                        scene.x(lane.rect, line),
                        lane.rect,
                        1.0,
                        colours.text,
                    );
                }
            }
        }
        LaneDrag::Trim { preview, .. } => {
            if let Some(line) = preview.and_then(|preview| preview.snap_line) {
                for lane in lanes {
                    paint_haloed_line(
                        &painter,
                        scene.x(lane.rect, line),
                        lane.rect,
                        1.0,
                        colours.text,
                    );
                }
            }
        }
        LaneDrag::Box { from, to } => {
            let x = |frame: f64| area.left() + scene.timeline.x_of(frame);
            let rect = Rect::from_two_pos(pos2(x(from.0), from.1), pos2(x(to.0), to.1));
            for (width, ink) in [
                (BOX_EDGE_WIDTH + 2.0 * HALO_WIDTH, colours.well),
                (BOX_EDGE_WIDTH, colours.lit),
            ] {
                painter.rect_stroke(
                    rect,
                    0,
                    Stroke::new(width, colour(ink)),
                    egui::StrokeKind::Middle,
                );
            }
        }
    }
}

/// Where a moved Item would land: a plate at reduced emphasis, the panel with a boundary edge and its Take name in muted text, no waveform.
fn paint_ghost(painter: &Painter, span: Rect, item: &ItemBlock) {
    let colours = palette();
    painter.rect(
        span,
        RADIUS_CONTROL,
        colour(colours.panel),
        Stroke::new(TAKE_EDGE_WIDTH, colour(colours.boundary)),
        egui::StrokeKind::Inside,
    );
    painter.with_clip_rect(span).text(
        span.left_top() + vec2(theme::SPACE_XS, UNIT_EDGE_WIDTH + theme::SPACE_XS),
        Align2::LEFT_TOP,
        item.label(),
        theme::legend(),
        colour(colours.muted),
    );
}

/// A vertical line `width` wide down `lane` in `ink`, over a `well` halo of [`HALO_WIDTH`] on each side, so it holds its contrast against a Track colour.
fn paint_haloed_line(painter: &Painter, x: f32, lane: Rect, width: f32, ink: Rgb) {
    painter.vline(
        x,
        lane.y_range(),
        Stroke::new(width + 2.0 * HALO_WIDTH, colour(palette().well)),
    );
    painter.vline(x, lane.y_range(), Stroke::new(width, colour(ink)));
}

/// A plate on the lane: the Take body with a top edge in the Track colour and the Take's name on the body.
fn paint_take(painter: &Painter, span: Rect, name: Option<&str>, track: Rgb) {
    let colours = palette();
    painter.rect(
        span,
        RADIUS_CONTROL,
        colour(colours.panel_open),
        Stroke::new(TAKE_EDGE_WIDTH, colour(colours.take_edge)),
        egui::StrokeKind::Inside,
    );
    paint_plate_top(painter, span, track, name);
}

/// A zero line and min/max columns in `ink` from `left`, one per pixel, over the plate body below the Take's name, so silence reads as a flat line.
#[allow(
    clippy::cast_precision_loss,
    reason = "a column index below the window width"
)]
fn paint_waveform(painter: &Painter, span: Rect, left: f32, columns: &[(i8, i8)], ink: Rgb) {
    let body = Rect::from_x_y_ranges(
        span.x_range(),
        span.top() + WAVE_TOP..=span.bottom() - theme::SPACE_XS,
    );
    let centre = body.center().y.floor();
    let scale = body.height() / 2.0 / f32::from(i8::MAX);
    let fill = colour(ink);
    let zero = Rect::from_x_y_ranges(left..=left + columns.len() as f32, centre..=centre + 1.0);
    painter.rect_filled(zero, 0, fill);
    for (index, &(min, max)) in columns.iter().enumerate() {
        let x = left + index as f32;
        let top = centre - f32::from(max) * scale;
        let bottom = (centre + 1.0 - f32::from(min) * scale).max(top + 1.0);
        painter.rect_filled(Rect::from_x_y_ranges(x..=x + 1.0, top..=bottom), 0, fill);
    }
}

/// A Take plate's top edge in `edge` and `name` on the body below it.
fn paint_plate_top(painter: &Painter, span: Rect, edge: Rgb, name: Option<&str>) {
    let top = Rect::from_min_size(span.min, vec2(span.width(), UNIT_EDGE_WIDTH));
    painter.rect_filled(top, 0, colour(edge));
    let Some(name) = name else {
        return;
    };
    let mut job = LayoutJob::single_section(
        name.to_owned(),
        TextFormat::simple(theme::legend(), colour(palette().text)),
    );
    job.wrap = TextWrapping::truncate_at_width(span.width() - 2.0 * theme::SPACE_XS);
    painter.with_clip_rect(span).galley(
        span.left_top() + vec2(theme::SPACE_XS, UNIT_EDGE_WIDTH + theme::SPACE_XS),
        painter.layout_job(job),
        colour(palette().text),
    );
}

/// The Take being recorded: a dark red plate with a danger top edge and its name, in a dashed danger outline, up to the playhead.
fn paint_recording_take(painter: &Painter, span: Rect, name: &str) {
    let colours = palette();
    painter.rect_filled(span, RADIUS_CONTROL, colour(colours.recording_take));
    paint_plate_top(painter, span, colours.danger, Some(name));
    let corners = [
        span.left_top(),
        span.right_top(),
        span.right_bottom(),
        span.left_bottom(),
        span.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(
        &corners,
        Stroke::new(1.0, colour(colours.danger)),
        TAKE_DASH,
        TAKE_GAP,
    ));
}

/// The Track number, then the name with Arm, Mute and Solo over the input and its level bar, then volume and pan.
fn track_controls(ui: &mut Ui, row: &TrackRow, ports: &[String]) -> Vec<Command> {
    let mut edit = TrackEdit::default();
    let mut refresh_ports = false;
    let colours = palette();
    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(NUMBER_WIDTH, ui.available_height()),
            egui::Label::new(muted_text(format!("{}", row.index + 1)).font(theme::unit_name())),
        );
        ui.vertical(|ui| {
            ui.set_width(HEADER_WIDTH);
            ui.spacing_mut().item_spacing.y = theme::SPACE_XS;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = SWITCH_GAP;
                let name_id = Id::unique(("track_name", row.index));
                let name_width = HEADER_WIDTH - 3.0 * (SWITCH_SIZE.x + SWITCH_GAP);
                let name = format!("Track {}", row.index + 1);
                edit.name = edited_text(ui, name_id, &row.name, name_width, &name);
                let switches = [
                    ("R", "Arm", row.armed, colours.danger, &mut edit.arm),
                    ("M", "Mute", row.mute, colours.lit, &mut edit.mute),
                    ("S", "Solo", row.solo, colours.lit, &mut edit.solo),
                ];
                for (text, name, on, lit, change) in switches {
                    let response = ui.add(lit_button(text, on, lit).min_size(SWITCH_SIZE));
                    response.widget_info(|| {
                        WidgetInfo::labeled(Role::Button, true, format!("{name} {}", row.name))
                    });
                    if response.clicked() {
                        *change = Some(!on);
                    }
                }
            });
            refresh_ports = input_picker(ui, row, ports, &mut edit.input);
            input_meter(ui, row.input_peak_db);
        });
        let accent = colour(track_colour(row.index));
        let volume = track_knob(
            ui,
            ("Volume", &row.name),
            row.volume_db,
            VOLUME_DB,
            " dB",
            accent,
        );
        edit.volume = volume.map(|db| db as f32);
        let pan = track_knob(ui, ("Pan", &row.name), row.pan, PAN, "", accent);
        edit.pan = pan.map(|pan| pan as f32);
    });
    let mut commands = Vec::new();
    if refresh_ports {
        commands.push(Command::RefreshPorts);
    }
    if edit != TrackEdit::default() {
        commands.push(Command::TrackSet {
            track: row.index,
            edit,
        });
    }
    commands
}

/// `true` when the menu was opened, so that the port list is read again.
fn input_picker(
    ui: &mut Ui,
    row: &TrackRow,
    ports: &[String],
    change: &mut Option<String>,
) -> bool {
    let combo = ComboBox::from_id_salt(("track_input", row.index))
        .width(HEADER_WIDTH)
        .truncate()
        .selected_text(row.input.as_deref().unwrap_or(NO_INPUT))
        .show_ui(ui, |ui| {
            if ui.selectable_label(row.input.is_none(), NO_INPUT).clicked() {
                *change = Some(String::new());
            }
            for port in ports {
                let selected = row.input.as_ref() == Some(port);
                if ui.selectable_label(selected, port).clicked() && !selected {
                    *change = Some(port.clone());
                }
            }
        });
    combo
        .response
        .widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, format!("Input {}", row.name)));
    combo.response.clicked()
}

fn input_meter(ui: &mut Ui, db: f64) {
    let colours = palette();
    let size = vec2(HEADER_WIDTH, LEVEL_BAR_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Meter, true, format!("{db:.1} dB")));
    let painter = ui.painter();
    painter.rect_filled(rect, 1, colour(colours.well));
    let fraction = ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0) as f32;
    let lit = if db >= LADDER_RED_DB {
        colours.danger
    } else if db >= LADDER_YELLOW_DB {
        colours.lit
    } else {
        colours.go
    };
    let level = Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
    painter.rect_filled(level, 1, colour(lit));
}

/// A small knob over a number field, its value arc in `accent`; either returns its value when a drag ends or a value is typed, a double-click on the knob the middle (0 dB, centre).
pub(crate) fn track_knob(
    ui: &mut Ui,
    label: (&str, &str),
    value: f64,
    range: (f64, f64),
    suffix: &str,
    accent: Color32,
) -> Option<f64> {
    knob_with_reset(ui, label, value, range, suffix, 0.0, accent)
}

/// [`track_knob`] whose double-click returns `reset`, its value arc in `accent`.
pub(crate) fn knob_with_reset(
    ui: &mut Ui,
    (what, track): (&str, &str),
    value: f64,
    (min, max): (f64, f64),
    suffix: &str,
    reset: f64,
    accent: Color32,
) -> Option<f64> {
    ui.vertical_centered(|ui| {
        ui.set_width(KNOB_COLUMN_WIDTH);
        let fraction = ((value - min) / (max - min)) as f32;
        let origin = knob_origin(min, max, reset);
        let knob = widgets::rack_knob(ui, KnobSize::Strip, (fraction, origin), accent);
        knob.response
            .widget_info(|| WidgetInfo::labeled(Role::Slider, true, format!("{what} {track}")));
        let field_id = knob.response.id.with("field");
        let (typed, field) = released_value(ui, field_id, value, |typed| {
            DragValue::new(typed)
                .range(min..=max)
                .speed((max - min) / 200.0)
                .fixed_decimals(if suffix.is_empty() { 2 } else { 1 })
                .suffix(suffix.to_string())
        });
        field.widget_info(|| {
            WidgetInfo::labeled(Role::SpinButton, true, format!("{what} {track} value"))
        });
        if knob.reset {
            return Some(reset);
        }
        let dragged_id = knob.response.id.with("dragged");
        if knob.response.dragged() {
            ui.data_mut(|data| data.insert_temp(dragged_id, knob.fraction));
        }
        if knob.response.drag_stopped()
            && let Some(fraction) = ui.data_mut(|data| data.remove_temp::<f32>(dragged_id))
        {
            return Some(min + f64::from(fraction) * (max - min));
        }
        typed
    })
    .inner
}

/// The prompt after `project_open` found a newer autosave; `None` while open, else whether to restore.
pub fn autosave_prompt(ctx: &egui::Context, name: &str) -> Option<bool> {
    let mut choice = None;
    widgets::modal(Id::unique("autosave_prompt")).show(ctx, |ui| {
        ui.spacing_mut().interact_size.y = theme::CONTROL_HEIGHT;
        ui.label(RichText::new(format!("{AUTOSAVE_PROMPT} — {name}")).font(theme::dialog_title()));
        ui.label(muted_text(
            "It holds the changes since the Project was last saved.",
        ));
        ui.add_space(theme::SPACE_XS);
        ui.horizontal(|ui| {
            if ui.add(primary_button("Restore")).clicked() {
                choice = Some(true);
            }
            if ui.add(outlined_button("Open saved")).clicked() {
                choice = Some(false);
            }
        });
    });
    choice
}
