use egui::{
    Align2, Button, Color32, Context, CornerRadius, EventFilter, Frame, Id, Mesh, Modal, Painter,
    Pos2, Rect, Response, RichText, Role, Sense, Stroke, StrokeKind, TextEdit, Ui, Vec2, Visuals,
    Widget, WidgetInfo, pos2, vec2,
};

use crate::mixer_model::progress_text_place;
use crate::model::{
    Band, EAR_HOLE_RADIUS, EAR_WIDTH, FOOTSWITCH_WIDTH, KNOB_ARC_SWEEP_DEG, LADDER_SEGMENTS,
    ScreenRect, ear_holes, focus_ring_rects, knob_arcs, knob_points, knob_value_from_drag,
    ladder_segments,
};
use crate::theme::{
    self, CARET_WIDTH, CONTROL_HEIGHT, DIALOG_MARGIN, EDIT_BORDER_WIDTH, FOCUS_RING_GAP,
    FOCUS_RING_WIDTH, PROGRESS_HEIGHT, RADIUS_CONTROL, RADIUS_LED, RADIUS_PANEL, RADIUS_PILL,
    SPACE_S, accent_for, colour, palette,
};

const ARC_SEGMENTS: usize = 48;
const CENTS_ARC_DIAMETER: f32 = 160.0;
const CENTS_ARC_WIDTH: f32 = 4.0;
const CENTS_NEEDLE_WIDTH: f32 = 3.0;
const CENTS_ARC_START_DEG: f32 = 180.0;
const CENTS_ARC_SWEEP_DEG: f32 = 180.0;
const STRIP_KNOB_DIAMETER: f32 = 34.0;
const SMALL_KNOB_DIAMETER: f32 = 40.0;
const LARGE_KNOB_DIAMETER: f32 = 56.0;
/// Knob geometry in the mockup's 60-unit drawing, scaled to the knob's diameter.
const KNOB_DRAWING: f32 = 60.0;
const KNOB_ARC_RADIUS: f32 = 26.0;
const KNOB_ARC_WIDTH: f32 = 3.4;
const KNOB_TICKS: usize = 11;
const KNOB_TICK_INNER: f32 = 19.0;
const KNOB_TICK_OUTER: f32 = 21.5;
const KNOB_TICK_WIDTH: f32 = 1.3;
const KNOB_SKIRT_RADIUS: f32 = 17.0;
const KNOB_BODY_RADIUS: f32 = 14.5;
const KNOB_RIM_WIDTH: f32 = 1.3;
const KNOB_POINTER_INNER: f32 = 4.0;
const KNOB_POINTER_OUTER: f32 = 14.0;
const KNOB_POINTER_WIDTH: f32 = 2.6;
/// Where the body's highlight sits, as a fraction of its radius left and up of the centre.
const KNOB_HIGHLIGHT_OFFSET: Vec2 = Vec2::new(-0.3, -0.4);
const LAMP_RADIUS: f32 = 6.0;
const LAMP_GAP: f32 = 2.0;
const LAMP_RING_WIDTH: f32 = 1.0;
const LAMP_OFF_RING_WIDTH: f32 = 1.5;
const TAG_PADDING: Vec2 = Vec2::new(SPACE_S, 0.0);
const LADDER_SEGMENT_HEIGHT: f32 = 11.0;
const LADDER_SEGMENT_WIDTHS: (f32, f32) = (8.0, 20.0);
const LADDER_GAP: f32 = 3.0;
const SCREW_RING_WIDTH: f32 = 1.5;
/// How far the light copy of an engraved Rack position sits below the dark one.
const ENGRAVE_OFFSET: f32 = 1.0;
const CENTS_TICKS: usize = 21;
const CENTS_TICK_HEIGHT: f32 = 8.0;
const CENTS_CENTRE_TICK_HEIGHT: f32 = 20.0;
const CENTS_STRIP_INSET: f32 = 2.0;
const BANK_ROW_BORDER: f32 = 3.0;
const SWITCH_SIZE: Vec2 = Vec2::new(32.0, 18.0);
const SWITCH_TARGET: Vec2 = Vec2::new(SWITCH_SIZE.x, CONTROL_HEIGHT);
const SWITCH_THUMB_RADIUS: f32 = 6.0;
const SWITCH_THUMB_INSET: f32 = 3.0;
const BANK_NUMBER_WIDTH: f32 = 18.0;
pub const FOOTSWITCH_SIZE: Vec2 = Vec2::new(FOOTSWITCH_WIDTH, 38.0);
const FOOTSWITCH_LED_TOP: f32 = 13.0;
const FOOTSWITCH_TEXT_BOTTOM: f32 = 4.0;
/// Where the focus pass reads a widget's own corner radius, for a widget that is not a rounded rectangle.
const FOCUS_CORNER: &str = "focus corner";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnobSize {
    /// Mixer strips and Track headers.
    Strip,
    Small,
    Large,
}

