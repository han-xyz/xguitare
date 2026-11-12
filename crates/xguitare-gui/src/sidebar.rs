use std::time::Instant;

use egui::collapsing_header::CollapsingState;
use egui::style::ScrollStyle;
use egui::{
    Align, Align2, Button, Color32, ComboBox, CornerRadius, CursorIcon, DragAndDrop, DragValue,
    Frame, Grid, Id, Key, LayerId, Layout, Margin, Order, Painter, Pos2, Rect, Response, RichText,
    Role, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui, Vec2, WidgetInfo, pos2, vec2,
};
use xguitare_core::BankKind;

use crate::metronome_model::{
    BPM_RANGE, BeatLight, DEFAULT_VOLUME_DB, METER_CHOICES, VOLUME_DB_RANGE, metronome_view,
};
use crate::metronome_model::{BeatDot, beat_dots, bpm_text};
use crate::model::{
    AddPlace, BankRow, CHIP_GAP, ChoiceKey, Command, Dragged, Feedback, FixedUnit, HiddenFixed,
    ModuleType, PortSelection, RackLayout, RemovedUnit, SECTIONS, Section, SettingsDraft, Snapshot,
    add_command, add_place, add_places, chip_columns, engine_readout, filter_bank, param_rows,
    port_choices, rack_channels, set_ports_command, set_settings_command, settings_draft,
    shown_undo, tuner_toggle,
};
use crate::project::{knob_with_reset, released_value};
use crate::project_model::{project_name, project_rows, transport_view, undo_text};
use crate::rack::choice_keys;
use crate::rack::muted_text;
use crate::theme::{
    self, CONTROL_HEIGHT, RADIUS_CONTROL, RADIUS_PANEL, SECTION_HEADER_HEIGHT, SPACE_S, SPACE_XS,
    TOAST_DOT_RADIUS, TOAST_EDGE_WIDTH, TOAST_MARGIN, TOAST_OFFSET, TOAST_WIDTH, colour, palette,
};
use crate::widgets::{
    accent, bank_row, choice_button, outlined_button, primary_button, switched_button, text_field,
    toggle_switch,
};

/// After the footer's undo label: the key that undoes.
const UNDO_KEY: &str = "Ctrl+Z";
const NO_PRESET: &str = "no preset";
const CABINET: &str = "cabinet";
/// The Daemon's `rack_add` error for a cabinet on a Bus or Master Rack.
const CABINET_REFUSED: &str = "a cabinet is not allowed on a Bus or Master";
const SIDEBAR_SPACING: Vec2 = vec2(8.0, 4.0);
const SECTION_GAP: f32 = 8.0;
const SECTION_PADDING: Margin = Margin::same(SPACE_S as i8);
const ARROW_SIZE: f32 = 8.0;
const UNSAVED: &str = "unsaved changes";
const SAVE_AS_LABEL: &str = "Save as new";
const SAVE_AS_HINT: &str = "New Preset name";
const SEARCH_HINT: &str = "Search by name or number";
const CHOOSE: &str = "choose";
const RACK_RULES: &str = "by the Rack rules";
const TILE_TAB_WIDTH: f32 = 3.0;
const NO_PORTS: &str = "no port list from the Daemon";
const NO_SETTINGS: &str = "no settings from the Daemon";
const NO_PROJECT: &str = "No Project open";
const NEW_PROJECT_HINT: &str = "New Project";
const OPEN_PROJECT: &str = "Open a Project";
const NO_PROJECTS: &str = "no Projects yet";
const NO_METRONOME: &str = "no Metronome from the Daemon";
const FOLLOWS_PROJECT: &str = "follows the Project";
const BEAT_DOT_SIZE: f32 = 10.0;
const DISMISS: &str = "Dismiss";
const BEAT_DOT_RING_WIDTH: f32 = 1.5;
/// How far a new beat's halo sits outside its dot.
const BEAT_FLASH_OFFSET: f32 = 2.0;
const METER_WIDTH: f32 = 40.0;
const BANK_KIND_WIDTH: f32 = 56.0;
const BANK_KINDS: [(BankKind, &str); 2] = [(BankKind::Guitar, "Guitar"), (BankKind::Bass, "Bass")];

pub struct Tuner<'a> {
    pub open: &'a mut bool,
    pub muted_by_us: &'a mut bool,
    pub mute_default: bool,
}

impl Tuner<'_> {
    pub fn toggle(&mut self, muted_now: bool) -> Option<Command> {
        let (open, command, muted_by_us) =
            tuner_toggle(*self.open, self.mute_default, *self.muted_by_us, muted_now);
        *self.open = open;
        *self.muted_by_us = muted_by_us;
        command
    }
}

/// The Place menu's choice; `None` follows the Rack rules for the type of the tile clicked.
#[derive(Debug, Default)]
pub struct AddDraft {
    pub place: Option<AddPlace>,
}

