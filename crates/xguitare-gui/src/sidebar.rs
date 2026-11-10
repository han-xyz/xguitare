use std::time::Instant;

use egui::{
    Button, CollapsingHeader, ComboBox, Grid, Key, RichText, ScrollArea, TextEdit, Ui, vec2,
};
use xguitare_core::BankKind;

use crate::model::{
    AddPlace, BankRow, Command, Feedback, Link, ModuleType, PortSelection, SECTIONS, Section,
    SettingsDraft, Snapshot, add_command, add_place, add_places, engine_readout, filter_bank,
    link_text, param_rows, port_choices, rack_channels, set_ports_command, set_settings_command,
    settings_draft, tuner_toggle,
};
use crate::rack::muted_text;

const NO_PRESET: &str = "no preset";
const PRESET_NAME_SIZE: f32 = 30.0;
const SIDEBAR_BUTTON_HEIGHT: f32 = 40.0;
const UNSAVED: &str = "unsaved changes";
const SAVE_AS_LABEL: &str = "Save as new";
const SAVE_AS_HINT: &str = "New Preset name";
const SEARCH_HINT: &str = "Search by name or number";
const CHOOSE: &str = "choose";
const NO_PORTS: &str = "no port list from the Daemon";
const NO_SETTINGS: &str = "no settings from the Daemon";
const BANK_KINDS: [(BankKind, &str); 2] = [(BankKind::Guitar, "Guitar"), (BankKind::Bass, "Bass")];

pub struct Tuner<'a> {
    pub open: &'a mut bool,
    pub muted_by_us: &'a mut bool,
    pub mute_default: bool,
}

impl Tuner<'_> {
    fn toggle(&mut self) -> Option<Command> {
        let (open, command, muted_by_us) =
            tuner_toggle(*self.open, self.mute_default, *self.muted_by_us);
        *self.open = open;
        *self.muted_by_us = muted_by_us;
        command
    }
}

/// The Add Module picker's choices; `place` `None` follows the Rack rules for the chosen type.
#[derive(Debug, Default)]
pub struct AddDraft {
    pub type_name: Option<String>,
    pub place: Option<AddPlace>,
}

/// The left panel's own state: a draft `None` shows what the Daemon reports.
pub struct SidebarView<'a, 'b> {
    pub rows: &'a [BankRow],
    pub bank_kind: BankKind,
    pub query: &'a mut String,
    pub save_as_name: &'a mut Option<String>,
    pub add: &'a mut AddDraft,
    pub tuner: &'a mut Tuner<'b>,
    pub ports: &'a mut Option<PortSelection>,
    pub settings: &'a mut Option<SettingsDraft>,
}

/// One collapsible section per [`SECTIONS`] entry, top to bottom.
pub fn sidebar(ui: &mut Ui, snapshot: &Snapshot, view: &mut SidebarView) -> Vec<Command> {
    let mut commands = Vec::new();
    ScrollArea::vertical().show(ui, |ui| {
        for section in SECTIONS {
            CollapsingHeader::new(section.title(view.bank_kind))
                .id_salt(("section", section as u8))
                .default_open(section.open_by_default())
                .show(ui, |ui| {
                    commands.extend(section_body(ui, section, snapshot, view))
                });
        }
    });
    commands
}

fn section_body(
    ui: &mut Ui,
    section: Section,
    snapshot: &Snapshot,
    view: &mut SidebarView,
) -> Vec<Command> {
    match section {
        Section::Preset => {
            preset_heading(ui, snapshot);
            save_buttons(ui, view.save_as_name).into_iter().collect()
        }
        Section::Bank => {
            ui.add(TextEdit::singleline(view.query).hint_text(SEARCH_HINT));
            bank_panel(ui, &filter_bank(view.rows, view.query))
                .into_iter()
                .collect()
        }
        Section::Rack => add_module(ui, snapshot, view.add).into_iter().collect(),
        Section::Tuner => tuner_and_mute(ui, snapshot, view.tuner),
        Section::Audio => audio_ports(ui, snapshot, view.ports),
        Section::Settings => settings(ui, snapshot, view.settings).into_iter().collect(),
        Section::Engine => {
            engine_readout_grid(ui, snapshot);
            Vec::new()
        }
    }
}

fn preset_heading(ui: &mut Ui, snapshot: &Snapshot) {
    let status = snapshot.status.as_ref();
    let preset = status.and_then(|status| status["preset"].as_str());
    ui.label(
        RichText::new(preset.unwrap_or(NO_PRESET))
            .size(PRESET_NAME_SIZE)
            .strong(),
    );
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
        SIDEBAR_BUTTON_HEIGHT,
    );
    let mut command = None;
    ui.horizontal(|ui| {
        if ui.add(Button::new("Save").min_size(button_size)).clicked() {
            command = Some(Command::Save);
        }
        if ui
            .add(Button::new(SAVE_AS_LABEL).min_size(button_size))
            .clicked()
        {
            *save_as_name = save_as_name.is_none().then(String::new);
        }
    });
    let Some(name) = save_as_name else {
        return command;
    };
    let field = ui.add(TextEdit::singleline(name).hint_text(SAVE_AS_HINT));
    let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    let (confirm, cancel) = ui
        .horizontal(|ui| (ui.button("Save").clicked(), ui.button("Cancel").clicked()))
        .inner;
    if entered || confirm {
        command = Some(Command::SaveAs(name.trim().to_string()));
        *save_as_name = None;
    } else if cancel {
        *save_as_name = None;
    }
    command
}