impl KnobSize {
    pub fn diameter(self) -> f32 {
        match self {
            KnobSize::Strip => STRIP_KNOB_DIAMETER,
            KnobSize::Small => SMALL_KNOB_DIAMETER,
            KnobSize::Large => LARGE_KNOB_DIAMETER,
        }
    }
}

/// `fraction` is the dragged position in 0..=1 before any integer stepping, which the caller applies; `reset` is a double-click asking for the Parameter default. While focused, ↑ and ↓ stay with the knob instead of moving focus; the caller reads them.
pub struct RackKnob {
    pub response: Response,
    pub fraction: f32,
    pub reset: bool,
}

pub fn accent(type_name: &str) -> Color32 {
    colour(accent_for(type_name))
}

pub fn visuals() -> Visuals {
    let colours = palette();
    let mut visuals = Visuals::dark();
    visuals.panel_fill = colour(colours.case);
    visuals.window_fill = colour(colours.panel);
    visuals.extreme_bg_color = colour(colours.well);
    visuals.faint_bg_color = colour(colours.panel);
    visuals.override_text_color = Some(colour(colours.text));
    visuals.selection.bg_fill = colour(colours.hover);
    visuals.selection.stroke = Stroke::new(1.0, colour(colours.text));
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = colour(colours.panel);
    widgets.noninteractive.bg_stroke.color = colour(colours.hover);
    widgets.noninteractive.fg_stroke.color = colour(colours.muted);
    visuals.weak_text_color = Some(colour(colours.muted));
    visuals.text_cursor.stroke = Stroke::new(CARET_WIDTH, colour(colours.text));
    for (state, edge) in [
        (&mut widgets.inactive, colours.boundary),
        (&mut widgets.hovered, colours.hover_boundary),
        (&mut widgets.active, colours.hover_boundary),
    ] {
        state.bg_stroke = Stroke::new(1.0, colour(edge));
        state.fg_stroke.color = colour(colours.text);
    }
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = RADIUS_CONTROL.into();
    }
    widgets.inactive.bg_fill = colour(colours.panel_open);
    widgets.inactive.weak_bg_fill = colour(colours.panel_open);
    widgets.hovered.bg_fill = colour(colours.hover);
    widgets.hovered.weak_bg_fill = colour(colours.hover);
    widgets.active.bg_fill = colour(colours.panel_open);
    widgets.active.weak_bg_fill = colour(colours.panel_open);
    widgets.active.bg_stroke.color = colour(colours.boundary);
    visuals
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ButtonKind {
    Primary,
    Outlined,
    Warning,
    Lit(theme::Rgb),
    Edged(theme::Rgb),
}

/// A button with a rest, a hover and a pressed look: [`primary_button`], [`outlined_button`], [`warning_button`], [`switched_button`] and [`edged_button`]. Keyboard focus keeps the rest or hover look inside the focus ring; only a press shows the pressed one.
pub struct StyledButton {
    button: Button<'static>,
    kind: ButtonKind,
}

impl StyledButton {
    #[must_use]
    pub fn min_size(self, size: Vec2) -> Self {
        Self {
            button: self.button.min_size(size),
            ..self
        }
    }

    /// Cuts the text short with an ellipsis where it does not fit.
    #[must_use]
    pub fn truncate(self) -> Self {
        Self {
            button: self.button.truncate(),
            ..self
        }
    }

    /// Marks the button as a toggle in the accessibility tree; its look stays the kind's own.
    #[must_use]
    pub fn selected(self, selected: bool) -> Self {
        Self {
            button: self.button.selected(selected),
            ..self
        }
    }
}

