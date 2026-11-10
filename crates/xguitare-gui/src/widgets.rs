use egui::{
    Color32, EventFilter, Mesh, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, Visuals,
    pos2, vec2,
};

use crate::model::{
    Band, EAR_HOLE_RADIUS, EAR_WIDTH, LADDER_SEGMENTS, Rgb, accent_for, ear_holes, knob_points,
    knob_value_from_drag, ladder_segments, palette,
};

const ARC_SEGMENTS: usize = 48;
const CENTS_ARC_DIAMETER: f32 = 160.0;
const CENTS_ARC_WIDTH: f32 = 4.0;
const CENTS_NEEDLE_WIDTH: f32 = 3.0;
const CENTS_ARC_START_DEG: f32 = 180.0;
const CENTS_ARC_SWEEP_DEG: f32 = 180.0;
const SMALL_KNOB_DIAMETER: f32 = 34.0;
const LARGE_KNOB_DIAMETER: f32 = 76.0;
const RACK_KNOB_BODY: f32 = 0.85;
const RACK_KNOB_POINTER: f32 = 0.65;
const RACK_KNOB_RIM_WIDTH: f32 = 0.0375;
const RACK_KNOB_POINTER_WIDTH: f32 = 0.075;
const RACK_KNOB_ARC_WIDTH: f32 = 0.05;
const FOCUS_RING_GAP: f32 = 2.0;
const FOCUS_RING_WIDTH: f32 = 1.5;
const LAMP_RADIUS: f32 = 5.0;
const LAMP_GLOW_RADIUS: f32 = 9.0;
const LAMP_GLOW_ALPHA: f32 = 0.3;
const LADDER_SEGMENT_SIZE: Vec2 = Vec2::new(14.0, 10.0);
const LADDER_GAP: f32 = 3.0;
const LADDER_SEGMENT_RADIUS: u8 = 1;
const IN_TUNE_COLOUR: Color32 = Color32::GREEN;
const OUT_OF_TUNE_COLOUR: Color32 = Color32::from_rgb(255, 176, 0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnobSize {
    Small,
    Large,
}

impl KnobSize {
    fn diameter(self) -> f32 {
        match self {
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

pub fn colour((r, g, b): Rgb) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn accent(type_name: &str) -> Color32 {
    colour(accent_for(type_name))
}

pub fn visuals() -> Visuals {
    let colours = palette();
    let mut visuals = Visuals::dark();
    visuals.panel_fill = colour(colours.ground);
    visuals.window_fill = colour(colours.sidebar);
    visuals.extreme_bg_color = colour(colours.knob_body);
    visuals.faint_bg_color = colour(colours.unit);
    visuals.override_text_color = Some(colour(colours.text));
    visuals.selection.bg_fill = colour(colours.button_border);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = colour(colours.unit);
    widgets.noninteractive.bg_stroke.color = colour(colours.edge_light);
    widgets.noninteractive.fg_stroke.color = colour(colours.muted);
    for state in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
    ] {
        state.bg_stroke.color = colour(colours.button_border);
        state.fg_stroke.color = colour(colours.text);
    }
    widgets.inactive.bg_fill = colour(colours.unit_open);
    widgets.inactive.weak_bg_fill = colour(colours.unit_open);
    widgets.hovered.weak_bg_fill = colour(colours.edge_light);
    widgets.active.weak_bg_fill = colour(colours.button_border);
    visuals
}

pub fn rack_knob(ui: &mut Ui, size: KnobSize, fraction: f32, accent: Color32) -> RackKnob {
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
    paint_rack_knob(ui.painter(), rect, next, accent, response.hovered());
    if focused {
        ui.painter().circle_stroke(
            rect.center(),
            rect.width() / 2.0 + FOCUS_RING_GAP,
            Stroke::new(FOCUS_RING_WIDTH, colour(palette().text)),
        );
    }
    RackKnob {
        response,
        fraction: next,
        reset,
    }
}

fn paint_rack_knob(painter: &Painter, rect: Rect, fraction: f32, accent: Color32, hovered: bool) {
    let colours = palette();
    let centre = rect.center();
    let diameter = rect.width();
    let outer = diameter / 2.0;
    let arc_width = diameter * RACK_KNOB_ARC_WIDTH;
    let to_screen = |(x, y): (f32, f32)| centre + vec2(x, y);
    let rim = if hovered {
        colours.muted
    } else {
        colours.knob_rim
    };
    painter.circle(
        centre,
        outer * RACK_KNOB_BODY,
        colour(colours.knob_body),
        Stroke::new(diameter * RACK_KNOB_RIM_WIDTH, colour(rim)),
    );
    if fraction > 0.0 {
        let arc = knob_points(fraction, outer - arc_width / 2.0).arc;
        painter.line(
            arc.into_iter().map(to_screen).collect(),
            Stroke::new(arc_width, accent),
        );
    }
    let pointer = knob_points(fraction, outer * RACK_KNOB_POINTER).pointer;
    painter.line_segment(
        [centre, to_screen(pointer)],
        Stroke::new(diameter * RACK_KNOB_POINTER_WIDTH, accent),
    );
}

/// Both silver ears, at the left and right edges of `unit`.
pub fn rack_ears(painter: &Painter, unit: Rect) {
    let size = vec2(EAR_WIDTH, unit.height());
    paint_ear(painter, Rect::from_min_size(unit.min, size));
    paint_ear(
        painter,
        Rect::from_min_size(pos2(unit.right() - EAR_WIDTH, unit.top()), size),
    );
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
        painter.circle_filled(
            ear.min + vec2(x, y),
            EAR_HOLE_RADIUS,
            colour(colours.ear_hole),
        );
    }
}

pub fn lamp(painter: &Painter, centre: Pos2, colour_lit: Color32, lit: bool) {
    if !lit {
        painter.circle_filled(centre, LAMP_RADIUS, colour(palette().led_off));
        return;
    }
    painter.circle_filled(
        centre,
        LAMP_GLOW_RADIUS,
        colour_lit.gamma_multiply(LAMP_GLOW_ALPHA),
    );
    painter.circle_filled(centre, LAMP_RADIUS, colour_lit);
}

pub fn led_ladder(ui: &mut Ui, db: f64, clipped: bool) -> Response {
    let count = LADDER_SEGMENTS as f32;
    let size = vec2(
        count * LADDER_SEGMENT_SIZE.x + (count - 1.0) * LADDER_GAP,
        LADDER_SEGMENT_SIZE.y,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let colours = palette();
    let painter = ui.painter();
    for (index, segment) in ladder_segments(db, clipped).iter().enumerate() {
        let left = rect.left() + index as f32 * (LADDER_SEGMENT_SIZE.x + LADDER_GAP);
        let fill = match (segment.lit, segment.band) {
            (false, _) => colours.led_off,
            (true, Band::Green) => colours.green,
            (true, Band::Yellow) => colours.yellow,
            (true, Band::Red) => colours.red,
        };
        painter.rect_filled(
            Rect::from_min_size(pos2(left, rect.top()), LADDER_SEGMENT_SIZE),
            LADDER_SEGMENT_RADIUS,
            colour(fill),
        );
    }
    response
}

pub fn tune_colour(in_tune: bool) -> Color32 {
    if in_tune {
        IN_TUNE_COLOUR
    } else {
        OUT_OF_TUNE_COLOUR
    }
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
        Stroke::new(CENTS_ARC_WIDTH, ui.visuals().extreme_bg_color),
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
