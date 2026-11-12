use egui::{
    Align2, Color32, ComboBox, CursorIcon, DragValue, EventFilter, Frame, Id, Key, Margin, Painter,
    Rect, Response, RichText, Role, ScrollArea, Sense, Stroke, TextEdit, TextStyle, TextWrapMode,
    Ui, UiBuilder, WidgetInfo, WidgetText, pos2, vec2,
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
    EDGE_GRAB_PX, GridLine, ItemBlock, ItemSelection, LoopEdge, MainView, MoveDrag, MovePointer,
    MovePreview, Tempo, TimelineView, TrackRow, TransportView, bars_beats_text, dragged_loop,
    grid_lines, grid_step, hit_test, item_blocks, item_name, items_in_box, move_preview,
    nudge_command, parse_meter, project_key_to_command, project_tempo, recording_span, sample_rate,
    snap_to_beat, time_text, track_colour, track_rows, transport_view, waveform_columns,
};
use crate::rack::muted_text;
use crate::theme::{self, RADIUS_CONTROL, Rgb, UNIT_EDGE_WIDTH, colour, palette, translucent};
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
/// Where a Take's waveform starts below its plate's top: under the top edge and the Take's name.
const WAVE_TOP: f32 = UNIT_EDGE_WIDTH + theme::SPACE_XS + theme::SPACE_L;
/// The loop region's lit tint over the lanes (project mockup: loop overlay, 13 %).
const LOOP_ALPHA_ON: u8 = 33;
const LOOP_ALPHA_OFF: u8 = 16;
const SELECTED_OUTLINE: f32 = 2.0;
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

/// A drag on the lanes: the selected Items being moved, or a selection box from one corner (a timeline frame and a screen y) to the other.
#[derive(Debug, Clone)]
enum LaneDrag {
    Move {
        drag: MoveDrag,
        preview: MovePreview,
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
    let strip = TRACK_LIST_WIDTH - 8.0;
    let add = Rect::from_min_size(
        full.min,
        vec2(strip - SNAP_BUTTON_WIDTH - theme::SPACE_XS, RULER_HEIGHT),
    );
    if ui.put(add, outlined_button("+ Add Track")).clicked() {
        commands.push(Command::TrackAdd);
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
            for row in &rows {
                commands.extend(track(
                    ui,
                    row,
                    &items,
                    &snapshot.peaks,
                    &ports,
                    &scene,
                    &mut lanes,
                ));
            }
            commands.extend(item_nodes(ui, &items, &lanes, &scene, view, (rate, tempo)));
            commands.extend(lane_pointer(ui, &lanes, &items, &scene, view));
            paint_lane_drag(ui, &lanes, &items, &scene, view);
        });
    });
    widgets::rack_ears(ui.painter(), face, None);
    ui.allocate_rect(face, Sense::hover());
    commands
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
    ui.scope_builder(UiBuilder::new().max_rect(controls), |ui| {
        commands.extend(track_controls(ui, row, ports));
    });
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
    for item in items.iter().filter(|item| item.track_id == row.id) {
        let span = scene.plate(lane, item.position, item.length);
        let track = track_colour(row.index);
        paint_take(painter, span, item, track);
        if let Some(visible) = item.visible_take(scene.timeline, lane.width())
            && let Some(peaks) = peaks.get(&item.file)
        {
            let columns = waveform_columns(peaks, visible.frames, visible.width);
            let left = lane.left() + visible.x;
            let inner = span.shrink(TAKE_EDGE_WIDTH);
            paint_waveform(&painter.with_clip_rect(inner), inner, left, &columns, track);
        }
        if scene.selected.contains(item.id) {
            painter.rect_stroke(
                span,
                RADIUS_CONTROL,
                Stroke::new(SELECTED_OUTLINE, colour(colours.lit)),
                egui::StrokeKind::Inside,
            );
        }
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
}