impl Widget for StyledButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let colours = palette();
        let (fills, edges) = match self.kind {
            ButtonKind::Primary => {
                let fills = [colours.text, colours.hover_boundary, colours.muted];
                (fills, fills)
            }
            ButtonKind::Outlined => (
                [colours.panel_open, colours.hover, colours.well],
                [colours.boundary, colours.hover_boundary, colours.text],
            ),
            ButtonKind::Warning => (
                [colours.panel_open, colours.hover, colours.well],
                [colours.danger; 3],
            ),
            ButtonKind::Lit(lit) => ([lit; 3], [lit, colours.hover_boundary, colours.text]),
            ButtonKind::Edged(edge) => (
                [colours.panel_open, colours.hover, colours.well],
                [edge, colours.hover_boundary, colours.text],
            ),
        };
        ui.scope(|ui| {
            // egui draws a focused widget with its `active` look, so `active` gets the look the button had last frame: pressed while the pointer is down on it, else hover or rest.
            let last = ui.ctx().read_response(ui.next_auto_id());
            let look = match last {
                Some(last) if last.is_pointer_button_down_on() => 2,
                Some(last) if last.hovered() => 1,
                _ => 0,
            };
            let widgets = &mut ui.visuals_mut().widgets;
            for (state, look) in [
                (&mut widgets.inactive, 0),
                (&mut widgets.hovered, 1),
                (&mut widgets.active, look),
            ] {
                state.weak_bg_fill = colour(fills[look]);
                state.bg_stroke = Stroke::new(1.0, colour(edges[look]));
            }
            let selection = &mut ui.visuals_mut().selection;
            selection.bg_fill = colour(fills[look]);
            selection.stroke.color = colour(colours.text);
            ui.add(self.button)
        })
        .inner
    }
}

/// Light fill and dark text: the one action a group of buttons is for.
pub fn primary_button(text: &str) -> StyledButton {
    StyledButton {
        button: Button::new(RichText::new(text).color(colour(palette().on_accent))),
        kind: ButtonKind::Primary,
    }
}

/// The window's button look: a boundary edge, a lighter fill on hover, a dark well while pressed.
pub fn outlined_button(text: impl Into<RichText>) -> StyledButton {
    StyledButton {
        button: Button::new(text.into()),
        kind: ButtonKind::Outlined,
    }
}

/// Edged in the clip red, for an action that removes something.
pub fn warning_button(text: &str) -> StyledButton {
    StyledButton {
        button: Button::new(RichText::new(text).color(colour(palette().danger_text))),
        kind: ButtonKind::Warning,
    }
}

/// A glyph with no frame at rest; on hover and while pressed it takes the [`outlined_button`] fill and edge.
pub fn frameless_button(text: impl Into<RichText>) -> StyledButton {
    StyledButton {
        button: Button::new(text.into()).frame_when_inactive(false),
        kind: ButtonKind::Outlined,
    }
}

/// Lays out radio buttons in a row whose selected dot is lit, as every other selection is.
pub fn lit_radios(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let lit = colour(palette().lit);
        let widgets = &mut ui.visuals_mut().widgets;
        for state in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
        ] {
            state.fg_stroke.color = lit;
        }
        add(ui);
    });
}

/// A toggle reading `text`: while `lit` filled in `lit_colour` with dark text, else an [`outlined_button`].
pub fn switched_button(text: &str, lit: bool, lit_colour: theme::Rgb) -> StyledButton {
    let button = if lit {
        StyledButton {
            button: Button::new(RichText::new(text).color(colour(palette().on_accent))),
            kind: ButtonKind::Lit(lit_colour),
        }
    } else {
        outlined_button(text)
    };
    button.selected(lit)
}

/// An [`outlined_button`] whose resting edge is `edge`: a state between unlit and lit.
pub fn edged_button(text: &str, edge: theme::Rgb) -> StyledButton {
    StyledButton {
        button: Button::new(text.to_string()),
        kind: ButtonKind::Edged(edge),
    }
}

/// A pill switch named `name`: while on, lit with its thumb at the right; while off, a dark well with a muted thumb at the left.
pub fn toggle_switch(ui: &mut Ui, name: &str, on: bool) -> Response {
    let colours = palette();
    let (rect, response) = ui.allocate_exact_size(SWITCH_TARGET, Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Switch, true, on, name));
    focus_corner(ui, response.id, SWITCH_TARGET.y / 2.0);
    let pill = Rect::from_center_size(rect.center(), SWITCH_SIZE);
    let (_, edge) = control_state(&response);
    let at_rest = !response.hovered() && !response.is_pointer_button_down_on();
    let (fill, edge, thumb) = if on {
        let edge = if at_rest { colours.lit } else { edge };
        (colours.lit, edge, colours.on_accent)
    } else {
        (colours.well, edge, colours.muted)
    };
    let painter = ui.painter();
    painter.rect(
        pill,
        RADIUS_PILL,
        colour(fill),
        Stroke::new(1.0, colour(edge)),
        StrokeKind::Inside,
    );
    let inset = SWITCH_THUMB_INSET + SWITCH_THUMB_RADIUS;
    let thumb_x = if on {
        pill.right() - inset
    } else {
        pill.left() + inset
    };
    painter.circle_filled(
        pos2(thumb_x, pill.center().y),
        SWITCH_THUMB_RADIUS,
        colour(thumb),
    );
    response
}

