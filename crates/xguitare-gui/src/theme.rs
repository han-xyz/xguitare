use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

pub type Rgb = (u8, u8, u8);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub case: Rgb,
    pub panel: Rgb,
    pub panel_open: Rgb,
    pub well: Rgb,
    pub ear_edge: Rgb,
    pub ear_centre: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub boundary: Rgb,
    pub lit: Rgb,
    pub bypass_panel: Rgb,
    pub bypass_text: Rgb,
    pub arc_track: Rgb,
    pub bypass_arc: Rgb,
    pub danger: Rgb,
    pub danger_text: Rgb,
    pub go: Rgb,
    pub focus_ring: Rgb,
    /// The edge of a control under the pointer or pressed, and a knob's printed ticks (states mockup: hover boundary).
    pub hover_boundary: Rgb,
    /// The light side of a knob body's gradient (a2 knob).
    pub knob_highlight: Rgb,
    /// The dark side of a knob body's gradient (a2 knob).
    pub knob_shadow: Rgb,
    pub hover: Rgb,
    pub on_accent: Rgb,
}

pub fn palette() -> Palette {
    Palette {
        case: (0x0f, 0x11, 0x13),
        panel: (0x24, 0x28, 0x2d),
        panel_open: (0x2d, 0x32, 0x38),
        well: (0x0c, 0x0e, 0x10),
        ear_edge: (0x8c, 0x93, 0x9a),
        ear_centre: (0xcb, 0xd1, 0xd6),
        text: (0xee, 0xf1, 0xf4),
        muted: (0xa9, 0xb1, 0xb9),
        boundary: (0x8b, 0x94, 0x9d),
        lit: (0xf0, 0xb4, 0x32),
        bypass_panel: (0x1b, 0x1e, 0x22),
        bypass_text: (0xb3, 0xba, 0xc2),
        arc_track: (0x45, 0x4c, 0x54),
        bypass_arc: (0x9a, 0xa2, 0xaa),
        danger: (0xf0, 0x64, 0x5a),
        danger_text: (0xff, 0x9c, 0x94),
        go: (0x44, 0xb4, 0x4f),
        focus_ring: (0xf4, 0xf6, 0xf8),
        hover_boundary: (0xc6, 0xcd, 0xd4),
        knob_highlight: (0x5a, 0x61, 0x69),
        knob_shadow: (0x1b, 0x1e, 0x22),
        hover: (0x3d, 0x44, 0x4c),
        on_accent: (0x15, 0x17, 0x1a),
    }
}

/// Dims what lies under a modal dialog.
pub fn scrim() -> Color32 {
    Color32::from_rgba_unmultiplied(0x08, 0x09, 0x0a, 158)
}

pub fn colour((r, g, b): Rgb) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn translucent((r, g, b): Rgb, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

pub const NEUTRAL_ACCENT: Rgb = (120, 120, 120);

const MODULE_ACCENTS: [(&str, Rgb); 17] = [
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
];

pub fn accent_for(type_name: &str) -> Rgb {
    MODULE_ACCENTS
        .iter()
        .find(|(name, _)| *name == type_name)
        .map_or(NEUTRAL_ACCENT, |(_, accent)| *accent)
}

/// Track colours in Track order, repeating after the last.
pub const TRACK_COLOURS: [Rgb; 6] = [
    (60, 120, 220),
    (235, 130, 40),
    (70, 180, 80),
    (150, 90, 210),
    (40, 170, 160),
    (210, 50, 45),
];

/// A foreground drawn on a background somewhere in the GUI, so that its contrast can be checked once here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContrastPair {
    pub name: &'static str,
    pub fore: Rgb,
    pub back: Rgb,
}

fn pair(name: &'static str, fore: Rgb, back: Rgb) -> ContrastPair {
    ContrastPair { name, fore, back }
}