/// The left panel's own state: a draft `None` shows what the Daemon reports.
pub struct SidebarView<'a, 'b> {
    pub rows: &'a [BankRow],
    pub bank_kind: BankKind,
    pub query: &'a mut String,
    pub save_as_name: &'a mut Option<String>,
    pub add: &'a mut AddDraft,
    /// Whether the Rack shown is a Bus or Master Rack, which takes no cabinet.
    pub stereo_rack: bool,
    pub tuner: &'a mut Tuner<'b>,
    pub ports: &'a mut Option<PortSelection>,
    pub settings: &'a mut Option<SettingsDraft>,
    pub new_project: &'a mut String,
    pub beat_light: BeatLight,
    /// Where the Tuner and the meters sit in the Rack; their chips show a hidden one.
    pub layout: &'a mut RackLayout,
}

/// One collapsible section per [`SECTIONS`] entry, top to bottom.
pub fn sidebar(ui: &mut Ui, snapshot: &Snapshot, view: &mut SidebarView) -> Vec<Command> {
    let mut commands = Vec::new();
    ui.spacing_mut().item_spacing = SIDEBAR_SPACING;
    ui.visuals_mut().indent_has_left_vline = false;
    ui.spacing_mut().scroll = ScrollStyle::solid();
    ui.visuals_mut().widgets.inactive.bg_stroke.width = 1.0;
    ScrollArea::vertical().show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = SECTION_GAP;
        for section in SECTIONS {
            let title = section.title(view.bank_kind);
            section_box(ui, section, title, |ui| {
                commands.extend(section_body(ui, section, snapshot, view));
            });
        }
    });
    commands
}

/// A triangle `ARROW_SIZE` across, centred on `centre`: pointing down when `open`, right when closed.
fn arrow_points(centre: Pos2, open: bool) -> Vec<Pos2> {
    let half = ARROW_SIZE / 2.0;
    let points = if open {
        [(-half, -half / 2.0), (half, -half / 2.0), (0.0, half / 2.0)]
    } else {
        [(-half / 2.0, -half), (half / 2.0, 0.0), (-half / 2.0, half)]
    };
    points
        .into_iter()
        .map(|(x, y)| centre + vec2(x, y))
        .collect()
}

/// A panel holding its title and the collapse arrow, which a click opens or closes, over the body.
fn section_box(ui: &mut Ui, section: Section, title: &str, body: impl FnOnce(&mut Ui)) {
    let colours = palette();
    let id = ui.make_persistent_id(("section", section as u8));
    let mut state =
        CollapsingState::load_with_default_open(ui.ctx(), id, section.open_by_default());
    Frame::new()
        .fill(colour(colours.panel))
        .stroke(Stroke::new(1.0, colour(colours.hover)))
        .corner_radius(RADIUS_PANEL)
        .inner_margin(SECTION_PADDING)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = SIDEBAR_SPACING;
            ui.set_width(ui.available_width());
            let size = vec2(ui.available_width(), SECTION_HEADER_HEIGHT);
            let (rect, header) = ui.allocate_exact_size(size, Sense::click());
            header.widget_info(|| WidgetInfo::labeled(Role::Button, true, title));
            if header.clicked() {
                state.toggle(ui);
            }
            let open = state.is_open();
            let painter = ui.painter();
            painter.text(
                rect.left_center(),
                Align2::LEFT_CENTER,
                title,
                theme::caption(),
                colour(colours.text),
            );
            let arrow_centre = rect.right_center() - vec2(ARROW_SIZE / 2.0, 0.0);
            painter.add(Shape::convex_polygon(
                arrow_points(arrow_centre, open),
                colour(colours.muted),
                Stroke::NONE,
            ));
            state.show_body_unindented(ui, body);
        });
}

fn section_body(
    ui: &mut Ui,
    section: Section,
    snapshot: &Snapshot,
    view: &mut SidebarView,
) -> Vec<Command> {
    match section {
        Section::Project => project_section(ui, snapshot, view.new_project),
        Section::Preset => {
            preset_heading(ui, snapshot);
            save_buttons(ui, view.save_as_name).into_iter().collect()
        }
        Section::Bank => {
            ui.add(text_field(
                TextEdit::singleline(view.query).hint_text(SEARCH_HINT),
            ));
            bank_panel(ui, &filter_bank(view.rows, view.query))
                .into_iter()
                .collect()
        }
        Section::Rack => {
            fixed_chips(ui, snapshot, view.layout);
            add_module(ui, snapshot, view.add, view.stereo_rack)
                .into_iter()
                .collect()
        }
        Section::Tuner => tuner_and_mute(ui, snapshot, view.tuner),
        Section::Metronome => metronome_section(ui, snapshot, view.beat_light),
        Section::Audio => audio_ports(ui, snapshot, view.ports),
        Section::Settings => settings(ui, snapshot, view.settings).into_iter().collect(),
        Section::Engine => {
            engine_readout_grid(ui, snapshot);
            Vec::new()
        }
    }
}