/// A single-line field: muted placeholder, while editing a lit edge, a caret and lit selected text in dark letters.
pub fn text_field(edit: TextEdit<'_>) -> impl Widget + '_ {
    move |ui: &mut Ui| {
        ui.scope(|ui| {
            let colours = palette();
            let selection = &mut ui.visuals_mut().selection;
            selection.bg_fill = colour(colours.lit);
            selection.stroke = Stroke::new(EDIT_BORDER_WIDTH, colour(colours.on_accent));
            let response = ui.add(edit);
            if response.has_focus() {
                ui.painter().rect_stroke(
                    response.rect,
                    RADIUS_CONTROL,
                    Stroke::new(EDIT_BORDER_WIDTH, colour(colours.lit)),
                    StrokeKind::Inside,
                );
            }
            response
        })
        .inner
    }
}

/// A dialog over the window: the open panel edged in the boundary colour, over the scrim.
pub fn modal(id: Id) -> Modal {
    let colours = palette();
    Modal::new(id)
        .frame(
            Frame::new()
                .fill(colour(colours.panel_open))
                .stroke(Stroke::new(1.0, colour(colours.boundary)))
                .corner_radius(RADIUS_PANEL)
                .inner_margin(DIALOG_MARGIN),
        )
        .backdrop_color(theme::scrim())
}

/// A rounded well edged in the boundary colour, lit from the left as far as `fraction`, with the percentage in dark text on the lit part, or in the text colour on the well after it while the lit part is too short for it.
pub fn progress_bar(ui: &mut Ui, fraction: f32, width: f32) -> Response {
    let colours = palette();
    let (rect, response) = ui.allocate_exact_size(vec2(width, PROGRESS_HEIGHT), Sense::hover());
    let percent = format!("{:.0}%", fraction * 100.0);
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::ProgressIndicator, true, "Render progress");
        info.current_text_value = Some(percent.clone());
        info
    });
    let radius = PROGRESS_HEIGHT / 2.0;
    let painter = ui.painter();
    painter.rect_filled(rect, radius, colour(colours.well));
    let lit = Rect::from_min_size(
        rect.min,
        vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()),
    );
    painter.rect_filled(lit, radius, colour(colours.lit));
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, colour(colours.boundary)),
        StrokeKind::Inside,
    );
    let galley = painter.layout_no_wrap(percent, theme::value(), Color32::PLACEHOLDER);
    let (left, on_lit) = progress_text_place(lit.width(), galley.size().x);
    let text_colour = if on_lit {
        colours.on_accent
    } else {
        colours.text
    };
    let pos = pos2(rect.left() + left, rect.center().y - galley.size().y / 2.0);
    painter.galley(pos, galley, colour(text_colour));
    response
}

/// The `Bypassed` pill beside a bypassed unit's name.
pub fn tag(ui: &mut Ui, text: &str) -> Response {
    let colours = palette();
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        theme::legend(),
        colour(colours.bypass_text),
    );
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + TAG_PADDING * 2.0, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        RADIUS_PILL,
        Stroke::new(1.0, colour(colours.boundary)),
        StrokeKind::Inside,
    );
    painter.galley(rect.min + TAG_PADDING, galley, colour(colours.bypass_text));
    response
}

/// The ring around whichever widget has keyboard focus, drawn once a frame on that widget's layer: a dark gap band, then the focus-ring colour. A text field shows its focus with its own lit edge instead; a widget partly scrolled out of view is ringed only inside the view. A widget that is no rounded rectangle names its corner radius with [`focus_corner`].
pub fn paint_focus_ring(ctx: &Context) {
    if ctx.text_edit_focused() {
        return;
    }
    let Some(id) = ctx.memory(egui::Memory::focused) else {
        return;
    };
    let Some(response) = ctx.read_response(id) else {
        return;
    };
    let visible = response.interact_rect;
    if !visible.is_positive() {
        return;
    }
    let clip = if visible == response.rect {
        visible.expand(FOCUS_RING_GAP + FOCUS_RING_WIDTH)
    } else {
        visible
    };
    let corner = ctx
        .data(|data| data.get_temp::<f32>(id.with(FOCUS_CORNER)))
        .unwrap_or(f32::from(RADIUS_CONTROL));
    let painter = ctx.layer_painter(response.layer_id).with_clip_rect(clip);
    focus_ring(&painter, response.rect, corner);
}