/// Text on its backgrounds; each must reach 4.5:1.
pub fn text_pairs() -> Vec<ContrastPair> {
    let c = palette();
    vec![
        pair("text on case", c.text, c.case),
        pair("text on panel", c.text, c.panel),
        pair("text on panel open", c.text, c.panel_open),
        pair(
            "text on well; also the pressed edge and the cents centre mark",
            c.text,
            c.well,
        ),
        pair("text on hover; also the selection stroke", c.text, c.hover),
        pair("text on beat light", c.text, c.arc_track),
        pair("muted on case", c.muted, c.case),
        pair("muted on panel", c.muted, c.panel),
        pair("muted on panel open", c.muted, c.panel_open),
        pair("muted on well", c.muted, c.well),
        pair("muted on hover", c.muted, c.hover),
        pair("bypass text on bypass panel", c.bypass_text, c.bypass_panel),
        pair("muted on bypass panel", c.muted, c.bypass_panel),
        pair("on accent on text", c.on_accent, c.text),
        pair("on accent on lit", c.on_accent, c.lit),
        pair("on accent on danger", c.on_accent, c.danger),
        pair("on accent on go", c.on_accent, c.go),
        pair("danger text on case", c.danger_text, c.case),
        pair("danger text on panel", c.danger_text, c.panel),
        pair("danger text on panel open", c.danger_text, c.panel_open),
        pair("danger text on hover", c.danger_text, c.hover),
        pair("go on case", c.go, c.case),
        pair("go on panel", c.go, c.panel),
        pair("lit on panel", c.lit, c.panel),
        pair(
            "lit on panel open; also the lit lamp ring",
            c.lit,
            c.panel_open,
        ),
        pair("lit on beat light", c.lit, c.arc_track),
        pair(
            "go on panel open; also the Tuner lamp ring",
            c.go,
            c.panel_open,
        ),
        pair("bypass text on well", c.bypass_text, c.well),
        pair("bypass text on hover", c.bypass_text, c.hover),
        pair("danger text on well", c.danger_text, c.well),
        pair("on accent on hover boundary", c.on_accent, c.hover_boundary),
        pair("on accent on muted", c.on_accent, c.muted),
        pair("on accent on bypass arc", c.on_accent, c.bypass_arc),
        pair("rack position on ear edge", c.on_accent, c.ear_edge),
        pair("rack position on ear centre", c.on_accent, c.ear_centre),
        pair(
            "lit on well; also the edit border and the cents needle",
            c.lit,
            c.well,
        ),
    ]
}

/// Control boundaries, focus rings, lamps and arcs on their backgrounds; each must reach 3:1.
pub fn boundary_pairs() -> Vec<ContrastPair> {
    let c = palette();
    vec![
        pair("boundary on case", c.boundary, c.case),
        pair("boundary on panel", c.boundary, c.panel),
        pair("boundary on panel open", c.boundary, c.panel_open),
        pair("boundary on well", c.boundary, c.well),
        pair("boundary on bypass panel", c.boundary, c.bypass_panel),
        pair("boundary on hover", c.boundary, c.hover),
        pair("danger on case", c.danger, c.case),
        pair("danger on panel", c.danger, c.panel),
        pair("focus ring on case", c.focus_ring, c.case),
        pair("focus ring on panel", c.focus_ring, c.panel),
        pair("focus ring on panel open", c.focus_ring, c.panel_open),
        pair("focus ring on bypass panel", c.focus_ring, c.bypass_panel),
        pair("bypass arc on bypass panel", c.bypass_arc, c.bypass_panel),
        pair("danger on panel open", c.danger, c.panel_open),
        pair("hover boundary on hover", c.hover_boundary, c.hover),
        pair("hover boundary on well", c.hover_boundary, c.well),
        pair(
            "bypassed pointer on knob highlight",
            c.bypass_text,
            c.knob_highlight,
        ),
        pair("danger on hover", c.danger, c.hover),
        pair("danger on well", c.danger, c.well),
        pair("tick and chevron on panel", c.hover_boundary, c.panel),
        pair(
            "tick and chevron on panel open",
            c.hover_boundary,
            c.panel_open,
        ),
        pair(
            "tick and chevron on bypass panel",
            c.hover_boundary,
            c.bypass_panel,
        ),
        pair("pointer on knob highlight", c.text, c.knob_highlight),
        pair("pointer on knob shadow", c.text, c.knob_shadow),
        pair("knob rim on knob shadow", c.boundary, c.knob_shadow),
        pair("focus ring on focus gap", c.focus_ring, c.on_accent),
        pair("focus ring on hover", c.focus_ring, c.hover),
        pair("focus ring on well", c.focus_ring, c.well),
    ]
}

