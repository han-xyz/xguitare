use std::fs;
use std::path::Path;

use egui::{Color32, Context, FontDefinitions, FontId, RawInput, Vec2};
use xguitare_gui::theme::{
    self, ContrastPair, NEUTRAL_ACCENT, Rgb, TRACK_COLOURS, accent_for, boundary_pairs,
    decorative_pairs, palette, text_pairs,
};

const TEXT_FLOOR: f64 = 4.5;
const BOUNDARY_FLOOR: f64 = 3.0;
const ACCENT_DELTA_E_FLOOR: f64 = 15.0;
const SAMPLE: &str = "Funk 120 dB";

const ACCENTS: [(&str, Rgb); 18] = [
    ("gain", (0xa9, 0xaf, 0xb9)),
    ("gate", (0x77, 0x7e, 0xad)),
    ("drive", (0xeb, 0x82, 0x28)),
    ("fuzz", (0xf2, 0x37, 0x35)),
    ("preamp", (0xf0, 0xb3, 0x30)),
    ("tonestack", (0xd6, 0xc3, 0x98)),
    ("compressor", (0x2a, 0xa9, 0x9c)),
    ("cabinet", (0xa5, 0x77, 0x57)),
    ("delay", (0x03, 0x7d, 0xff)),
    ("reverb", (0xa3, 0x61, 0xe2)),
    ("chorus", (0x44, 0xb4, 0x4f)),
    ("flanger", (0x24, 0xca, 0xe0)),
    ("autowah", (0xc3, 0xf0, 0x42)),
    ("eq", (0xf1, 0x6b, 0x9b)),
    ("blend", (0x6c, 0xf4, 0xc7)),
    ("limiter", (0x70, 0xb2, 0xf1)),
    ("shaper", (0xef, 0x8a, 0x78)),
    ("octave", (0xf4, 0xac, 0xfe)),
];