pub fn focus_corner(ui: &Ui, id: Id, corner: f32) {
    ui.data_mut(|data| data.insert_temp(id.with(FOCUS_CORNER), corner));
}

fn focus_ring(painter: &Painter, widget: Rect, corner: f32) {
    let colours = palette();
    let (gap, ring) = focus_ring_rects(ScreenRect {
        left: widget.left(),
        top: widget.top(),
        right: widget.right(),
        bottom: widget.bottom(),
    });
    for (band, width, offset, shade) in [
        (gap, FOCUS_RING_GAP, FOCUS_RING_GAP / 2.0, colours.on_accent),
        (
            ring,
            FOCUS_RING_WIDTH,
            FOCUS_RING_GAP + FOCUS_RING_WIDTH / 2.0,
            colours.focus_ring,
        ),
    ] {
        let rect = Rect::from_min_max(pos2(band.left, band.top), pos2(band.right, band.bottom));
        painter.rect_stroke(
            rect,
            CornerRadius::same((corner + offset).round() as u8),
            Stroke::new(width, colour(shade)),
            StrokeKind::Middle,
        );
    }
}

/// A Bank row: the program number in a muted column, then the name; the active row has an amber left bar on a lighter fill, its number in the text colour and its name in semibold. `enabled` false greys the row and takes no click.
pub fn bank_row(ui: &mut Ui, number: &str, name: &str, active: bool, enabled: bool) -> Response {
    let colours = palette();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let size = vec2(ui.available_width(), CONTROL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let label = format!("{number} {name}");
    response.widget_info(|| WidgetInfo::selected(Role::Button, enabled, active, &label));
    let painter = ui.painter();
    if active {
        painter.rect_filled(rect, 0, colour(colours.panel_open));
        let border = Rect::from_min_size(rect.min, vec2(BANK_ROW_BORDER, rect.height()));
        painter.rect_filled(border, 0, colour(colours.lit));
    } else if enabled && response.hovered() {
        painter.rect_filled(rect, 0, colour(colours.hover));
    }
    let number_left = rect.left() + BANK_ROW_BORDER + SPACE_S;
    let number_colour = if active { colours.text } else { colours.muted };
    painter.text(
        pos2(number_left, rect.center().y),
        Align2::LEFT_CENTER,
        number,
        theme::value(),
        colour(number_colour),
    );
    let name_colour = if enabled { colours.text } else { colours.muted };
    let name_font = if active {
        theme::unit_name()
    } else {
        theme::list_item()
    };
    painter.text(
        pos2(number_left + BANK_NUMBER_WIDTH + SPACE_S, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        name_font,
        colour(name_colour),
    );
    response
}

/// A unit's On/Off switch with the unit's lamp on its face: lit in `accent` and reading `On` while on, unlit and reading `Off` while off. Assistive technology reads it as a switch named `name` with its on/off state.
pub fn footswitch(ui: &mut Ui, name: &str, on: bool, accent: Color32) -> Response {
    let colours = palette();
    let (rect, response) = ui.allocate_exact_size(FOOTSWITCH_SIZE, Sense::click());
    let text = if on { "On" } else { "Off" };
    response.widget_info(|| WidgetInfo::selected(Role::Switch, true, on, name));
    focus_corner(ui, response.id, f32::from(RADIUS_PANEL));
    let (_, edge) = control_state(&response);
    let painter = ui.painter();
    painter.rect(
        rect,
        RADIUS_PANEL,
        colour(colours.well),
        Stroke::new(1.0, colour(edge)),
        StrokeKind::Inside,
    );
    lamp(
        painter,
        pos2(rect.center().x, rect.top() + FOOTSWITCH_LED_TOP),
        accent,
        on,
    );
    painter.text(
        pos2(rect.center().x, rect.bottom() - FOOTSWITCH_TEXT_BOTTOM),
        Align2::CENTER_BOTTOM,
        text,
        theme::legend(),
        colour(colours.text),
    );
    response
}

/// Face and edge of a drawn control at rest, under the pointer and pressed. The footswitch keeps the rest face, so that its lamp ring stays on the dark well.
fn control_state(response: &Response) -> (theme::Rgb, theme::Rgb) {
    let colours = palette();
    if response.is_pointer_button_down_on() {
        (colours.well, colours.text)
    } else if response.hovered() {
        (colours.hover, colours.hover_boundary)
    } else {
        (colours.well, colours.boundary)
    }
}

/// `origin` is where the value arc starts, as a fraction of the sweep ([`crate::model::knob_origin`]).
pub fn rack_knob(
    ui: &mut Ui,
    size: KnobSize,
    (fraction, origin): (f32, f32),
    accent: Color32,
) -> RackKnob {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(size.diameter()), Sense::click_and_drag());
    let unrounded_id = response.id;
    if response.drag_started() {
        ui.data_mut(|data| data.insert_temp(unrounded_id, fraction));
    }
    let mut next = fraction;
    if response.dragged() {
        let fine = ui.input(|input| input.modifiers.shift);
        let unrounded = ui
            .data(|data| data.get_temp(unrounded_id))
            .unwrap_or(fraction);
        next = knob_value_from_drag(unrounded, 0.0, 1.0, response.drag_delta().y, fine, false);
        ui.data_mut(|data| data.insert_temp(unrounded_id, next));
    }
    let reset = response.double_clicked();
    let focused = response.has_focus();
    if focused {
        let arrows_stay = EventFilter {
            vertical_arrows: true,
            ..EventFilter::default()
        };
        ui.memory_mut(|memory| memory.set_focus_lock_filter(response.id, arrows_stay));
    }
    focus_corner(ui, response.id, rect.width() / 2.0);
    let pointer = ui.visuals().text_color();
    let colours = (accent, pointer);
    paint_rack_knob(
        ui.painter(),
        rect,
        (next, origin),
        colours,
        response.hovered(),
    );
    RackKnob {
        response,
        fraction: next,
        reset,
    }
}

/// Arc track and value arc outside a printed tick ring, around a dark skirt and a lit body with a light pointer.
#[allow(
    clippy::similar_names,
    reason = "painter is egui's name, pointer the knob's"
)]
fn paint_rack_knob(
    painter: &Painter,
    rect: Rect,
    (fraction, origin): (f32, f32),
    (accent, pointer): (Color32, Color32),
    hovered: bool,
) {
    let colours = palette();
    let centre = rect.center();
    let scale = rect.width() / KNOB_DRAWING;
    let arcs = knob_arcs(fraction, origin);
    let arc_radius = KNOB_ARC_RADIUS * scale;
    let arc_stroke = |shade| Stroke::new(KNOB_ARC_WIDTH * scale, shade);
    let (from, to) = arcs.track;
    painter.line(
        arc_points(centre, arc_radius, from, to),
        arc_stroke(colour(colours.arc_track)),
    );
    if let Some((from, to)) = arcs.value {
        painter.line(arc_points(centre, arc_radius, from, to), arc_stroke(accent));
    }
    let tick = Stroke::new(KNOB_TICK_WIDTH * scale, colour(colours.hover_boundary));
    for step in 0..KNOB_TICKS {
        let deg = from + KNOB_ARC_SWEEP_DEG * step as f32 / (KNOB_TICKS - 1) as f32;
        painter.line_segment(
            [
                point_at(centre, KNOB_TICK_INNER * scale, deg),
                point_at(centre, KNOB_TICK_OUTER * scale, deg),
            ],
            tick,
        );
    }
    painter.circle_filled(centre, KNOB_SKIRT_RADIUS * scale, colour(colours.well));
    let body = KNOB_BODY_RADIUS * scale;
    paint_knob_body(painter, centre, body);
    let rim = if hovered {
        colours.hover_boundary
    } else {
        colours.boundary
    };
    painter.circle_stroke(
        centre,
        body,
        Stroke::new(KNOB_RIM_WIDTH * scale, colour(rim)),
    );
    let to_screen = |(x, y): (f32, f32)| centre + vec2(x, y);
    painter.line_segment(
        [
            to_screen(knob_points(fraction, KNOB_POINTER_INNER * scale).pointer),
            to_screen(knob_points(fraction, KNOB_POINTER_OUTER * scale).pointer),
        ],
        Stroke::new(KNOB_POINTER_WIDTH * scale, pointer),
    );
}