/// While a Project is open: its name, the Take audio it holds in memory, `Save` and `Close`; else a name field with `New`, and `Open` listing the Daemon's Projects, read again when the menu opens.
fn project_section(ui: &mut Ui, snapshot: &Snapshot, new_name: &mut String) -> Vec<Command> {
    let button_size = vec2(
        (ui.available_width() - ui.spacing().item_spacing.x) / 2.0,
        CONTROL_HEIGHT,
    );
    let mut commands = Vec::new();
    if let Some(name) = project_name(snapshot.status.as_ref()) {
        ui.label(RichText::new(name).font(theme::heading()));
        let transport = snapshot.status.as_ref().and_then(transport_view);
        if let Some(transport) = transport {
            ui.label(muted_text(transport.loaded_audio_text()));
        }
        ui.horizontal(|ui| {
            if ui
                .add(primary_button("Save").min_size(button_size))
                .clicked()
            {
                commands.push(Command::ProjectSave);
            }
            if ui
                .add(outlined_button("Close").min_size(button_size))
                .clicked()
            {
                commands.push(Command::ProjectClose);
            }
        });
        return commands;
    }
    ui.label(muted_text(NO_PROJECT));
    let (field, new) = ui
        .horizontal(|ui| {
            let label = ui.label(muted_text("Name"));
            let width = ui.available_width() - button_size.x / 2.0 - ui.spacing().item_spacing.x;
            let field = TextEdit::singleline(new_name)
                .hint_text(NEW_PROJECT_HINT)
                .desired_width(width);
            let field = ui.add(text_field(field)).labelled_by(label.id);
            (field, ui.add(outlined_button("New")).clicked())
        })
        .inner;
    let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    if (entered || new) && !new_name.trim().is_empty() {
        commands.push(Command::ProjectNew(new_name.trim().to_string()));
        new_name.clear();
    }
    let rows = snapshot
        .projects
        .as_ref()
        .map(project_rows)
        .unwrap_or_default();
    let combo = ComboBox::from_id_salt("open_project")
        .width(ui.available_width())
        .selected_text(OPEN_PROJECT)
        .show_ui(ui, |ui| {
            if rows.is_empty() {
                ui.label(muted_text(NO_PROJECTS));
            }
            for row in &rows {
                let item =
                    ui.add_enabled(row.error.is_none(), Button::selectable(false, &row.name));
                if let Some(error) = &row.error {
                    item.on_disabled_hover_text(error);
                } else if item.clicked() {
                    commands.push(Command::ProjectOpen(row.folder.clone()));
                }
            }
        });
    combo
        .response
        .widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, OPEN_PROJECT));
    if combo.response.clicked() {
        commands.push(Command::RefreshProjects);
    }
    commands
}

fn preset_heading(ui: &mut Ui, snapshot: &Snapshot) {
    let status = snapshot.status.as_ref();
    let preset = status.and_then(|status| status["preset"].as_str());
    ui.label(RichText::new(preset.unwrap_or(NO_PRESET)).font(theme::preset_name()));
    let program = status
        .and_then(|status| status["program"].as_u64())
        .map(|program| format!("Program {program}"));
    let modified = status.is_some_and(|status| status["modified"].as_bool() == Some(true));
    let line: Vec<String> = program
        .into_iter()
        .chain(modified.then(|| UNSAVED.to_string()))
        .collect();
    if !line.is_empty() {
        ui.label(muted_text(line.join(", ")));
    }
}

/// `Save as new` opens a name field; `Save` beside it sends the name, so that the Daemon's error for an empty or taken name reaches the footer.
fn save_buttons(ui: &mut Ui, save_as_name: &mut Option<String>) -> Option<Command> {
    let button_size = vec2(
        (ui.available_width() - ui.spacing().item_spacing.x) / 2.0,
        CONTROL_HEIGHT,
    );
    let mut command = None;
    let mut opened = false;
    ui.horizontal(|ui| {
        if ui
            .add(primary_button("Save").min_size(button_size))
            .clicked()
        {
            command = Some(Command::Save);
        }
        if ui
            .add(outlined_button(SAVE_AS_LABEL).min_size(button_size))
            .clicked()
        {
            *save_as_name = save_as_name.is_none().then(String::new);
            opened = save_as_name.is_some();
        }
    });
    let Some(name) = save_as_name else {
        return command;
    };
    let field = ui.add(text_field(
        TextEdit::singleline(name).hint_text(SAVE_AS_HINT),
    ));
    if opened {
        field.request_focus();
    }
    let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    let (confirm, cancel) = ui
        .horizontal(|ui| {
            (
                ui.add(primary_button("Save")).clicked(),
                ui.add(outlined_button("Cancel")).clicked(),
            )
        })
        .inner;
    if entered || confirm {
        command = Some(Command::SaveAs(name.trim().to_string()));
        *save_as_name = None;
    } else if cancel {
        *save_as_name = None;
    }
    command
}