fn linear(value: u8) -> f64 {
    let srgb = f64::from(value) / 255.0;
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance((r, g, b): Rgb) -> f64 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[allow(clippy::many_single_char_names, reason = "CIE XYZ and Lab symbols")]
fn lab((r, g, b): Rgb) -> [f64; 3] {
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn hue_deg(b: f64, a: f64) -> f64 {
    if a == 0.0 && b == 0.0 {
        0.0
    } else {
        b.atan2(a).to_degrees().rem_euclid(360.0)
    }
}

#[allow(clippy::many_single_char_names, reason = "CIEDE2000's own symbols")]
fn delta_e00(one: Rgb, two: Rgb) -> f64 {
    let [l1, a1, b1] = lab(one);
    let [l2, a2, b2] = lab(two);
    let chroma_mean = f64::midpoint(a1.hypot(b1), a2.hypot(b2));
    let g = 0.5 * (1.0 - (chroma_mean.powi(7) / (chroma_mean.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let (h1p, h2p) = (hue_deg(b1, a1p), hue_deg(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh_deg = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dh = 2.0 * (c1p * c2p).sqrt() * (dh_deg / 2.0).to_radians().sin();
    let l_mean = f64::midpoint(l1, l2);
    let prime_chroma_mean = f64::midpoint(c1p, c2p);
    let hp_mean = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        f64::midpoint(h1p, h2p)
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hp_mean - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_mean).to_radians().cos()
        + 0.32 * (3.0 * hp_mean + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_mean - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_mean - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (prime_chroma_mean.powi(7) / (prime_chroma_mean.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (l_mean - 50.0).powi(2) / (20.0 + (l_mean - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * prime_chroma_mean;
    let sh = 1.0 + 0.015 * prime_chroma_mean * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dh / sh).powi(2) + rt * (dc / sc) * (dh / sh)).sqrt()
}

fn assert_floor(pairs: &[ContrastPair], floor: f64) {
    for pair in pairs {
        let ratio = contrast(pair.fore, pair.back);
        assert!(ratio >= floor, "{}: {ratio:.2} < {floor}", pair.name);
    }
}

#[test]
fn contrast_helper_matches_wcag_extremes() {
    assert!((contrast((255, 255, 255), (0, 0, 0)) - 21.0).abs() < 1e-9);
}

#[test]
fn every_text_pair_meets_the_text_floor() {
    let pairs = text_pairs();
    assert!(pairs.iter().any(|pair| pair.name.contains("bypass")));
    assert_floor(&pairs, TEXT_FLOOR);
}

#[test]
fn every_boundary_pair_meets_the_boundary_floor() {
    let pairs = boundary_pairs();
    assert!(pairs.iter().any(|pair| pair.name.contains("bypass")));
    assert!(pairs.iter().any(|pair| pair.name.contains("focus")));
    assert_floor(&pairs, BOUNDARY_FLOOR);
}

#[test]
fn accents_are_the_faceplate_set() {
    for (type_name, rgb) in ACCENTS {
        assert_eq!(accent_for(type_name), rgb, "{type_name}");
    }
    assert_eq!(accent_for("theremin"), NEUTRAL_ACCENT);
}

#[test]
fn delta_e00_matches_the_published_accent_table() {
    let compressor_flanger = delta_e00(accent_for("compressor"), accent_for("flanger"));
    assert!(
        (compressor_flanger - 15.62).abs() < 0.05,
        "{compressor_flanger}"
    );
    let chorus_blend = delta_e00(accent_for("chorus"), accent_for("blend"));
    assert!((chorus_blend - 20.8).abs() < 0.05, "{chorus_blend}");
}

#[test]
fn accents_are_pairwise_distinct_by_ciede2000() {
    for (index, (one, rgb_one)) in ACCENTS.iter().enumerate() {
        for (two, rgb_two) in &ACCENTS[index + 1..] {
            let distance = delta_e00(*rgb_one, *rgb_two);
            assert!(
                distance >= ACCENT_DELTA_E_FLOOR,
                "{one}/{two}: ΔE00 {distance:.2}"
            );
        }
    }
}

#[test]
fn on_accent_text_and_value_arcs_meet_their_floors() {
    let colours = palette();
    for (type_name, _) in ACCENTS {
        let accent = accent_for(type_name);
        let text = contrast(colours.on_accent, accent);
        assert!(text >= TEXT_FLOOR, "{type_name} text {text:.2}");
        for (back, name) in [
            (colours.panel_open, "panel open"),
            (colours.panel, "panel"),
            (colours.well, "well"),
        ] {
            let arc = contrast(accent, back);
            assert!(arc >= BOUNDARY_FLOOR, "{type_name} arc on {name} {arc:.2}");
        }
    }
}

#[test]
fn track_colours_mark_the_strip_tab_on_every_surface_it_meets() {
    let colours = palette();
    let declared = boundary_pairs();
    for track in TRACK_COLOURS {
        for back in [colours.panel, colours.panel_open] {
            let ratio = contrast(track, back);
            assert!(ratio >= BOUNDARY_FLOOR, "{track:?} on {back:?} {ratio:.2}");
            assert!(
                declared
                    .iter()
                    .any(|pair| (pair.fore, pair.back) == (track, back)),
                "{track:?} on {back:?} is not a declared boundary pair"
            );
        }
    }
    let edges = [
        ("Bus", theme::bus_colour()),
        ("Master", theme::master_colour()),
    ];
    for (index, track) in TRACK_COLOURS.iter().enumerate() {
        let others = TRACK_COLOURS[index + 1..]
            .iter()
            .map(|other| ("a later Track", *other))
            .chain(edges);
        for (name, other) in others {
            let distance = delta_e00(*track, other);
            assert!(
                distance >= ACCENT_DELTA_E_FLOOR,
                "Track {} {track:?} vs {name} {other:?}: ΔE00 {distance:.2}",
                index + 1
            );
        }
    }
}

fn sample_size(definitions: FontDefinitions, font: &FontId) -> Vec2 {
    let ctx = Context::default();
    ctx.set_fonts(definitions);
    ctx.run_ui(RawInput::default(), |_| {})
        .textures_delta
        .clear();
    ctx.fonts_mut(|fonts| {
        assert!(fonts.has_glyphs(font, SAMPLE), "{font:?}");
        fonts
            .layout_no_wrap(SAMPLE.to_string(), font.clone(), Color32::WHITE)
            .size()
    })
}

fn sample_width(definitions: FontDefinitions, font: &FontId) -> f32 {
    sample_size(definitions, font).x
}

/// The theme's families with every bundled face taken out, leaving only egui's own fonts.
fn fallback_only() -> FontDefinitions {
    let egui_fonts = FontDefinitions::default().font_data;
    let mut definitions = theme::fonts();
    for faces in definitions.families.values_mut() {
        faces.retain(|name| egui_fonts.contains_key(name));
    }
    definitions
}

#[test]
fn every_font_role_lays_out_in_its_own_plex_face() {
    let roles = [
        ("legend", theme::legend()),
        ("body", theme::body()),
        ("value", theme::value()),
        ("unit name", theme::unit_name()),
        ("note", theme::note()),
        ("heading", theme::heading()),
        ("Preset name", theme::preset_name()),
        ("timecode", theme::timecode()),
    ];
    for (role, font) in roles {
        let bundled = sample_size(theme::fonts(), &font);
        let fallback = sample_size(fallback_only(), &font);
        assert!(
            (bundled - fallback).abs().max_elem() > 0.5,
            "{role}: {bundled:?} vs {fallback:?}"
        );
    }
}

#[test]
fn plex_weights_and_widths_are_distinct_faces() {
    let size = theme::TEXT_UNIT;
    let regular = sample_width(theme::fonts(), &FontId::proportional(size));
    let medium = sample_width(theme::fonts(), &FontId::new(size, theme::family_medium()));
    let semibold = sample_width(theme::fonts(), &FontId::new(size, theme::family_semibold()));
    let condensed = sample_width(
        theme::fonts(),
        &FontId::new(size, theme::family_condensed()),
    );
    assert!(condensed < regular, "{condensed} vs {regular}");
    assert!(regular < medium, "{regular} vs {medium}");
    assert!(medium < semibold, "{medium} vs {semibold}");
}

#[test]
fn no_colour_literal_outside_the_theme() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name == "theme.rs") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for (number, line) in text.lines().enumerate() {
            if colour_literal(line) {
                found.push(format!("{}:{}: {line}", path.display(), number + 1));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

fn colour_literal(line: &str) -> bool {
    const NAMED: [&str; 10] = [
        "WHITE",
        "BLACK",
        "GRAY",
        "LIGHT_GRAY",
        "DARK_GRAY",
        "RED",
        "GREEN",
        "BLUE",
        "YELLOW",
        "GOLD",
    ];
    let constructor = [
        "from_rgb",
        "from_rgba",
        "from_gray",
        "from_black_alpha",
        "from_white_alpha",
    ]
    .iter()
    .any(|name| line.contains(name));
    let named = line.split("Color32::").skip(1).any(|rest| {
        let ident: String = rest
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || *c == '_')
            .collect();
        NAMED.contains(&ident.as_str())
    });
    let hex = line.match_indices('#').any(|(at, _)| {
        let digits: Vec<char> = line[at + 1..].chars().take(6).collect();
        digits.len() == 6 && digits.iter().all(char::is_ascii_hexdigit)
    }) || line.contains("(0x");
    constructor || named || hex || decimal_triple(line)
}

fn decimal_triple(line: &str) -> bool {
    line.split('(').skip(1).any(|rest| {
        let Some((inner, _)) = rest.split_once(')') else {
            return false;
        };
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        parts.len() == 3 && parts.iter().all(|part| part.parse::<u8>().is_ok())
    })
}

#[test]
fn colour_literal_catches_every_colour_form() {
    for line in [
        "const TRACK: Rgb = (60, 120, 220);",
        "colour((235, 130, 40))",
        "Color32::from_rgb(1, 2, 3)",
        "let red = (0xf0, 0x64, 0x5a);",
        "// #3c78dc",
        "Color32::WHITE",
    ] {
        assert!(colour_literal(line), "{line}");
    }
    for line in [
        "let size = vec2(4.0, 8.0);",
        "let pair = (1, 2);",
        "let wide = (300, 2, 3);",
        "format!(\"#{index}\")",
        "// #ab— not hex",
    ] {
        assert!(!colour_literal(line), "{line}");
    }
}

fn rgb_of(colour: Color32) -> Rgb {
    (colour.r(), colour.g(), colour.b())
}

#[test]
fn every_control_state_draws_its_boundary() {
    let visuals = xguitare_gui::widgets::visuals();
    let states = [
        ("inactive", visuals.widgets.inactive),
        ("hovered", visuals.widgets.hovered),
        ("active", visuals.widgets.active),
    ];
    for (name, state) in states {
        assert!(state.bg_stroke.width >= 1.0, "{name} stroke width");
        let ratio = contrast(rgb_of(state.bg_stroke.color), rgb_of(state.bg_fill));
        assert!(ratio >= BOUNDARY_FLOOR, "{name} stroke on fill {ratio:.2}");
    }
}

fn all_declared_pairs() -> Vec<(&'static str, ContrastPair)> {
    let lists = [
        ("text", text_pairs()),
        ("boundary", boundary_pairs()),
        ("decorative", decorative_pairs()),
    ];
    lists
        .into_iter()
        .flat_map(|(list, pairs)| pairs.into_iter().map(move |pair| (list, pair)))
        .collect()
}

#[test]
fn every_colour_pair_is_declared_in_exactly_one_list() {
    let pairs = all_declared_pairs();
    for (index, (list, pair)) in pairs.iter().enumerate() {
        for (other_list, other) in &pairs[index + 1..] {
            if list == other_list {
                continue;
            }
            assert!(
                (pair.fore, pair.back) != (other.fore, other.back),
                "{} ({list}) and {} ({other_list}) are the same pair",
                pair.name,
                other.name
            );
        }
    }
}

#[test]
fn decorative_pairs_list_the_faint_lines_that_mark_no_control() {
    let pairs = decorative_pairs();
    for name in [
        "header rule on panel",
        "header and band title rule on panel open",
        "screw ring on ear edge",
        "faceplate edge on panel",
    ] {
        assert!(
            pairs.iter().any(|pair| pair.name == name),
            "{name} is not declared decorative"
        );
    }
}

#[test]
fn a_bypassed_unit_keeps_its_accent_edge_as_decoration() {
    let pairs = decorative_pairs();
    let back = palette().bypass_panel;
    for (type_name, accent) in ACCENTS {
        assert!(
            pairs
                .iter()
                .any(|pair| (pair.fore, pair.back) == (accent, back)),
            "{type_name} edge on bypass panel is not declared decorative"
        );
    }
}

#[test]
fn every_palette_token_is_in_a_declared_pair() {
    let c = palette();
    let tokens = [
        ("case", c.case),
        ("panel", c.panel),
        ("panel_open", c.panel_open),
        ("well", c.well),
        ("ear_edge", c.ear_edge),
        ("ear_centre", c.ear_centre),
        ("text", c.text),
        ("muted", c.muted),
        ("boundary", c.boundary),
        ("lit", c.lit),
        ("bypass_panel", c.bypass_panel),
        ("bypass_text", c.bypass_text),
        ("arc_track", c.arc_track),
        ("bypass_arc", c.bypass_arc),
        ("danger", c.danger),
        ("danger_text", c.danger_text),
        ("go", c.go),
        ("focus_ring", c.focus_ring),
        ("hover_boundary", c.hover_boundary),
        ("knob_highlight", c.knob_highlight),
        ("knob_shadow", c.knob_shadow),
        ("hover", c.hover),
        ("on_accent", c.on_accent),
        ("clip_off", c.clip_off),
        ("ruler", c.ruler),
        ("take_edge", c.take_edge),
        ("grid_bar", c.grid_bar),
        ("grid_beat", c.grid_beat),
        ("recording_take", c.recording_take),
    ];
    let pairs = all_declared_pairs();
    for (name, token) in tokens {
        assert!(
            pairs
                .iter()
                .any(|(_, pair)| pair.fore == token || pair.back == token),
            "{name} is drawn in no declared pair"
        );
    }
}

#[test]
fn the_timeline_and_transport_draw_in_declared_pairs() {
    let c = palette();
    let has = |pairs: &[ContrastPair], fore: Rgb, back: Rgb| {
        pairs
            .iter()
            .any(|pair| (pair.fore, pair.back) == (fore, back))
    };
    let (text, boundary) = (text_pairs(), boundary_pairs());
    assert!(has(&text, c.muted, c.ruler), "bar numbers on the ruler");
    assert!(has(&text, c.text, c.panel_open), "Take name on its body");
    assert!(has(&text, c.text, c.well), "bar.beat in the LCD well");
    assert!(
        has(&text, c.lit, c.well),
        "seconds in the LCD well; loop edges on the lane"
    );
    assert!(has(&text, c.lit, c.panel_open), "loop edges over a Take");
    assert!(
        has(&text, c.text, c.recording_take),
        "the recording Take's name on its body"
    );
    assert!(has(&boundary, c.take_edge, c.well), "Take edge on the lane");
    assert!(
        has(&boundary, c.danger, c.recording_take),
        "recording Take edge"
    );
    assert!(has(&boundary, c.danger, c.ruler), "playhead on the ruler");
    assert!(has(&boundary, c.lit, c.ruler), "loop edges on the ruler");
    assert!(has(&boundary, c.go, c.well), "Track level bar");
    for track in TRACK_COLOURS {
        assert!(has(&boundary, track, c.panel_open), "Take top edge");
    }
}
