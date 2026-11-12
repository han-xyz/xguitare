use egui::{Align2, Color32, Painter, Pos2, Rect, Stroke, pos2};

use crate::eq_model::{
    Clip, CurvePoint, DotPoint, EQ_FREQ_LINES, EQ_GRID_DB, EQ_GUTTER, EQ_PLOT_HEIGHT, EQ_RANGE_DB,
    eq_x, eq_y,
};
use crate::theme::{self, colour, palette};

const CURVE_WIDTH: f32 = 2.0;
const MINI_CURVE_WIDTH: f32 = 1.5;
const EDGE_TICK_WIDTH: f32 = 2.0;
const DOT_RADIUS: f32 = 4.0;
const RING_RADIUS: f32 = 7.0;
const RING_WIDTH: f32 = 2.0;
const AXIS_TEXT_GAP: f32 = 4.0;
const AXIS_TEXT_TOP: f32 = 1.0;

/// The curve and dots the plot draws over its grid; none while the Daemon is disconnected.
pub struct PlotLines<'a> {
    pub curve: &'a [CurvePoint],
    pub dots: &'a [DotPoint],
}

/// Two painters: `painter` dims with a bypassed unit, `zero_line` draws the 0 dB line at full strength.
pub struct EqPainters<'a> {
    pub painter: &'a Painter,
    pub zero_line: &'a Painter,
}

/// The open eq's plot in `area`: the dB labels in the gutter at its left, the frame with its grid, the frequency labels below it, then the curve and band dots in `accent`.
pub fn paint_plot(painters: &EqPainters, area: Rect, lines: Option<PlotLines>, accent: Color32) {
    let frame = Rect::from_min_max(
        pos2(area.left() + EQ_GUTTER, area.top()),
        pos2(area.right(), area.top() + EQ_PLOT_HEIGHT),
    );
    paint_grid(painters.painter, frame);
    paint_zero_line(painters.zero_line, frame);
    let Some(lines) = lines else {
        return;
    };
    paint_curve(painters.painter, frame, lines.curve, CURVE_WIDTH, accent);
    let clipped = painters.painter.with_clip_rect(frame);
    for dot in lines.dots {
        let centre = at(frame, dot.x, dot.y);
        clipped.circle_filled(centre, DOT_RADIUS, accent);
        if dot.ringed {
            clipped.circle_stroke(centre, RING_RADIUS, Stroke::new(RING_WIDTH, accent));
        }
    }
}

/// The closed eq's curve in `rect`, without grid, labels or dots.
pub fn paint_mini(
    painters: &EqPainters,
    rect: Rect,
    curve: Option<&[CurvePoint]>,
    accent: Color32,
) {
    painters
        .painter
        .rect_filled(rect, 0, colour(palette().well));
    paint_zero_line(painters.zero_line, rect);
    if let Some(curve) = curve {
        paint_curve(painters.painter, rect, curve, MINI_CURVE_WIDTH, accent);
    }
}

fn at(frame: Rect, x: f32, y: f32) -> Pos2 {
    pos2(
        frame.left() + x * frame.width(),
        frame.top() + y * frame.height(),
    )
}

fn paint_grid(painter: &Painter, frame: Rect) {
    let colours = palette();
    let grid = Stroke::new(1.0, colour(colours.hover));
    let text = |pos: Pos2, align: Align2, label: &str| {
        painter.text(pos, align, label, theme::value(), colour(colours.muted));
    };
    painter.rect_filled(frame, 0, colour(colours.well));
    let steps = (EQ_RANGE_DB / EQ_GRID_DB) as i32;
    for step in -steps..=steps {
        let db = step as f32 * EQ_GRID_DB;
        let y = frame.top() + eq_y(db) * frame.height();
        if step != 0 {
            painter.hline(frame.x_range(), y, grid);
        }
        let label = format!("{:+}", db as i32)
            .replace('-', "−")
            .replace("+0", "0");
        text(
            pos2(frame.left() - AXIS_TEXT_GAP, y),
            Align2::RIGHT_CENTER,
            &label,
        );
    }
    let last = EQ_FREQ_LINES.len() - 1;
    for (index, (freq, label)) in EQ_FREQ_LINES.into_iter().enumerate() {
        let x = frame.left() + eq_x(freq) * frame.width();
        painter.vline(x, frame.y_range(), grid);
        let align = if index == last {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        text(pos2(x, frame.bottom() + AXIS_TEXT_TOP), align, label);
    }
}

fn paint_zero_line(painter: &Painter, frame: Rect) {
    let y = frame.top() + eq_y(0.0) * frame.height();
    painter.hline(
        frame.x_range(),
        y,
        Stroke::new(1.0, colour(palette().muted)),
    );
}

/// The curve inside `frame`; a stretch held at the top or bottom edge draws as a tick along that edge.
fn paint_curve(painter: &Painter, frame: Rect, curve: &[CurvePoint], width: f32, accent: Color32) {
    let points = curve
        .iter()
        .map(|point| at(frame, point.x, point.y))
        .collect();
    painter
        .with_clip_rect(frame)
        .line(points, Stroke::new(width, accent));
    let tick = Stroke::new(EDGE_TICK_WIDTH, accent);
    for pair in curve.windows(2) {
        let edge_y = match (pair[0].clip, pair[1].clip) {
            (Clip::Above, Clip::Above) => frame.top() + EDGE_TICK_WIDTH / 2.0,
            (Clip::Below, Clip::Below) => frame.bottom() - EDGE_TICK_WIDTH / 2.0,
            _ => continue,
        };
        let x = |point: &CurvePoint| frame.left() + point.x * frame.width();
        painter.line_segment([pos2(x(&pair[0]), edge_y), pos2(x(&pair[1]), edge_y)], tick);
    }
}
