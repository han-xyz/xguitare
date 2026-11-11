use xguitare_core::ModuleRegistry;
use xguitare_gui::eq_model::{
    Clip, EqDot, eq_bands, eq_curve, eq_curve_name, eq_dot_of_param, eq_dots, eq_open_height, eq_x,
    eq_y,
};
use xguitare_gui::model::{OPEN_UNIT_HEIGHT, ParamRow};

const RATE: f32 = 48_000.0;
const POINTS: usize = 241;

fn eq_rows(values: &[(&str, f64)]) -> Vec<ParamRow> {
    let registry = ModuleRegistry::default();
    registry
        .spec("eq")
        .unwrap()
        .params
        .iter()
        .map(|spec| ParamRow {
            address: format!("eq.{}", spec.name),
            module: "eq".to_string(),
            module_type: "eq".to_string(),
            param: spec.name.to_string(),
            value: values
                .iter()
                .find(|(name, _)| *name == spec.name)
                .map_or(f64::from(spec.default), |(_, value)| *value),
            min: f64::from(spec.min),
            max: f64::from(spec.max),
            default: f64::from(spec.default),
            unit: match format!("{:?}", spec.unit).as_str() {
                "Db" => "dB".to_string(),
                "Hz" => "Hz".to_string(),
                _ => String::new(),
            },
            integer: spec.integer,
            labels: Vec::new(),
        })
        .collect()
}

/// Blueprint frame 3: Low +4 dB at 100 Hz, Mid 1 −3 dB at 400 Hz, Mid 2 +2 dB at 2.5 kHz, Low cut 40 Hz, High cut 12 kHz.
fn frame_three() -> Vec<ParamRow> {
    eq_rows(&[
        ("low_cut", 40.0),
        ("low_gain", 4.0),
        ("mid1_gain", -3.0),
        ("mid2_gain", 2.0),
        ("high_cut", 12_000.0),
    ])
}

#[test]
fn curve_points_are_log_spaced_from_10_hz_to_20_khz() {
    let curve = eq_curve(&eq_rows(&[]), RATE, POINTS);
    assert_eq!(curve.len(), POINTS);
    assert!((curve[0].freq_hz - 10.0).abs() < 1e-3);
    assert!((curve[POINTS - 1].freq_hz - 20_000.0).abs() < 1e-1);
    let ratio = curve[1].freq_hz / curve[0].freq_hz;
    for (index, pair) in curve.windows(2).enumerate() {
        assert!(
            (pair[1].freq_hz / pair[0].freq_hz - ratio).abs() < 1e-4,
            "{index}"
        );
        assert!((pair[0].x - index as f32 / (POINTS - 1) as f32).abs() < 1e-5);
    }
    assert_eq!(eq_x(10.0), 0.0);
    assert_eq!(eq_x(20_000.0), 1.0);
}

#[test]
fn curve_stops_where_the_module_clamps_its_filters() {
    let rate = 32_000.0;
    let curve = eq_curve(&eq_rows(&[]), rate, POINTS);
    let last = curve.last().unwrap();
    assert!((last.freq_hz - 0.45 * rate).abs() < 1e-1, "{last:?}");
    assert!((last.x - eq_x(0.45 * rate)).abs() < 1e-5);
    assert!(
        curve
            .iter()
            .all(|point| point.y.is_finite() && point.clip == Clip::None)
    );
    let high_cut = eq_dots(&eq_rows(&[]), rate, None).pop().unwrap();
    assert!((high_cut.y - 0.5).abs() < 0.1, "{high_cut:?}");
}

#[test]
fn defaults_draw_a_flat_line_in_the_middle() {
    let curve = eq_curve(&eq_rows(&[]), RATE, POINTS);
    let middle = curve
        .iter()
        .filter(|point| (30.0..=14_000.0).contains(&point.freq_hz));
    for point in middle {
        assert!((point.y - 0.5).abs() < 0.01, "{point:?}");
        assert_eq!(point.clip, Clip::None);
    }
    assert_eq!(eq_y(18.0), 0.0);
    assert_eq!(eq_y(-18.0), 1.0);
}

#[test]
fn past_18_db_clips_to_the_frame_edge() {
    let boosted = eq_curve(
        &eq_rows(&[("low_gain", 15.0), ("output", 12.0)]),
        RATE,
        POINTS,
    );
    let low = boosted.iter().find(|point| point.freq_hz >= 30.0).unwrap();
    assert_eq!((low.y, low.clip), (0.0, Clip::Above));
    let cut = eq_curve(&eq_rows(&[("high_cut", 1_000.0)]), RATE, POINTS);
    let top = cut.last().unwrap();
    assert_eq!((top.y, top.clip), (1.0, Clip::Below));
    assert_eq!(cut[0].clip, Clip::None);
}