/// Pure decoration: divider rules, band-title underlines, screw rings, the faceplate edge of the meter bridge and the Tuner, the knob body's gradient, the unlit LED and arc track, a bypassed unit's accent top edge. These mark no control and no state, so WCAG 1.4.11 sets them no floor; anything that outlines or identifies a control belongs in [`boundary_pairs`].
pub fn decorative_pairs() -> Vec<ContrastPair> {
    let c = palette();
    let mut pairs = vec![
        pair("header rule on panel", c.knob_highlight, c.panel),
        pair(
            "header and band title rule on panel open",
            c.knob_highlight,
            c.panel_open,
        ),
        pair(
            "header rule on bypass panel",
            c.knob_highlight,
            c.bypass_panel,
        ),
        pair("screw ring on ear edge", c.knob_highlight, c.ear_edge),
        pair("screw hole on ear edge", c.well, c.ear_edge),
        pair("engraving highlight on ear edge", c.ear_centre, c.ear_edge),
        pair("faceplate edge on panel", c.hover, c.panel),
        pair("faceplate edge on panel open", c.hover, c.panel_open),
        pair("rack frame on case", c.hover, c.case),
        pair("unit bottom edge on panel", c.well, c.panel),
        pair("eq grid on well", c.hover, c.well),
        pair(
            "knob highlight on knob shadow",
            c.knob_highlight,
            c.knob_shadow,
        ),
        pair("arc track on panel", c.arc_track, c.panel),
        pair("arc track on panel open", c.arc_track, c.panel_open),
    ];
    pairs.extend(MODULE_ACCENTS.iter().map(|(_, accent)| {
        pair(
            "Module accent edge on bypass panel",
            *accent,
            c.bypass_panel,
        )
    }));
    pairs
}

pub const TEXT_LEGEND: f32 = 11.0;
pub const TEXT_BODY: f32 = 12.0;
pub const TEXT_UNIT: f32 = 14.0;
pub const TEXT_LARGE: f32 = 18.0;
pub const TEXT_DISPLAY: f32 = 30.0;

pub const SPACE_XS: f32 = 4.0;
pub const SPACE_S: f32 = 8.0;
pub const SPACE_M: f32 = 12.0;
pub const SPACE_L: f32 = 16.0;
pub const SPACE_XL: f32 = 24.0;

/// The Module-coloured edge along a Rack unit's top.
pub const UNIT_EDGE_WIDTH: f32 = 2.0;

pub const RADIUS_LED: u8 = 1;
pub const RADIUS_CONTROL: u8 = 3;
pub const RADIUS_PANEL: u8 = 6;
/// Half a tag's height: a pill.
pub const RADIUS_PILL: u8 = 9;

pub const FOCUS_RING_GAP: f32 = 2.0;
pub const FOCUS_RING_WIDTH: f32 = 2.0;
pub const EDIT_BORDER_WIDTH: f32 = 2.0;
pub const CARET_WIDTH: f32 = 2.0;

const PLEX: &str = "plex";
const PLEX_MEDIUM: &str = "plex-medium";
const PLEX_SEMIBOLD: &str = "plex-semibold";
const PLEX_CONDENSED: &str = "plex-cond";
const PLEX_MONO: &str = "plex-mono";

