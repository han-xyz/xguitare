use egui::{Color32, Rect, Sense, Ui, Vec2, pos2};
use xguitare_gui::model::{METER_RED_DB, meter_fraction};

const METER_SIZE: Vec2 = Vec2::new(120.0, 12.0);

/// A horizontal bar filled to `fraction`: green up to the −6 dB mark, red above, on grey.
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