#[test]
fn dots_sit_on_the_curve_at_each_band() {
    let dots = eq_dots(&frame_three(), RATE, None);
    let kinds: Vec<EqDot> = dots.iter().map(|dot| dot.dot).collect();
    assert_eq!(
        kinds,
        [
            EqDot::LowCut,
            EqDot::Low,
            EqDot::Mid1,
            EqDot::Mid2,
            EqDot::High,
            EqDot::HighCut
        ]
    );
    let at = |kind: EqDot| dots.iter().find(|dot| dot.dot == kind).unwrap();
    assert!((at(EqDot::Mid1).x - eq_x(400.0)).abs() < 1e-5);
    assert!(at(EqDot::Mid1).y > 0.5, "Mid 1 cuts");
    assert!(at(EqDot::Low).y < 0.5, "Low boosts");
    assert!(at(EqDot::HighCut).y > 0.5, "High cut is down at its cutoff");
    assert!(dots.iter().all(|dot| !dot.half && !dot.ringed));
}

#[test]
fn cuts_at_the_axis_ends_draw_half_dots() {
    let dots = eq_dots(&eq_rows(&[]), RATE, None);
    let halves: Vec<(EqDot, f32)> = dots
        .iter()
        .filter(|dot| dot.half)
        .map(|dot| (dot.dot, dot.x))
        .collect();
    assert_eq!(halves, [(EqDot::LowCut, 0.0), (EqDot::HighCut, 1.0)]);
}

#[test]
fn the_band_of_the_active_knob_rings_its_dot() {
    let active = eq_dot_of_param("mid1_freq");
    assert_eq!(active, Some(EqDot::Mid1));
    let dots = eq_dots(&frame_three(), RATE, active);
    let ringed: Vec<EqDot> = dots
        .iter()
        .filter(|dot| dot.ringed)
        .map(|dot| dot.dot)
        .collect();
    assert_eq!(ringed, [EqDot::Mid1]);
    assert_eq!(eq_dot_of_param("low_cut"), Some(EqDot::LowCut));
    assert_eq!(eq_dot_of_param("low_gain"), Some(EqDot::Low));
    assert_eq!(eq_dot_of_param("mid2_q"), Some(EqDot::Mid2));
    assert_eq!(eq_dot_of_param("high_freq"), Some(EqDot::High));
    assert_eq!(eq_dot_of_param("high_cut"), Some(EqDot::HighCut));
    assert_eq!(eq_dot_of_param("output"), None);
}

#[test]
fn band_columns_group_the_knobs() {
    let rows = eq_rows(&[]);
    let bands = eq_bands(&rows);
    let columns: Vec<(&str, Vec<(&str, &str)>)> = bands
        .iter()
        .map(|band| {
            let knobs = band
                .knobs
                .iter()
                .map(|knob| (rows[knob.index].param.as_str(), knob.label))
                .collect();
            (band.title, knobs)
        })
        .collect();
    assert_eq!(
        columns,
        [
            (
                "Low",
                vec![
                    ("low_cut", "Low cut"),
                    ("low_freq", "Freq"),
                    ("low_gain", "Gain")
                ]
            ),
            (
                "Mid 1",
                vec![
                    ("mid1_freq", "Freq"),
                    ("mid1_gain", "Gain"),
                    ("mid1_q", "Q")
                ]
            ),
            (
                "Mid 2",
                vec![
                    ("mid2_freq", "Freq"),
                    ("mid2_gain", "Gain"),
                    ("mid2_q", "Q")
                ]
            ),
            (
                "High",
                vec![
                    ("high_freq", "Freq"),
                    ("high_gain", "Gain"),
                    ("high_cut", "High cut"),
                    ("output", "Output")
                ]
            ),
        ]
    );
    let names: Vec<&str> = bands[1].knobs.iter().map(|knob| knob.name).collect();
    assert_eq!(names, ["Mid 1 Freq", "Mid 1 Gain", "Mid 1 Q"]);
}

#[test]
fn open_eq_grows_with_its_longest_band_column() {
    let bands = eq_bands(&eq_rows(&[]));
    let height = eq_open_height(&bands);
    assert!(height > OPEN_UNIT_HEIGHT);
    let fewer = eq_open_height(&bands[..3]);
    assert!(height > fewer, "the High column has one knob more");
}

#[test]
fn plot_name_lists_the_band_values() {
    assert_eq!(
        eq_curve_name(&frame_three()),
        "EQ curve: Low cut 40 Hz; Low 4 dB at 100 Hz; Mid 1 −3 dB at 400 Hz, Q 0.71; \
         Mid 2 2 dB at 2.5 kHz, Q 0.71; High 0 dB at 8 kHz; High cut 12 kHz; Output 0 dB"
    );
}