const FONT_FILES: [(&str, &[u8]); 5] = [
    (
        PLEX,
        include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    ),
    (
        PLEX_MEDIUM,
        include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
    ),
    (
        PLEX_SEMIBOLD,
        include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    ),
    (
        PLEX_CONDENSED,
        include_bytes!("../assets/fonts/IBMPlexSansCondensed-Regular.ttf"),
    ),
    (
        PLEX_MONO,
        include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    ),
];

pub fn family_medium() -> FontFamily {
    FontFamily::Name(PLEX_MEDIUM.into())
}

pub fn family_semibold() -> FontFamily {
    FontFamily::Name(PLEX_SEMIBOLD.into())
}

pub fn family_condensed() -> FontFamily {
    FontFamily::Name(PLEX_CONDENSED.into())
}

/// The bundled IBM Plex faces first in every family, egui's own fonts after them for glyphs Plex lacks.
pub fn fonts() -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    for (name, bytes) in FONT_FILES {
        definitions
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    let fallback = definitions.families[&FontFamily::Proportional].clone();
    let with_fallback = |name: &str| {
        std::iter::once(name.to_owned())
            .chain(fallback.iter().cloned())
            .collect::<Vec<_>>()
    };
    let families = &mut definitions.families;
    families.insert(FontFamily::Proportional, with_fallback(PLEX));
    families.insert(family_medium(), with_fallback(PLEX_MEDIUM));
    families.insert(family_semibold(), with_fallback(PLEX_SEMIBOLD));
    families.insert(family_condensed(), with_fallback(PLEX_CONDENSED));
    if let Some(mono) = families.get_mut(&FontFamily::Monospace) {
        mono.insert(0, PLEX_MONO.to_owned());
    }
    definitions
}

pub fn legend() -> FontId {
    FontId::new(TEXT_LEGEND, family_condensed())
}

pub fn body() -> FontId {
    FontId::proportional(TEXT_BODY)
}

pub fn value() -> FontId {
    FontId::monospace(TEXT_BODY)
}

pub fn list_item() -> FontId {
    FontId::proportional(TEXT_UNIT)
}

pub fn unit_name() -> FontId {
    FontId::new(TEXT_UNIT, family_semibold())
}

/// The Tuner note and the Metronome bpm.
pub fn note() -> FontId {
    FontId::monospace(TEXT_LARGE)
}

/// A Rack position engraved on a unit's ears.
pub fn ear_number() -> FontId {
    FontId::monospace(TEXT_UNIT)
}

/// A left-panel section's title.
pub fn section_title() -> FontId {
    FontId::new(TEXT_BODY, family_semibold())
}

/// An eq band column's title.
pub fn band_title() -> FontId {
    FontId::new(TEXT_BODY, family_semibold())
}

/// The note in the open Tuner view.
pub fn tuner_note() -> FontId {
    FontId::monospace(TEXT_DISPLAY)
}

/// The cents beside the Tuner note.
pub fn cents() -> FontId {
    FontId::monospace(TEXT_UNIT)
}

/// The dB marks under the meter bridge's ladders.
pub fn scale() -> FontId {
    FontId::monospace(TEXT_LEGEND)
}

pub fn heading() -> FontId {
    FontId::new(TEXT_LARGE, family_semibold())
}

pub fn preset_name() -> FontId {
    FontId::new(TEXT_DISPLAY, family_medium())
}

pub fn timecode() -> FontId {
    FontId::monospace(TEXT_DISPLAY)
}

/// egui's text styles on the same type scale, for text that names no role.
pub fn text_styles() -> std::collections::BTreeMap<TextStyle, FontId> {
    [
        (TextStyle::Small, FontId::proportional(TEXT_LEGEND)),
        (TextStyle::Body, body()),
        (TextStyle::Button, body()),
        (TextStyle::Monospace, value()),
        (TextStyle::Heading, heading()),
    ]
    .into()
}