/// One tile per Module type: a click adds the type at the Place menu's place, a drag carries it into the Rack.
/// On a Bus or Master Rack (`stereo_rack`) the cabinet tile is greyed out, with the Daemon's refusal on hover.
fn add_module(
    ui: &mut Ui,
    snapshot: &Snapshot,
    add: &mut AddDraft,
    stereo_rack: bool,
) -> Option<Command> {
    let modules = snapshot.params.as_ref().map(param_rows).unwrap_or_default();
    let rack = rack_channels(&modules, &snapshot.module_types);
    let width = ui.available_width();
    let columns = chip_columns(width);
    let tile_size = vec2(
        (width - CHIP_GAP * (columns - 1) as f32) / columns as f32,
        CONTROL_HEIGHT,
    );
    let mut clicked = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(CHIP_GAP);
        for row in snapshot.module_types.chunks(columns) {
            ui.horizontal(|ui| {
                for module_type in row {
                    if stereo_rack && module_type.type_name == CABINET {
                        ui.add_enabled_ui(false, |ui| {
                            module_tile(ui, module_type, tile_size);
                        })
                        .response
                        .on_disabled_hover_text(CABINET_REFUSED);
                    } else if module_tile(ui, module_type, tile_size).clicked() {
                        clicked = Some(module_type);
                    }
                }
            });
        }
    });
    let offered = add_places(&rack);
    let chosen = add.place.clone().filter(|place| offered.contains(place));
    ui.horizontal(|ui| {
        let place_label = ui.label(muted_text("Place"));
        ComboBox::from_id_salt("add_place")
            .width(ui.available_width())
            .selected_text(
                chosen
                    .as_ref()
                    .map_or(RACK_RULES.to_string(), AddPlace::label),
            )
            .show_ui(ui, |ui| {
                if ui.selectable_label(chosen.is_none(), RACK_RULES).clicked() {
                    add.place = None;
                }
                for option in &offered {
                    if ui
                        .selectable_label(chosen.as_ref() == Some(option), option.label())
                        .clicked()
                    {
                        add.place = Some(option.clone());
                    }
                }
            })
            .response
            .labelled_by(place_label.id);
    });
    let module_type = clicked?;
    let place = add_place(&rack, &module_type.channels, add.place.as_ref());
    *add = AddDraft::default();
    Some(add_command(&module_type.type_name, &place))
}

/// Named in the accessibility tree by the type and its channels; while dragged it follows the pointer and carries [`Dragged::Tile`].
fn module_tile(ui: &mut Ui, module_type: &ModuleType, size: Vec2) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, module_type.label()));
    let type_name = &module_type.type_name;
    paint_tile(ui.painter(), rect, type_name, response.hovered());
    if response.dragged() {
        DragAndDrop::set_payload(ui.ctx(), Dragged::Tile(type_name.clone()));
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            let painter = ui
                .ctx()
                .layer_painter(LayerId::new(Order::Tooltip, response.id));
            paint_tile(
                &painter,
                Rect::from_center_size(pointer, size),
                type_name,
                true,
            );
        }
    }
    response.on_hover_cursor(CursorIcon::Grab)
}

/// A dark chip in a boundary edge, lighter-edged under the pointer, with the Module accent as a tab at its left.
fn paint_tile(painter: &Painter, rect: Rect, type_name: &str, hovered: bool) {
    let colours = palette();
    let edge = if hovered {
        colours.hover_boundary
    } else {
        colours.boundary
    };
    painter.rect(
        rect,
        RADIUS_CONTROL,
        colour(colours.well),
        Stroke::new(1.0, colour(edge)),
        StrokeKind::Inside,
    );
    let inside = rect.shrink(1.0);
    let tab = Rect::from_min_size(inside.min, vec2(TILE_TAB_WIDTH, inside.height()));
    let tab_corners = CornerRadius {
        nw: RADIUS_CONTROL,
        sw: RADIUS_CONTROL,
        ne: 0,
        se: 0,
    };
    painter.rect_filled(tab, tab_corners, accent(type_name));
    painter.text(
        rect.left_center() + vec2(SPACE_S, 0.0),
        Align2::LEFT_CENTER,
        type_name,
        theme::caption(),
        colour(colours.text),
    );
}