/// A fan from a highlight up and left of the centre out to a dark edge.
fn paint_knob_body(painter: &Painter, centre: Pos2, radius: f32) {
    let colours = palette();
    let mut mesh = Mesh::default();
    mesh.colored_vertex(
        centre + KNOB_HIGHLIGHT_OFFSET * radius,
        colour(colours.knob_highlight),
    );
    let edge = colour(colours.knob_shadow);
    for point in arc_points(centre, radius, 0.0, 360.0) {
        mesh.colored_vertex(point, edge);
    }
    for index in 1..=ARC_SEGMENTS as u32 {
        mesh.add_triangle(0, index, index + 1);
    }
    painter.add(mesh);
}

/// Both steel ears, at the left and right edges of `unit`, each engraved with `position` between its two screws.
pub fn rack_ears(painter: &Painter, unit: Rect, position: Option<&str>) {
    let size = vec2(EAR_WIDTH, unit.height());
    for ear in [
        Rect::from_min_size(unit.min, size),
        Rect::from_min_size(pos2(unit.right() - EAR_WIDTH, unit.top()), size),
    ] {
        paint_ear(painter, ear);
        if let Some(position) = position {
            engrave(painter, ear.center(), position);
        }
    }
}

fn paint_ear(painter: &Painter, ear: Rect) {
    let colours = palette();
    let (edge, centre) = (colour(colours.ear_edge), colour(colours.ear_centre));
    let mut mesh = Mesh::default();
    for (x, shade) in [
        (ear.left(), edge),
        (ear.center().x, centre),
        (ear.right(), edge),
    ] {
        mesh.colored_vertex(pos2(x, ear.top()), shade);
        mesh.colored_vertex(pos2(x, ear.bottom()), shade);
    }
    for quad in [0, 2] {
        mesh.add_triangle(quad, quad + 1, quad + 2);
        mesh.add_triangle(quad + 1, quad + 2, quad + 3);
    }
    painter.add(mesh);
    for (x, y) in ear_holes(ear.height()) {
        painter.circle(
            ear.min + vec2(x, y),
            EAR_HOLE_RADIUS,
            colour(colours.well),
            Stroke::new(SCREW_RING_WIDTH, colour(colours.knob_highlight)),
        );
    }
}

