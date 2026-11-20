use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Margin, TextStyle, Vec2, vec2};

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
    /// An unlit clip LED (mixer mockup: clip off).
    pub clip_off: Rgb,
    /// The timeline ruler's strip (project mockup: ruler).
    pub ruler: Rgb,
    /// A Take plate's outline on the lane (project mockup: take edge).
    pub take_edge: Rgb,
    /// The timeline's bar line (project mockup: grid bar).
    pub grid_bar: Rgb,
    /// The timeline's beat line (project mockup: grid beat).
    pub grid_beat: Rgb,
    /// The plate of the Take being recorded (project mockup: recording take).
    pub recording_take: Rgb,
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
        clip_off: (0x3a, 0x22, 0x20),
        ruler: (0x1b, 0x1e, 0x22),
        take_edge: (0x6d, 0x76, 0x7f),
        grid_bar: (0x36, 0x3c, 0x43),
        grid_beat: (0x1c, 0x20, 0x24),
        recording_take: (0x3a, 0x22, 0x20),
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

const MODULE_ACCENTS: [(&str, Rgb); 18] = [
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

pub fn accent_for(type_name: &str) -> Rgb {
    MODULE_ACCENTS
        .iter()
        .find(|(name, _)| *name == type_name)
        .map_or(NEUTRAL_ACCENT, |(_, accent)| *accent)
}

/// Track colours in Track order, repeating after the last.
pub const TRACK_COLOURS: [Rgb; 6] = [
    (70, 135, 230),
    (235, 130, 40),
    (70, 180, 80),
    (230, 90, 170),
    (40, 170, 160),
    (232, 72, 64),
];

/// A Bus strip's top edge: the reverb accent, as the mixer mockup's Bus.
pub fn bus_colour() -> Rgb {
    accent_for("reverb")
}

/// The Master strip's top edge: the gain accent, as the mixer mockup's Master.
pub fn master_colour() -> Rgb {
    accent_for("gain")
}

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
        pair(
            "text on panel; also the Metronome's playing beat dot",
            c.text,
            c.panel,
        ),
        pair("text on panel open", c.text, c.panel_open),
        pair(
            "text on well; also the pressed edge, the cents centre mark, the mixer scroll bar handle, the Render progress percentage past the lit part and the snap line on its halo",
            c.text,
            c.well,
        ),
        pair("text on hover; also the selection stroke", c.text, c.hover),
        pair("muted on case", c.muted, c.case),
        pair(
            "muted on panel; also a drag ghost's Take name",
            c.muted,
            c.panel,
        ),
        pair(
            "muted on panel open; also the error toast's detail",
            c.muted,
            c.panel_open,
        ),
        pair("muted on well", c.muted, c.well),
        pair("muted on hover", c.muted, c.hover),
        pair("bypass text on bypass panel", c.bypass_text, c.bypass_panel),
        pair("muted on bypass panel", c.muted, c.bypass_panel),
        pair("on accent on text", c.on_accent, c.text),
        pair(
            "on accent on lit; also the Render progress percentage",
            c.on_accent,
            c.lit,
        ),
        pair("on accent on danger", c.on_accent, c.danger),
        pair("on accent on go", c.on_accent, c.go),
        pair("danger text on case", c.danger_text, c.case),
        pair("danger text on panel", c.danger_text, c.panel),
        pair("danger text on panel open", c.danger_text, c.panel_open),
        pair("danger text on hover", c.danger_text, c.hover),
        pair("go on case", c.go, c.case),
        pair("go on panel", c.go, c.panel),
        pair(
            "lit on panel; also the Metronome's beat 1 dot and ring",
            c.lit,
            c.panel,
        ),
        pair(
            "lit on panel open; also the lit lamp ring, the loop edges over a Take, the disconnected banner's edge and ring and a selected radio's dot",
            c.lit,
            c.panel_open,
        ),
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
            "lit on well; also the edit border, the cents needle, the transport's seconds, the loop edges on a lane, the Render progress fill, the selected Item's ring and corners and the selection box, each on its own halo, so that they hold beside any plate",
            c.lit,
            c.well,
        ),
        pair("bar numbers on the ruler", c.muted, c.ruler),
        pair(
            "the recording Take's name on its body",
            c.text,
            c.recording_take,
        ),
    ]
}