/// One chip per fixed unit, as wide as a Module tile: a hidden unit's chip shows it at the top of the Rack on a click, or at the gap it is dragged into; a shown unit's chip is readable but takes no input.
fn fixed_chips(ui: &mut Ui, snapshot: &Snapshot, layout: &mut RackLayout) {
    let width = ui.available_width();
    let columns = chip_columns(width);
    let size = vec2(
        (width - CHIP_GAP * (columns - 1) as f32) / columns as f32,
        CONTROL_HEIGHT,
    );
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(CHIP_GAP);
        for unit in FixedUnit::ALL {
            if fixed_chip(ui, unit, layout.is_shown(unit), size).clicked() {
                clicked = Some(unit);
            }
        }
    });
    if let Some(unit) = clicked {
        let ids: Vec<String> = snapshot
            .params
            .as_ref()
            .map(param_rows)
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        layout.place(unit, 0, &ids);
    }
}

/// Named `Show Tuner` or `Show Meters`; while dragged it follows the pointer and carries [`Dragged::FixedChip`].
fn fixed_chip(ui: &mut Ui, unit: FixedUnit, shown: bool, size: Vec2) -> Response {
    let sense = if shown {
        Sense::hover()
    } else {
        Sense::click_and_drag()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let name = format!("Show {}", unit.label());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, !shown, &name));
    paint_fixed_chip(ui.painter(), rect, unit, shown, response.hovered());
    if shown {
        return response;
    }
    if response.dragged() {
        DragAndDrop::set_payload(ui.ctx(), Dragged::FixedChip(unit));
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            let painter = ui
                .ctx()
                .layer_painter(LayerId::new(Order::Tooltip, response.id));
            paint_fixed_chip(
                &painter,
                Rect::from_center_size(pointer, size),
                unit,
                false,
                true,
            );
        }
    }
    response.on_hover_cursor(CursorIcon::Grab)
}

/// A hidden unit's chip as a Module tile without its accent tab; a shown unit's chip unedged, its name muted.
fn paint_fixed_chip(painter: &Painter, rect: Rect, unit: FixedUnit, shown: bool, hovered: bool) {
    let colours = palette();
    let (edge, text) = match (shown, hovered) {
        (true, _) => (Stroke::NONE, colours.muted),
        (false, true) => (
            Stroke::new(1.0, colour(colours.hover_boundary)),
            colours.text,
        ),
        (false, false) => (Stroke::new(1.0, colour(colours.boundary)), colours.text),
    };
    painter.rect(
        rect,
        RADIUS_CONTROL,
        colour(colours.well),
        edge,
        StrokeKind::Inside,
    );
    painter.text(
        rect.left_center() + vec2(SPACE_S, 0.0),
        Align2::LEFT_CENTER,
        unit.label(),
        theme::caption(),
        colour(text),
    );
}

/// Whether the Daemon reports Mute on; `None` while no status arrives.
pub fn muted_now(snapshot: &Snapshot) -> Option<bool> {
    snapshot
        .status
        .as_ref()
        .and_then(|status| status["muted"].as_bool())
}

/// Turning Mute off by hand means closing the Tuner view leaves it off.
fn tuner_and_mute(ui: &mut Ui, snapshot: &Snapshot, tuner: &mut Tuner) -> Vec<Command> {
    let muted = muted_now(snapshot);
    let button_size = vec2(
        (ui.available_width() - ui.spacing().item_spacing.x) / 2.0,
        CONTROL_HEIGHT,
    );
    let mut commands = Vec::new();
    ui.horizontal(|ui| {
        let tuner_button =
            switched_button("Tuner", *tuner.open, palette().go).min_size(button_size);
        if ui.add(tuner_button).clicked() {
            commands.extend(tuner.toggle(muted.unwrap_or_default()));
        }
        let on = muted.unwrap_or_default();
        let mute_button = switched_button("Mute", on, palette().danger).min_size(button_size);
        if ui.add_enabled(muted.is_some(), mute_button).clicked() {
            if on {
                *tuner.muted_by_us = false;
            }
            commands.push(Command::Mute(!on));
        }
    });
    commands
}