/// One focusable node per Item, after every Track header and in time order, named by [`item_name`] with its selected state; focus selects the Item, and ← and → on it move the selected Items by one grid step.
fn item_nodes(
    ui: &Ui,
    items: &[ItemBlock],
    lanes: &[Lane],
    scene: &Scene,
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
    let mut nudge = None;
    for (item, lane) in placed {
        let plate = scene.plate(lane.rect, item.position, item.length);
        if !plate.is_positive() {
            continue;
        }
        let id = Id::unique(("item", item.id));
        let response = ui.interact(plate, id, Sense::FOCUSABLE);
        let name = item_name(item, &lane.name, rate, tempo);
        response.widget_info(|| WidgetInfo::labeled(Role::ListItem, true, &name));
        let selected = view.selection.contains(item.id);
        ui.ctx()
            .accesskit_node_builder(id, |node| node.set_selected(selected));
        if response.gained_focus() {
            view.selection.pick(item.id, false);
        }
        if !response.has_focus() {
            continue;
        }
        let arrows_stay = EventFilter {
            horizontal_arrows: true,
            ..EventFilter::default()
        };
        ui.memory_mut(|memory| memory.set_focus_lock_filter(id, arrows_stay));
        let step = grid_step(scene.timeline, rate, tempo);
        let (left, right) = ui.input(|input| {
            (
                input.key_pressed(Key::ArrowLeft),
                input.key_pressed(Key::ArrowRight),
            )
        });
        if left || right {
            let delta = if left { -step } else { step };
            nudge = nudge_command(items, &view.selection, delta);
        }
    }
    nudge
}

/// A click on an Item selects it (Ctrl: in or out), on the empty lane clears the selection (Ctrl: keeps it); both select the lane's Track. A drag from an Item moves the selected Items, sent as `item_move` on release; a drag from the empty lane draws a selection box (Ctrl: adds to the selection).
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
        if response.drag_started()
            && let Some(origin) = origin
        {
            let x = origin.x - lane.rect.left();
            let frame = scene.timeline.frame_at(x);
            view.lane_drag = Some(
                match hit_test(items, lane.track_id, scene.timeline, x).item() {
                    Some(id) => {
                        view.selection.grab(id, toggle);
                        LaneDrag::Move {
                            drag: MoveDrag::new(id, &view.selection, frame, lane.row),
                            preview: MovePreview::default(),
                        }
                    }
                    None => LaneDrag::Box {
                        from: (frame, origin.y),
                        to: (frame, origin.y),
                    },
                },
            );
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
            LaneDrag::Box { to, .. } => *to = (frame, at.y),
        }
    }
    if !lane.response.drag_stopped() {
        return commands;
    }
    match view.lane_drag.take() {
        Some(LaneDrag::Move { drag, preview }) => commands.extend(preview.command(&drag)),
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
        }
        None => {}
    }
    commands
}

/// The row of the lane under the screen `y`: the first above it, the last below it.
fn row_at(lanes: &[Lane], y: f32) -> usize {
    lanes
        .iter()
        .find(|lane| y < lane.rect.bottom())
        .or(lanes.last())
        .map_or(0, |lane| lane.row)
}

/// Over the lanes during a drag: a ghost plate of each moved Item at its preview place with the snap line down every lane, or the selection box on a `well` halo.
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
fn paint_take(painter: &Painter, span: Rect, item: &ItemBlock, track: Rgb) {
    let colours = palette();
    painter.rect(
        span,
        RADIUS_CONTROL,
        colour(colours.panel_open),
        Stroke::new(TAKE_EDGE_WIDTH, colour(colours.take_edge)),
        egui::StrokeKind::Inside,
    );
    paint_plate_top(painter, span, track, &item.label());
}

/// A zero line and min/max columns in `track` from `left`, one per pixel, over the plate body below the Take's name, so silence reads as a flat line.
#[allow(
    clippy::cast_precision_loss,
    reason = "a column index below the window width"
)]
fn paint_waveform(painter: &Painter, span: Rect, left: f32, columns: &[(i8, i8)], track: Rgb) {
    let body = Rect::from_x_y_ranges(
        span.x_range(),
        span.top() + WAVE_TOP..=span.bottom() - theme::SPACE_XS,
    );
    let centre = body.center().y.floor();
    let scale = body.height() / 2.0 / f32::from(i8::MAX);
    let fill = colour(track);
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
fn paint_plate_top(painter: &Painter, span: Rect, edge: Rgb, name: &str) {
    let top = Rect::from_min_size(span.min, vec2(span.width(), UNIT_EDGE_WIDTH));
    painter.rect_filled(top, 0, colour(edge));
    painter.with_clip_rect(span).text(
        span.left_top() + vec2(theme::SPACE_XS, UNIT_EDGE_WIDTH + theme::SPACE_XS),
        Align2::LEFT_TOP,
        name,
        theme::legend(),
        colour(palette().text),
    );
}

/// The Take being recorded: a dark red plate with a danger top edge and its name, in a dashed danger outline, up to the playhead.
fn paint_recording_take(painter: &Painter, span: Rect, name: &str) {
    let colours = palette();
    painter.rect_filled(span, RADIUS_CONTROL, colour(colours.recording_take));
    paint_plate_top(painter, span, colours.danger, name);
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
