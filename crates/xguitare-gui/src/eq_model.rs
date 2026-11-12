use xguitare_core::{EQ_MAX_FREQ_PER_RATE, eq_response_db};

use crate::model::{CLOSED_KNOB_WIDTH, ParamRow, format_value};

pub const EQ: &str = "eq";
pub const EQ_MIN_HZ: f32 = 10.0;
pub const EQ_MAX_HZ: f32 = 20_000.0;
/// The plot spans this many dB above and below 0 dB.
pub const EQ_RANGE_DB: f32 = 18.0;
pub const EQ_GRID_DB: f32 = 6.0;
pub const EQ_FREQ_LINES: [(f32, &str); 9] = [
    (20.0, "20"),
    (50.0, "50"),
    (100.0, "100"),
    (200.0, "200"),
    (500.0, "500"),
    (1_000.0, "1k"),
    (2_000.0, "2k"),
    (5_000.0, "5k"),
    (10_000.0, "10k"),
];
pub const EQ_PLOT_HEIGHT: f32 = 120.0;
/// The plot grows with its band columns up to this height.
pub const EQ_PLOT_MAX_HEIGHT: f32 = 200.0;
/// The dB labels' column left of the plot frame.
pub const EQ_GUTTER: f32 = 36.0;
pub const EQ_AXIS_LABEL_HEIGHT: f32 = 16.0;
pub const EQ_BAND_TITLE_HEIGHT: f32 = 20.0;
/// One small knob with its label and value field, and the gap below it.
pub const EQ_BAND_KNOB_PITCH: f32 = 52.0;
pub const EQ_SECTION_GAP: f32 = 8.0;
/// The plot keeps at least this width beside its band columns; with less, the columns stack.
pub const EQ_PLOT_MIN_WIDTH: f32 = 320.0;
/// Band columns side by side, tried widest first.
const EQ_BAND_COLUMNS: [usize; 3] = [4, 2, 1];
pub const EQ_FLAT: &str = "flat";
/// The gain and frequency Parameters of each band, in band order.
const SUMMARY_BANDS: [(&str, &str); 4] = [
    ("low_gain", "low_freq"),
    ("mid1_gain", "mid1_freq"),
    ("mid2_gain", "mid2_freq"),
    ("high_gain", "high_freq"),
];
pub const EQ_MINI_WIDTH: f32 = 160.0;
pub const EQ_MINI_HEIGHT: f32 = 36.0;
pub const EQ_CURVE_POINTS: usize = 241;
pub const EQ_MINI_POINTS: usize = 81;
/// The header row of an open unit with the unit's top and bottom margins.
const UNIT_FRAME_HEIGHT: f32 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clip {
    None,
    Above,
    Below,
}

/// A point of the curve: `x` from 0 at 10 Hz to 1 at 20 kHz, `y` from 0 at +18 dB to 1 at −18 dB, held at the frame edge past those.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePoint {
    pub freq_hz: f32,
    pub x: f32,
    pub y: f32,
    pub clip: Clip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqDot {
    LowCut,
    Low,
    Mid1,
    Mid2,
    High,
    HighCut,
}

const DOT_FREQS: [(EqDot, &str); 6] = [
    (EqDot::LowCut, "low_cut"),
    (EqDot::Low, "low_freq"),
    (EqDot::Mid1, "mid1_freq"),
    (EqDot::Mid2, "mid2_freq"),
    (EqDot::High, "high_freq"),
    (EqDot::HighCut, "high_cut"),
];

/// A band's dot on the curve; `half` when it sits on an axis end, `ringed` for the band whose knob is in use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DotPoint {
    pub dot: EqDot,
    pub x: f32,
    pub y: f32,
    pub half: bool,
    pub ringed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EqBand {
    pub title: &'static str,
    pub knobs: Vec<EqKnob>,
}

/// A knob in a band column: its row in the eq's rows, the label it shows and the name it has for assistive technology.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqKnob {
    pub index: usize,
    pub label: &'static str,
    pub name: &'static str,
}

type BandTable = [(
    &'static str,
    &'static [(&'static str, &'static str, &'static str)],
); 4];