/// The switch, tempo and meter, then the beat dots and Tap, then the volume; with a Project open these are the Project's tempo, meter and click.
fn metronome_section(ui: &mut Ui, snapshot: &Snapshot, light: BeatLight) -> Vec<Command> {
    let status = snapshot.status.as_ref();
    let Some(view) = status.and_then(metronome_view) else {
        ui.label(muted_text(NO_METRONOME));
        return Vec::new();
    };
    let change = |on, bpm, meter, volume| Command::Metronome {
        on,
        bpm,
        meter,
        volume,
    };
    let mut commands = Vec::new();
    let beats = view.meter.0;
    let dots_on_top = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SPACE_XS;
            if toggle_switch(ui, "Metronome", view.on).clicked() {
                commands.push(change(Some(!view.on), None, None, None));
            }
            if let Some(bpm) = tempo_field(ui, view.bpm) {
                commands.push(change(None, Some(bpm), None, None));
            }
            ui.label(muted_text("bpm"));
            if let Some(meter) = meter_picker(ui, view.meter) {
                commands.push(change(None, None, Some(meter), None));
            }
            let fits = beat_dots_width(beats) <= ui.available_width();
            if fits {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    paint_beat_dots(ui, beats, light);
                });
            }
            fits
        })
        .inner;
    ui.horizontal(|ui| {
        if !dots_on_top {
            paint_beat_dots(ui, beats, light);
        }
        ui.label(muted_text("Volume"));
        if let Some(volume) = knob_with_reset(
            ui,
            ("Metronome", "volume"),
            view.volume_db,
            VOLUME_DB_RANGE,
            " dB",
            DEFAULT_VOLUME_DB,
            colour(palette().text),
        ) {
            commands.push(change(None, None, None, Some(volume)));
        }
        if ui.add(outlined_button("Tap")).clicked() {
            commands.push(Command::MetronomeTap);
        }
    });
    if project_name(status).is_some() {
        ui.label(muted_text(FOLLOWS_PROJECT));
    }
    commands
}

/// The tempo in the note role, unframed until pointed at or edited; whole tempos show no decimal.
fn tempo_field(ui: &mut Ui, bpm: f64) -> Option<f64> {
    ui.scope(|ui| {
        ui.style_mut().override_font_id = Some(theme::note());
        let inactive = &mut ui.visuals_mut().widgets.inactive;
        inactive.weak_bg_fill = Color32::TRANSPARENT;
        inactive.bg_stroke = Stroke::NONE;
        let (bpm, field) = released_value(ui, Id::unique("metronome_tempo"), bpm, |bpm| {
            DragValue::new(bpm)
                .range(BPM_RANGE.0..=BPM_RANGE.1)
                .speed(0.5)
                .custom_formatter(|bpm, _| bpm_text(bpm))
        });
        field.widget_info(|| WidgetInfo::labeled(Role::SpinButton, true, "Metronome tempo"));
        bpm
    })
    .inner
}

fn meter_picker(ui: &mut Ui, meter: (u32, u32)) -> Option<(u32, u32)> {
    let shown = |(beats, note): (u32, u32)| format!("{beats}/{note}");
    let mut chosen = None;
    let combo = ComboBox::from_id_salt("metronome_meter")
        .width(METER_WIDTH)
        .selected_text(shown(meter))
        .show_ui(ui, |ui| {
            for choice in METER_CHOICES {
                if ui
                    .add(Button::selectable(choice == meter, shown(choice)))
                    .clicked()
                {
                    chosen = Some(choice);
                }
            }
        });
    combo
        .response
        .widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, "Metronome meter"));
    chosen
}

fn beat_dots_width(beats: u32) -> f32 {
    beats as f32 * (BEAT_DOT_SIZE + 2.0 * BEAT_FLASH_OFFSET)
}

/// [`beat_dots`] drawn in a row: a filled dot in the lit colour for beat 1, else the text colour, with a halo while the beat is new; a ring in the lit colour for beat 1, else the boundary colour.
fn paint_beat_dots(ui: &mut Ui, beats: u32, light: BeatLight) {
    let cell = BEAT_DOT_SIZE + 2.0 * BEAT_FLASH_OFFSET;
    let size = vec2(beat_dots_width(beats), cell);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let label = match light {
        BeatLight::Dark => "Beat off".to_string(),
        BeatLight::Beat { beat, .. } => format!("Beat {beat}"),
    };
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, &label));
    let colours = palette();
    let painter = ui.painter();
    let radius = (BEAT_DOT_SIZE - BEAT_DOT_RING_WIDTH) / 2.0;
    let ring_colour = |accent: bool| {
        if accent {
            colours.lit
        } else {
            colours.boundary
        }
    };
    for (index, dot) in beat_dots(beats, light).into_iter().enumerate() {
        let centre = pos2(
            rect.left() + index as f32 * cell + cell / 2.0,
            rect.center().y,
        );
        match dot {
            BeatDot::Ring { accent } => {
                painter.circle_stroke(
                    centre,
                    radius,
                    Stroke::new(BEAT_DOT_RING_WIDTH, colour(ring_colour(accent))),
                );
            }
            BeatDot::Filled { accent, flash } => {
                let fill = colour(if accent { colours.lit } else { colours.text });
                painter.circle_filled(centre, BEAT_DOT_SIZE / 2.0, fill);
                if flash {
                    painter.circle_stroke(
                        centre,
                        BEAT_DOT_SIZE / 2.0 + BEAT_FLASH_OFFSET,
                        Stroke::new(1.0, fill),
                    );
                }
            }
        }
    }
}