fn add_module(ui: &mut Ui, snapshot: &Snapshot, add: &mut AddDraft) -> Option<Command> {
    let modules = snapshot.params.as_ref().map(param_rows).unwrap_or_default();
    let rack = rack_channels(&modules, &snapshot.module_types);
    let chosen: Option<&ModuleType> = add.type_name.as_ref().and_then(|type_name| {
        snapshot
            .module_types
            .iter()
            .find(|module_type| &module_type.type_name == type_name)
    });
    let width = ui.available_width();
    let type_label = ui.label("Add Module");
    ComboBox::from_id_salt("add_type")
        .width(width)
        .selected_text(chosen.map_or(CHOOSE.to_string(), ModuleType::label))
        .show_ui(ui, |ui| {
            for module_type in &snapshot.module_types {
                let selected = chosen == Some(module_type);
                if ui.selectable_label(selected, module_type.label()).clicked() {
                    add.type_name = Some(module_type.type_name.clone());
                }
            }
        })
        .response
        .labelled_by(type_label.id);
    let channels = chosen.map_or("", |module_type| module_type.channels.as_str());
    let place = add_place(&rack, channels, add.place.as_ref());
    let place_label = ui.label("Place");
    ComboBox::from_id_salt("add_place")
        .width(width)
        .selected_text(place.label())
        .show_ui(ui, |ui| {
            for option in add_places(&rack) {
                if ui
                    .selectable_label(option == place, option.label())
                    .clicked()
                {
                    add.place = Some(option);
                }
            }
        })
        .response
        .labelled_by(place_label.id);
    let clicked = ui
        .add_enabled(chosen.is_some(), Button::new("Add"))
        .clicked();
    let type_name = chosen.filter(|_| clicked)?.type_name.clone();
    *add = AddDraft::default();
    Some(add_command(&type_name, &place))
}

/// Turning Mute off by hand means closing the Tuner view leaves it off.
fn tuner_and_mute(ui: &mut Ui, snapshot: &Snapshot, tuner: &mut Tuner) -> Vec<Command> {
    let muted = snapshot
        .status
        .as_ref()
        .and_then(|status| status["muted"].as_bool());
    let button_size = vec2(
        (ui.available_width() - ui.spacing().item_spacing.x) / 2.0,
        SIDEBAR_BUTTON_HEIGHT,
    );
    let mut commands = Vec::new();
    ui.horizontal(|ui| {
        let tuner_button = Button::new("Tuner")
            .selected(*tuner.open)
            .min_size(button_size);
        if ui.add(tuner_button).clicked() {
            commands.extend(tuner.toggle());
        }
        let on = muted.unwrap_or_default();
        let mute_button = Button::new("Mute").selected(on).min_size(button_size);
        if ui.add_enabled(muted.is_some(), mute_button).clicked() {
            if on {
                *tuner.muted_by_us = false;
            }
            commands.push(Command::Mute(!on));
        }
    });
    commands
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
        let label = ui.label(name);
        let combo = ComboBox::from_id_salt(("port", name))
            .width(ui.available_width())
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
        .add_enabled(apply.is_some(), Button::new("Apply"))
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
    ui.label("Bank kind");
    ui.horizontal(|ui| {
        for (kind, name) in BANK_KINDS {
            ui.selectable_value(&mut edited.bank_kind, kind, name);
        }
    });
    let label = ui.label("IR folder");
    ui.add(TextEdit::singleline(&mut edited.ir_dir).desired_width(f32::INFINITY))
        .labelled_by(label.id);
    let apply = set_settings_command(&current, &edited);
    let clicked = ui
        .add_enabled(apply.is_some(), Button::new("Apply"))
        .clicked();
    if clicked {
        *draft = None;
        return apply;
    }
    *draft = (edited != current).then_some(edited);
    None
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
    for row in rows {
        let text = format!("{:>2} {}", row.program, row.name);
        match &row.error {
            Some(error) => {
                ui.add_enabled(false, Button::selectable(false, text))
                    .on_disabled_hover_text(error);
            }
            None => {
                if ui.selectable_label(row.active, text).clicked() {
                    clicked = Some(Command::Load(row.program.to_string()));
                }
            }
        }
    }
    clicked
}

pub fn footer(ui: &mut Ui, snapshot: &Snapshot, feedback: &Feedback, now: Instant) {
    ui.horizontal(|ui| {
        if snapshot.link != Link::Connected {
            ui.label(link_text(&snapshot.link, now));
        }
        if let Some(text) = feedback.text(now) {
            ui.label(text);
        }
    });
}