const BANDS: BandTable = [
    (
        "Low",
        &[
            ("low_cut", "Low cut", "Low cut"),
            ("low_freq", "Freq", "Low Freq"),
            ("low_gain", "Gain", "Low Gain"),
        ],
    ),
    (
        "Mid 1",
        &[
            ("mid1_freq", "Freq", "Mid 1 Freq"),
            ("mid1_gain", "Gain", "Mid 1 Gain"),
            ("mid1_q", "Q", "Mid 1 Q"),
        ],
    ),
    (
        "Mid 2",
        &[
            ("mid2_freq", "Freq", "Mid 2 Freq"),
            ("mid2_gain", "Gain", "Mid 2 Gain"),
            ("mid2_q", "Q", "Mid 2 Q"),
        ],
    ),
    (
        "High",
        &[
            ("high_freq", "Freq", "High Freq"),
            ("high_gain", "Gain", "High Gain"),
            ("high_cut", "High cut", "High cut"),
            ("output", "Output", "Output"),
        ],
    ),
];

pub fn eq_x(freq_hz: f32) -> f32 {
    ((freq_hz / EQ_MIN_HZ).ln() / (EQ_MAX_HZ / EQ_MIN_HZ).ln()).clamp(0.0, 1.0)
}

pub fn eq_y(db: f32) -> f32 {
    ((EQ_RANGE_DB - db) / (2.0 * EQ_RANGE_DB)).clamp(0.0, 1.0)
}

fn values(rows: &[ParamRow]) -> Vec<f32> {
    rows.iter().map(|row| row.value as f32).collect()
}

/// The highest frequency the curve shows: 20 kHz, or lower where the Module clamps its filters at a low sample rate.
fn plotted_max_hz(sample_rate: f32) -> f32 {
    EQ_MAX_HZ.min(EQ_MAX_FREQ_PER_RATE * sample_rate)
}

/// `count` points of the eq's response, Output included, log-spaced from 10 Hz to 20 kHz or the Module's clamp below it; `rows` are the eq's Parameters in spec order.
pub fn eq_curve(rows: &[ParamRow], sample_rate: f32, count: usize) -> Vec<CurvePoint> {
    let values = values(rows);
    let top = plotted_max_hz(sample_rate);
    let last = count.saturating_sub(1).max(1) as f32;
    (0..count)
        .map(|index| {
            let freq_hz = EQ_MIN_HZ * (top / EQ_MIN_HZ).powf(index as f32 / last);
            let x = eq_x(freq_hz);
            let db = eq_response_db(&values, freq_hz, sample_rate);
            let clip = if db > EQ_RANGE_DB {
                Clip::Above
            } else if db < -EQ_RANGE_DB {
                Clip::Below
            } else {
                Clip::None
            };
            CurvePoint {
                freq_hz,
                x,
                y: eq_y(db),
                clip,
            }
        })
        .collect()
}

/// One dot per band at its frequency, on the curve.
pub fn eq_dots(rows: &[ParamRow], sample_rate: f32, active: Option<EqDot>) -> Vec<DotPoint> {
    let values = values(rows);
    DOT_FREQS
        .iter()
        .filter_map(|&(dot, param)| {
            let freq = rows.iter().find(|row| row.param == param)?.value as f32;
            Some(DotPoint {
                dot,
                x: eq_x(freq),
                y: eq_y(eq_response_db(
                    &values,
                    freq.min(plotted_max_hz(sample_rate)),
                    sample_rate,
                )),
                half: freq <= EQ_MIN_HZ || freq >= EQ_MAX_HZ,
                ringed: active == Some(dot),
            })
        })
        .collect()
}

/// The dot a knob of Parameter `param` moves; none for `output`.
pub fn eq_dot_of_param(param: &str) -> Option<EqDot> {
    match param {
        "low_cut" => Some(EqDot::LowCut),
        "high_cut" => Some(EqDot::HighCut),
        _ if param.starts_with("low_") => Some(EqDot::Low),
        _ if param.starts_with("mid1_") => Some(EqDot::Mid1),
        _ if param.starts_with("mid2_") => Some(EqDot::Mid2),
        _ if param.starts_with("high_") => Some(EqDot::High),
        _ => None,
    }
}

/// The four band columns under the curve, each a title over its knobs.
pub fn eq_bands(rows: &[ParamRow]) -> Vec<EqBand> {
    BANDS
        .iter()
        .map(|&(title, knobs)| EqBand {
            title,
            knobs: knobs
                .iter()
                .filter_map(|&(param, label, name)| {
                    let index = rows.iter().position(|row| row.param == param)?;
                    Some(EqKnob { index, label, name })
                })
                .collect(),
        })
        .collect()
}