/// Opening a picker reads the port list again, so that a device plugged in since shows.
fn audio_ports(
    ui: &mut Ui,
    snapshot: &Snapshot,
    draft: &mut Option<PortSelection>,
) -> Vec<Command> {
    let Some(choices) = snapshot.ports.as_ref().map(port_choices) else {
        ui.label(muted_text(NO_PORTS));
        return Vec::new();
    };
    let mut selection = draft.clone().unwrap_or(choices.connected.clone());
    let mut commands = Vec::new();
    let pickers = [
        ("Input", &choices.capture, &mut selection.input),
        ("Output L", &choices.playback, &mut selection.output_left),
        ("Output R", &choices.playback, &mut selection.output_right),
    ];
    for (name, ports, chosen) in pickers {
        let label = ui.label(muted_text(name));
        let combo = ComboBox::from_id_salt(("port", name))
            .width(ui.available_width())
            .truncate()
            .selected_text(chosen.as_deref().unwrap_or(CHOOSE))
            .show_ui(ui, |ui| {
                for port in ports {
                    let selected = chosen.as_ref() == Some(port);
                    if ui.selectable_label(selected, port).clicked() {
                        *chosen = Some(port.clone());
                    }
                }
            });
        if combo.response.labelled_by(label.id).clicked() {
            commands.push(Command::RefreshPorts);
        }
    }
    let apply = set_ports_command(&selection);
    let clicked = ui
        .add_enabled(apply.is_some(), outlined_button("Apply"))
        .clicked();
    if let Some(command) = apply.filter(|_| clicked) {
        commands.push(command);
        *draft = None;
        return commands;
    }
    *draft = (selection != choices.connected).then_some(selection);
    commands
}

fn settings(
    ui: &mut Ui,
    snapshot: &Snapshot,
    draft: &mut Option<SettingsDraft>,
) -> Option<Command> {
    let Some(current) = snapshot.settings.as_ref().and_then(settings_draft) else {
        ui.label(muted_text(NO_SETTINGS));
        return None;
    };
    let mut edited = draft.clone().unwrap_or(current.clone());
    ui.horizontal(|ui| {
        ui.label(muted_text("Bank kind"));
        edited.bank_kind = bank_kind_choice(ui, edited.bank_kind);
    });
    ui.horizontal(|ui| {
        let label = ui.label(muted_text("IR folder"));
        let field = TextEdit::singleline(&mut edited.ir_dir).desired_width(ui.available_width());
        ui.add(text_field(field)).labelled_by(label.id);
    });
    ui.horizontal(|ui| {
        let label = ui.label(muted_text("Projects folder"));
        let field =
            TextEdit::singleline(&mut edited.projects_dir).desired_width(ui.available_width());
        ui.add(text_field(field)).labelled_by(label.id);
    });
    let apply = set_settings_command(&current, &edited);
    let clicked = ui
        .add_enabled(apply.is_some(), outlined_button("Apply"))
        .clicked();
    if clicked {
        *draft = None;
        return apply;
    }
    *draft = (edited != current).then_some(edited);
    None
}

/// Guitar and Bass as one choice group: one Tab stop, the arrow keys move between them.
fn bank_kind_choice(ui: &mut Ui, kind: BankKind) -> BankKind {
    let id = |index: usize| Id::unique(("bank_kind", index));
    let mut chosen = BANK_KINDS
        .iter()
        .position(|(choice, _)| *choice == kind)
        .unwrap_or_default();
    let mut focused = false;
    for (index, (_, name)) in BANK_KINDS.iter().enumerate() {
        let lit = index == chosen;
        let size = vec2(BANK_KIND_WIDTH, CONTROL_HEIGHT);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let response = choice_button(ui, rect, id(index), (name, lit), colour(palette().lit));
        response.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, lit, *name));
        if response.clicked() {
            chosen = index;
        }
        focused |= response.has_focus();
    }
    if focused {
        let stepped = choice_keys(ui)
            .into_iter()
            .fold(chosen, |index, key| match key {
                ChoiceKey::Previous => index.saturating_sub(1),
                ChoiceKey::Next => (index + 1).min(BANK_KINDS.len() - 1),
            });
        if stepped != chosen {
            ui.memory_mut(|memory| memory.request_focus(id(stepped)));
            chosen = stepped;
        }
    }
    BANK_KINDS[chosen].0
}

fn engine_readout_grid(ui: &mut Ui, snapshot: &Snapshot) {
    let rows = engine_readout(snapshot.status.as_ref());
    Grid::new("engine_readout_rows").show(ui, |ui| {
        for (label, value) in rows {
            ui.label(muted_text(label));
            ui.label(value);
            ui.end_row();
        }
    });
}