/// Dark text over a light copy of it one pixel lower, as if cut into the steel.
fn engrave(painter: &Painter, centre: Pos2, text: &str) {
    let colours = palette();
    let font = theme::ear_number();
    painter.text(
        centre + vec2(0.0, ENGRAVE_OFFSET),
        Align2::CENTER_CENTER,
        text,
        font.clone(),
        colour(colours.ear_centre),
    );
    painter.text(
        centre,
        Align2::CENTER_CENTER,
        text,
        font,
        colour(colours.on_accent),
    );
}

/// Lit: filled in `colour_lit` inside a dark gap and a thin ring of it. Unlit: dark inside a ring of `colour_lit`.
pub fn lamp(painter: &Painter, centre: Pos2, colour_lit: Color32, lit: bool) {
    let dark = colour(palette().well);
    if !lit {
        painter.circle(
            centre,
            LAMP_RADIUS,
            dark,
            Stroke::new(LAMP_OFF_RING_WIDTH, colour_lit),
        );
        return;
    }
    painter.circle_filled(centre, LAMP_RADIUS, colour_lit);
    painter.circle_stroke(
        centre,
        LAMP_RADIUS + LAMP_GAP / 2.0,
        Stroke::new(LAMP_GAP, dark),
    );
    painter.circle_stroke(
        centre,
        LAMP_RADIUS + LAMP_GAP + LAMP_RING_WIDTH / 2.0,
        Stroke::new(LAMP_RING_WIDTH, colour_lit),
    );
}

