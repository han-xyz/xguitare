use std::time::Instant;

use egui::{Button, Grid, Key, Panel, RichText, ScrollArea, TextEdit, Ui, vec2};
use xguitare_gui::model::{
    BankRow, Command, Feedback, Link, Snapshot, engine_readout, filter_bank, link_text,
};

use crate::rack::{UNIT_MARGIN, muted_text};

const NO_PRESET: &str = "no preset";
const PRESET_NAME_SIZE: f32 = 30.0;
const SIDEBAR_BUTTON_HEIGHT: f32 = 40.0;
const UNSAVED: &str = "unsaved changes";
const SAVE_AS_LABEL: &str = "Save as new";
const SAVE_AS_HINT: &str = "New Preset name";
const SEARCH_HINT: &str = "Search by name or number";

pub struct BankView<'a> {
    pub rows: &'a [BankRow],
    pub heading: &'static str,
    pub query: &'a mut String,
    pub save_as_name: &'a mut Option<String>,
}

/// The Preset, saving, the Bank heading and search at the top, the engine readout at the bottom, the Bank list between.
pub fn sidebar(ui: &mut Ui, snapshot: &Snapshot, bank: &mut BankView) -> Vec<Command> {
    preset_heading(ui, snapshot);
    let mut commands: Vec<Command> = save_buttons(ui, bank.save_as_name).into_iter().collect();
    ui.add_space(UNIT_MARGIN.into());
    ui.heading(bank.heading);
    ui.add(TextEdit::singleline(bank.query).hint_text(SEARCH_HINT));
    Panel::bottom("engine_readout").show(ui, |ui| engine_readout_grid(ui, snapshot));
    ScrollArea::vertical().show(ui, |ui| {
        commands.extend(bank_panel(ui, &filter_bank(bank.rows, bank.query)))
    });
    commands
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