fn bank_panel(ui: &mut Ui, rows: &[BankRow]) -> Option<Command> {
    let mut clicked = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    for row in rows {
        let number = row.program.to_string();
        let response = bank_row(ui, &number, &row.name, row.active, row.error.is_none());
        match &row.error {
            Some(error) => {
                response.on_hover_text(error);
            }
            None => {
                if response.clicked() {
                    clicked = Some(Command::Load(number));
                }
            }
        }
    }
    clicked
}

/// `Undo` puts the last removed Module or hidden fixed unit back, once.
pub fn footer(
    ui: &mut Ui,
    snapshot: &Snapshot,
    feedback: &Feedback,
    (removed, hidden, layout): (
        &mut Option<RemovedUnit>,
        &mut Option<HiddenFixed>,
        &mut RackLayout,
    ),
    now: Instant,
) -> Option<Command> {
    let mut undo = None;
    ui.horizontal(|ui| {
        if let Some(text) = feedback.notice(now) {
            ui.label(text);
        }
        if let Some(text) = snapshot.status.as_ref().and_then(undo_text) {
            ui.label(text);
            ui.label(muted_text(UNDO_KEY));
        }
        if hidden.as_ref().is_some_and(|hidden| !hidden.is_shown(now)) {
            *hidden = None;
        }
        let status = snapshot.status.as_ref();
        if let Some(shown) = hidden.take() {
            ui.label(format!("Hid {} \u{2014}", shown.unit.in_prose()));
            if ui.add(outlined_button("Undo")).clicked() {
                layout.undo_hide(&shown);
            } else {
                *hidden = Some(shown);
            }
        } else if let Some(id) = shown_undo(removed, status, now).map(|shown| shown.id.clone()) {
            ui.label(format!("Removed {id} \u{2014}"));
            if ui.add(outlined_button("Undo")).clicked() {
                undo = removed
                    .take()
                    .map(|removed| Command::Restore(removed.restore));
            }
        }
    });
    undo
}

/// The error on the open panel with a red edge and a red dot before its title, the detail under it in the value role, at the bottom right of the window until `Dismiss` is pressed.
pub fn error_toast(ctx: &egui::Context, feedback: &mut Feedback) {
    let Some((title, detail)) = feedback
        .error()
        .map(|(title, detail)| (title.to_string(), detail.to_string()))
    else {
        return;
    };
    let colours = palette();
    let mut dismissed = false;
    egui::Area::new(Id::unique("error_toast"))
        .anchor(Align2::RIGHT_BOTTOM, TOAST_OFFSET)
        .order(Order::Foreground)
        .show(ctx, |ui| {
            let frame = Frame::new()
                .fill(colour(colours.panel_open))
                .stroke(Stroke::new(1.0, colour(colours.boundary)))
                .corner_radius(RADIUS_PANEL)
                .inner_margin(TOAST_MARGIN)
                .show(ui, |ui| {
                    ui.set_width(TOAST_WIDTH - TOAST_MARGIN.sum().x);
                    ui.horizontal_top(|ui| {
                        let title_height =
                            ui.fonts_mut(|fonts| fonts.row_height(&theme::dialog_title()));
                        let (dot, _) = ui.allocate_exact_size(
                            vec2(2.0 * TOAST_DOT_RADIUS, title_height),
                            Sense::hover(),
                        );
                        ui.painter().circle_filled(
                            dot.center(),
                            TOAST_DOT_RADIUS,
                            colour(colours.danger),
                        );
                        let dismiss_width = CONTROL_HEIGHT + ui.spacing().item_spacing.x;
                        let text_width = ui.available_width() - dismiss_width;
                        let text = ui.allocate_ui_with_layout(
                            vec2(text_width, 0.0),
                            Layout::top_down(Align::LEFT),
                            |ui| {
                                ui.set_width(text_width);
                                ui.label(RichText::new(&title).font(theme::dialog_title()));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&detail)
                                            .font(theme::value())
                                            .color(colour(colours.muted)),
                                    )
                                    .wrap(),
                                );
                            },
                        );
                        text.response.widget_info(|| {
                            WidgetInfo::labeled(Role::Alert, true, format!("{title}. {detail}"))
                        });
                        let dismiss = ui
                            .add(outlined_button("\u{2715}").min_size(Vec2::splat(CONTROL_HEIGHT)));
                        dismiss.widget_info(|| WidgetInfo::labeled(Role::Button, true, DISMISS));
                        dismissed = dismiss.clicked();
                    });
                });
            let edge = Rect::from_min_size(
                frame.response.rect.min,
                vec2(TOAST_EDGE_WIDTH, frame.response.rect.height()),
            );
            ui.painter().rect_filled(
                edge,
                CornerRadius {
                    nw: RADIUS_PANEL,
                    sw: RADIUS_PANEL,
                    ne: 0,
                    se: 0,
                },
                colour(colours.danger),
            );
        });
    if dismissed {
        feedback.dismiss();
    }
}