/// An open eq's plot beside its band columns: `columns` bands abreast in rows, the plot `plot_width` wide (its dB gutter included) and `plot_height` tall, as tall as the band rows within [`EQ_PLOT_HEIGHT`]..=[`EQ_PLOT_MAX_HEIGHT`]; `height` is the unit's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqLayout {
    pub columns: usize,
    pub plot_width: f32,
    pub plot_height: f32,
    pub height: f32,
}

/// The layout of an open eq whose content is `width` wide: four, two or one band columns, the most that leave the plot [`EQ_PLOT_MIN_WIDTH`].
pub fn eq_layout(bands: &[EqBand], width: f32) -> EqLayout {
    let beside = |columns: usize| width - columns as f32 * CLOSED_KNOB_WIDTH - 2.0 * EQ_SECTION_GAP;
    let columns = EQ_BAND_COLUMNS
        .into_iter()
        .find(|&columns| beside(columns) >= EQ_PLOT_MIN_WIDTH)
        .unwrap_or(1);
    let rows = bands.chunks(columns.max(1));
    let row_count = rows.len();
    let bands_height = rows
        .map(|row| {
            let knobs = row.iter().map(|band| band.knobs.len()).max().unwrap_or(0);
            EQ_BAND_TITLE_HEIGHT + knobs as f32 * EQ_BAND_KNOB_PITCH
        })
        .sum::<f32>()
        + row_count.saturating_sub(1) as f32 * EQ_SECTION_GAP;
    let plot_height =
        (bands_height - EQ_AXIS_LABEL_HEIGHT).clamp(EQ_PLOT_HEIGHT, EQ_PLOT_MAX_HEIGHT);
    EqLayout {
        columns,
        plot_width: beside(columns).max(0.0),
        plot_height,
        height: UNIT_FRAME_HEIGHT
            + EQ_SECTION_GAP
            + (plot_height + EQ_AXIS_LABEL_HEIGHT).max(bands_height)
            + EQ_SECTION_GAP,
    }
}

/// A closed eq's one-line summary: each band that cuts or boosts as `+4 @100`, gains in whole or tenth dB, frequencies in Hz or `k`; `flat` when none does.
pub fn eq_summary(rows: &[ParamRow]) -> String {
    let value = |param: &str| {
        rows.iter()
            .find(|row| row.param == param)
            .map(|row| row.value)
    };
    let parts: Vec<String> = SUMMARY_BANDS
        .iter()
        .filter_map(|&(gain, freq)| {
            let tenths = (value(gain)? * 10.0).round();
            if tenths == 0.0 {
                return None;
            }
            Some(format!(
                "{} @{}",
                gain_text(tenths),
                freq_text(value(freq)?)
            ))
        })
        .collect();
    if parts.is_empty() {
        return EQ_FLAT.to_string();
    }
    parts.join(" ")
}

fn gain_text(tenths: f64) -> String {
    let sign = if tenths > 0.0 { "+" } else { "\u{2212}" };
    format!("{sign}{}", tenths.abs() / 10.0)
}

fn freq_text(hz: f64) -> String {
    let hz = hz.round();
    if hz < 1000.0 {
        return format!("{hz}");
    }
    format!("{}k", (hz / 100.0).round() / 10.0)
}

/// The plot's accessible name: every band's values in words.
pub fn eq_curve_name(rows: &[ParamRow]) -> String {
    let text = |param: &str| {
        rows.iter()
            .find(|row| row.param == param)
            .map(|row| format_value(row, row.value))
            .unwrap_or_default()
    };
    format!(
        "EQ curve: Low cut {}; Low {} at {}; Mid 1 {} at {}, Q {}; Mid 2 {} at {}, Q {}; High {} at {}; High cut {}; Output {}",
        text("low_cut"),
        text("low_gain"),
        text("low_freq"),
        text("mid1_gain"),
        text("mid1_freq"),
        text("mid1_q"),
        text("mid2_gain"),
        text("mid2_freq"),
        text("mid2_q"),
        text("high_gain"),
        text("high_freq"),
        text("high_cut"),
        text("output"),
    )
}