/// A ladder of [`LADDER_SEGMENTS`] LEDs as wide as fits `width`, between the narrowest and widest segment.
pub fn led_ladder(ui: &mut Ui, width: f32, db: f64, clipped: bool) -> Response {
    let count = LADDER_SEGMENTS as f32;
    let (narrowest, widest) = LADDER_SEGMENT_WIDTHS;
    let segment = ((width + LADDER_GAP) / count - LADDER_GAP).clamp(narrowest, widest);
    let size = vec2(
        count * segment + (count - 1.0) * LADDER_GAP,
        LADDER_SEGMENT_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let colours = palette();
    let painter = ui.painter();
    for (index, led) in ladder_segments(db, clipped).iter().enumerate() {
        let left = rect.left() + index as f32 * (segment + LADDER_GAP);
        let fill = match (led.lit, led.band) {
            (false, _) => colours.arc_track,
            (true, Band::Green) => colours.go,
            (true, Band::Yellow) => colours.lit,
            (true, Band::Red) => colours.danger,
        };
        painter.rect_filled(
            Rect::from_min_size(pos2(left, rect.top()), vec2(segment, LADDER_SEGMENT_HEIGHT)),
            RADIUS_LED,
            colour(fill),
        );
    }
    response
}

pub fn tune_colour(in_tune: bool) -> Color32 {
    let colours = palette();
    colour(if in_tune { colours.go } else { colours.lit })
}

/// A half circle from −50 cents at the left to +50 at the right; the needle only when `fraction` is `Some`.
pub fn cents_arc(ui: &mut Ui, fraction: Option<f32>, colour: Color32) {
    let size = vec2(
        CENTS_ARC_DIAMETER,
        CENTS_ARC_DIAMETER / 2.0 + CENTS_ARC_WIDTH,
    );
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let center = pos2(rect.center().x, rect.bottom() - CENTS_ARC_WIDTH / 2.0);
    let radius = CENTS_ARC_DIAMETER / 2.0 - CENTS_ARC_WIDTH / 2.0;
    let painter = ui.painter();
    painter.line(
        arc_points(
            center,
            radius,
            CENTS_ARC_START_DEG,
            CENTS_ARC_START_DEG + CENTS_ARC_SWEEP_DEG,
        ),
        Stroke::new(CENTS_ARC_WIDTH, theme::colour(palette().arc_track)),
    );
    let Some(fraction) = fraction else {
        return;
    };
    let needle_deg = CENTS_ARC_START_DEG + CENTS_ARC_SWEEP_DEG * (fraction + 1.0) / 2.0;
    painter.line_segment(
        [center, point_at(center, radius, needle_deg)],
        Stroke::new(CENTS_NEEDLE_WIDTH, colour),
    );
}

/// A recessed strip from −50 cents at the left to +50 at the right: a tick every five cents, a tall one at 0, and the needle only when `fraction` is `Some`.
pub fn cents_strip(painter: &Painter, rect: Rect, fraction: Option<f32>, needle: Color32) {
    let colours = palette();
    painter.rect(
        rect,
        RADIUS_CONTROL,
        colour(colours.well),
        Stroke::new(1.0, colour(colours.boundary)),
        StrokeKind::Inside,
    );
    let inner = rect.shrink(CENTS_STRIP_INSET);
    let x_at = |fraction: f32| inner.left() + inner.width() * (fraction + 1.0) / 2.0;
    let centre = CENTS_TICKS / 2;
    for index in 0..CENTS_TICKS {
        let (height, tick) = if index == centre {
            (CENTS_CENTRE_TICK_HEIGHT, colours.text)
        } else {
            (CENTS_TICK_HEIGHT, colours.boundary)
        };
        let x = x_at(index as f32 / centre as f32 - 1.0);
        painter.vline(
            x,
            (inner.bottom() - height)..=inner.bottom(),
            Stroke::new(1.0, colour(tick)),
        );
    }
    if let Some(fraction) = fraction {
        painter.vline(
            x_at(fraction),
            inner.y_range(),
            Stroke::new(CENTS_NEEDLE_WIDTH, needle),
        );
    }
}

fn arc_points(center: Pos2, radius: f32, from_deg: f32, to_deg: f32) -> Vec<Pos2> {
    (0..=ARC_SEGMENTS)
        .map(|step| {
            let deg = from_deg + (to_deg - from_deg) * step as f32 / ARC_SEGMENTS as f32;
            point_at(center, radius, deg)
        })
        .collect()
}

// egui's y axis points down, so angles run clockwise: 135° is lower left, 270° the top, 405° lower right.
fn point_at(center: Pos2, radius: f32, deg: f32) -> Pos2 {
    let rad = deg.to_radians();
    center + radius * vec2(rad.cos(), rad.sin())
}

/// One choice of a choice Parameter, at `rect`: lit in the Module's accent with dark text, unlit as text on a dark face in a boundary edge, lighter under the pointer. Only the lit choice takes keyboard focus, so a group of them is one Tab stop. The caller gives it its accessible name and role.
pub fn choice_button(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    (text, lit): (&str, bool),
    accent: Color32,
) -> Response {
    let sense = if lit { Sense::click() } else { Sense::CLICK };
    let response = ui.interact(rect, id, sense);
    if response.has_focus() {
        let arrows_stay = EventFilter {
            horizontal_arrows: true,
            vertical_arrows: true,
            ..EventFilter::default()
        };
        ui.memory_mut(|memory| memory.set_focus_lock_filter(response.id, arrows_stay));
    }
    let colours = palette();
    let (fill, edge, text_colour) = if lit {
        (accent, accent, colour(colours.on_accent))
    } else {
        let (face, edge) = control_state(&response);
        (colour(face), colour(edge), ui.visuals().text_color())
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        RADIUS_CONTROL,
        fill,
        Stroke::new(1.0, edge),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme::value(),
        text_colour,
    );
    response
}