/// Control boundaries, focus rings, lamps and arcs on their backgrounds; each must reach 3:1.
pub fn boundary_pairs() -> Vec<ContrastPair> {
    let c = palette();
    let mut pairs = vec![
        pair("boundary on case", c.boundary, c.case),
        pair(
            "boundary on panel; also a Metronome beat's ring and a drag ghost's edge on its body",
            c.boundary,
            c.panel,
        ),
        pair("boundary on panel open", c.boundary, c.panel_open),
        pair(
            "boundary on well; also the loop edges, loop off, on their halo over the Takes, and a drag ghost's edge on the lane",
            c.boundary,
            c.well,
        ),
        pair("boundary on bypass panel", c.boundary, c.bypass_panel),
        pair("boundary on hover", c.boundary, c.hover),
        pair("danger on case", c.danger, c.case),
        pair("danger on panel", c.danger, c.panel),
        pair("focus ring on case", c.focus_ring, c.case),
        pair("focus ring on panel", c.focus_ring, c.panel),
        pair("focus ring on panel open", c.focus_ring, c.panel_open),
        pair("focus ring on bypass panel", c.focus_ring, c.bypass_panel),
        pair("bypass arc on bypass panel", c.bypass_arc, c.bypass_panel),
        pair(
            "danger on panel open; also the error toast's edge and dot",
            c.danger,
            c.panel_open,
        ),
        pair(
            "hover boundary on hover; also a hovered unit chevron",
            c.hover_boundary,
            c.hover,
        ),
        pair(
            "hover boundary on well; also a pressed unit chevron",
            c.hover_boundary,
            c.well,
        ),
        pair("a hovered selected radio's dot", c.lit, c.hover),
        pair(
            "bypassed pointer on knob highlight",
            c.bypass_text,
            c.knob_highlight,
        ),
        pair("danger on hover", c.danger, c.hover),
        pair(
            "danger on well; also the playhead on its halo over the Takes",
            c.danger,
            c.well,
        ),
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
        pair("Bus edge, pan and Send arc on panel", bus_colour(), c.panel),
        pair("Master edge on panel", master_colour(), c.panel),
        pair("clip LED lit on clip off", c.danger, c.clip_off),
        pair("fader cap on panel", c.ear_edge, c.panel),
        pair("fader cap on well", c.ear_edge, c.well),
        pair(
            "Take edge on the lane; also on the well gap between an Item and one it lies over",
            c.take_edge,
            c.well,
        ),
        pair("recording Take edge", c.danger, c.recording_take),
        pair("loop edges on the recording Take", c.lit, c.recording_take),
        pair(
            "loop edges, loop off, on the recording Take",
            c.boundary,
            c.recording_take,
        ),
        pair("playhead on the ruler", c.danger, c.ruler),
        pair("loop edges on the ruler", c.lit, c.ruler),
        pair("Track level bar on its well", c.go, c.well),
    ];
    for track in TRACK_COLOURS {
        pairs.push(pair("Track edge and pan arc on panel", track, c.panel));
        pairs.push(pair(
            "Track swatch on panel open; also the Take top edge, waveform and zero line on its body",
            track,
            c.panel_open,
        ));
    }
    pairs
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
        pair(
            "unit bottom edge and mixer scroll bar track on panel",
            c.well,
            c.panel,
        ),
        pair("eq grid on well", c.hover, c.well),
        pair(
            "knob highlight on knob shadow",
            c.knob_highlight,
            c.knob_shadow,
        ),
        pair("arc track on panel", c.arc_track, c.panel),
        pair("arc track on panel open", c.arc_track, c.panel_open),
        pair("unlit clip LED on panel", c.clip_off, c.panel),
        pair("timeline bar line on the lane", c.grid_bar, c.well),
        pair("timeline beat line on the lane", c.grid_beat, c.well),
        pair("ruler bar line on the ruler", c.grid_bar, c.ruler),
        pair("transport LCD well on panel", c.well, c.panel),
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

/// A button, an Add Module chip or a Bank row: the smallest pointer target.
pub const CONTROL_HEIGHT: f32 = 24.0;
/// A left-panel section's header, which reaches the 24 px target through the panel padding above it.
pub const SECTION_HEADER_HEIGHT: f32 = 20.0;

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

/// Inside a dialog's edge.
pub const DIALOG_MARGIN: Margin = Margin::same(SPACE_M as i8);
pub const PROGRESS_HEIGHT: f32 = CONTROL_HEIGHT;
/// The width of a drawn icon's lines: the Mute banner's speaker, the disconnected banner's ring.
pub const ICON_STROKE_WIDTH: f32 = 1.5;
/// The Mute banner's struck-through speaker: its height, and the room it takes before the text.
pub const MUTED_ICON_SIZE: f32 = SPACE_L;
pub const MUTED_ICON_WIDTH: f32 = MUTED_ICON_SIZE * 1.5;
/// The disconnected banner: two text lines, its lit edge and the ring before them.
pub const LINK_BANNER_HEIGHT: f32 = 2.0 * CONTROL_HEIGHT + SPACE_S;
/// The banner of a Daemon that exited: three text lines, its danger edge and the filled mark before them.
pub const LINK_BANNER_ERROR_HEIGHT: f32 = 3.0 * CONTROL_HEIGHT + SPACE_S;
pub const LINK_BANNER_EDGE_WIDTH: f32 = 2.0;
pub const LINK_RING_RADIUS: f32 = SPACE_S;
pub const TOAST_WIDTH: f32 = 330.0;
pub const TOAST_EDGE_WIDTH: f32 = SPACE_XS;
pub const TOAST_DOT_RADIUS: f32 = SPACE_XS;
/// Inside the error toast, wider at the left for its red edge.
pub const TOAST_MARGIN: Margin = Margin {
    left: SPACE_L as i8,
    right: SPACE_M as i8,
    top: SPACE_M as i8,
    bottom: SPACE_M as i8,
};
/// How far the toast sits from the window's right edge and above its bottom, clear of the footer.
pub const TOAST_OFFSET: Vec2 = vec2(-SPACE_M, -(SPACE_XL + SPACE_L));

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

/// A left-panel section's title and an Add Module chip's type.
pub fn caption() -> FontId {
    FontId::new(TEXT_BODY, family_condensed())
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

/// A dialog's title and a toast's.
pub fn dialog_title() -> FontId {
    FontId::new(TEXT_UNIT, family_semibold())
}

/// The Mute banner's line.
pub fn banner() -> FontId {
    FontId::new(TEXT_UNIT, family_medium())
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
