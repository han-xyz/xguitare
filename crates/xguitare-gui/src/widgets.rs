use egui::{Align, Color32, Layout, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use xguitare_gui::model::{METER_RED_DB, knob_value_from_drag, meter_fraction};

const METER_SIZE: Vec2 = Vec2::new(120.0, 12.0);
const KNOB_DIAMETER: f32 = 48.0;
const KNOB_CELL_WIDTH: f32 = 72.0;
const KNOB_ARC_WIDTH: f32 = 4.0;
const KNOB_BODY_INSET: f32 = 7.0;
const KNOB_POINTER_WIDTH: f32 = 2.5;
const ARC_START_DEG: f32 = 135.0;
const ARC_SWEEP_DEG: f32 = 270.0;
const ARC_SEGMENTS: usize = 48;

pub struct KnobSpec<'a> {
    pub min: f32,
    pub max: f32,
    pub default: Option<f32>,
    pub integer: bool,
    pub label: &'a str,
    pub suffix: &'a str,
}

pub struct KnobResponse {
    pub changed: bool,
    pub drag_stopped: bool,
    pub reset: bool,
}

pub fn knob(ui: &mut Ui, value: &mut f32, spec: &KnobSpec) -> KnobResponse {
    let layout = Layout::top_down(Align::Center);
    ui.allocate_ui_with_layout(vec2(KNOB_CELL_WIDTH, 0.0), layout, |ui| {
        ui.label(spec.label);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::splat(KNOB_DIAMETER), Sense::click_and_drag());
        let before = *value;
        let unrounded_id = response.id;
        if response.drag_started() {
            ui.data_mut(|data| data.insert_temp(unrounded_id, *value));
        }
        if response.dragged() {
            let fine = ui.input(|input| input.modifiers.shift);
            let dy = response.drag_delta().y;
            let unrounded = ui
                .data(|data| data.get_temp(unrounded_id))
                .unwrap_or(*value);
            let (min, max) = (spec.min, spec.max);
            *value = knob_value_from_drag(unrounded, min, max, dy, fine, spec.integer);
            let next = knob_value_from_drag(unrounded, min, max, dy, fine, false);
            ui.data_mut(|data| data.insert_temp(unrounded_id, next));
        }
        let reset = response.double_clicked() && spec.default.is_some();
        if let Some(default) = spec.default.filter(|_| reset) {
            *value = default;
        }
        paint_knob(ui, rect, knob_fraction(*value, spec), response.hovered());
        ui.label(knob_text(*value, spec));
        KnobResponse {
            changed: reset || *value != before,
            drag_stopped: response.drag_stopped(),
            reset,
        }
    })
    .inner
}

fn knob_fraction(value: f32, spec: &KnobSpec) -> f32 {
    let range = spec.max - spec.min;
    if range <= 0.0 {
        return 0.0;
    }
    ((value - spec.min) / range).clamp(0.0, 1.0)
}

fn knob_text(value: f32, spec: &KnobSpec) -> String {
    if spec.integer {
        format!("{}{}", value.round(), spec.suffix)
    } else {
        format!("{value:.1}{}", spec.suffix)
    }
}

fn paint_knob(ui: &Ui, rect: Rect, fraction: f32, hovered: bool) {
    let visuals = ui.visuals();
    let widget = if hovered {
        &visuals.widgets.hovered
    } else {
        &visuals.widgets.inactive
    };
    let center = rect.center();
    let radius = rect.width() / 2.0 - KNOB_ARC_WIDTH / 2.0;
    let value_deg = ARC_START_DEG + ARC_SWEEP_DEG * fraction;
    let painter = ui.painter();
    painter.line(
        arc_points(center, radius, ARC_START_DEG, ARC_START_DEG + ARC_SWEEP_DEG),
        Stroke::new(KNOB_ARC_WIDTH, visuals.extreme_bg_color),
    );
    if fraction > 0.0 {
        painter.line(
            arc_points(center, radius, ARC_START_DEG, value_deg),
            Stroke::new(KNOB_ARC_WIDTH, visuals.selection.bg_fill),
        );
    }
    let body_radius = radius - KNOB_BODY_INSET;
    painter.circle_filled(center, body_radius, widget.bg_fill);
    painter.line_segment(
        [center, point_at(center, body_radius, value_deg)],
        Stroke::new(KNOB_POINTER_WIDTH, widget.fg_stroke.color),
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

pub fn meter(ui: &mut Ui, label: &str, fraction: f32) {
    ui.label(label);
    let (rect, _) = ui.allocate_exact_size(METER_SIZE, Sense::hover());
    let red_from = meter_fraction(METER_RED_DB);
    let x = |fraction: f32| rect.left() + rect.width() * fraction;
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, Color32::DARK_GRAY);
    let green_to = fraction.min(red_from);
    painter.rect_filled(
        Rect::from_min_max(rect.min, pos2(x(green_to), rect.max.y)),
        0.0,
        Color32::GREEN,
    );
    if fraction > red_from {
        painter.rect_filled(
            Rect::from_min_max(pos2(x(red_from), rect.min.y), pos2(x(fraction), rect.max.y)),
            0.0,
            Color32::RED,
        );
    }
}
