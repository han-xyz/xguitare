use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use egui::{CentralPanel, Event, Frame, Key, Margin, Modifiers, Panel, RichText, Theme, Ui};
use serde_json::Value;
use tracing::{error, info};
use xguitare_core::control::{BusRef, TrackRef};
use xguitare_core::{BankKind, Config};

use crate::client::{self, TICK};
use crate::mixer::{MixerView, RenderDraft, mixer_panel, render_dialog};
use crate::mixer_model::{MASTER, MixerTarget, RackFocus, aim_rack_commands};
use crate::model::{
    ClipLatch, Command, Feedback, KeyEvent, KeyModifiers, Link, PeakHold, PortSelection,
    RemovedUnit, Restore, SET_INTERVAL, SettingsDraft, Snapshot, Throttle, bank_kind, bank_rows,
    forget_removed_units, key_to_command, open_units_from_string, open_units_to_string, palette,
    param_rows, peak_db, quit_commands, remember_removes,
};
use crate::project::{
    ProjectView, TIMELINE_HEIGHT, TIMELINE_HEIGHT_WITH_MIXER, autosave_prompt, tracks_view,
    transport_bar,
};
use crate::project_model::{
    AutosaveOffer, item_blocks, key_active, project_key_to_command, project_name, track_rows,
};
use crate::rack::{Knobs, METERS, RackView, rack_view};
use crate::sidebar::{AddDraft, SidebarView, Tuner, footer, sidebar};
use crate::widgets;

const SIDEBAR_WIDTH: f32 = 260.0;
const SIDEBAR_MARGIN: Margin = Margin::symmetric(20, 24);
const RACK_MARGIN: Margin = Margin::symmetric(8, 24);
const RACK_HEADING_SIZE: f32 = 18.0;
const MIXER_GAP: f32 = 8.0;
const OPEN_UNITS_KEY: &str = "open_units";
const DIGIT_KEYS: [Key; 9] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
];

pub struct App {
    commands: Sender<Command>,
    snapshots: Receiver<Snapshot>,
    client: Option<JoinHandle<()>>,
    snapshot: Snapshot,
    stop: Arc<AtomicBool>,
    throttle: Throttle,
    locals: HashMap<String, f64>,
    feedback: Feedback,
    tuner_open: bool,
    muted_by_us: bool,
    tuner_mute: bool,
    clips: [ClipLatch; 2],
    holds: [PeakHold; 2],
    open_units: BTreeSet<String>,
    removing: BTreeSet<String>,
    removed: Option<RemovedUnit>,
    bank_query: String,
    save_as_name: Option<String>,
    add_draft: AddDraft,
    ports_draft: Option<PortSelection>,
    settings_draft: Option<SettingsDraft>,
    configured_bank_kind: BankKind,
    new_project: String,
    project_view: ProjectView,
    mixer: MixerView,
    autosave_prompt: Option<AutosaveOffer>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        config: &Config,
        config_path: Option<PathBuf>,
        stop: Arc<AtomicBool>,
    ) -> App {
        let ctx = cc.egui_ctx.clone();
        let (commands, snapshots, client) =
            client::spawn(config.clone(), config_path, Arc::clone(&stop), move || {
                ctx.request_repaint();
            });
        App::assemble(cc, config, commands, snapshots, Some(client), stop)
    }

    /// An App with no client thread: `snapshot` arrives as the only client update and its commands go to `commands`.
    #[allow(
        clippy::missing_panics_doc,
        reason = "the expect is an invariant: the receiver is held in this scope"
    )]
    pub fn with_snapshot(
        cc: &eframe::CreationContext,
        config: &Config,
        snapshot: Snapshot,
        commands: Sender<Command>,
    ) -> App {
        let (updates, snapshots) = mpsc::channel();
        updates
            .send(snapshot)
            .expect("the receiver is held in this scope");
        let stop = Arc::new(AtomicBool::new(false));
        App::assemble(cc, config, commands, snapshots, None, stop)
    }

    fn assemble(
        cc: &eframe::CreationContext,
        config: &Config,
        commands: Sender<Command>,
        snapshots: Receiver<Snapshot>,
        client: Option<JoinHandle<()>>,
        stop: Arc<AtomicBool>,
    ) -> App {
        let open_units = cc
            .storage
            .and_then(|storage| storage.get_string(OPEN_UNITS_KEY))
            .map(|stored| open_units_from_string(&stored))
            .unwrap_or_default();
        cc.egui_ctx.set_theme(Theme::Dark);
        cc.egui_ctx.set_visuals_of(Theme::Dark, widgets::visuals());
        App {
            commands,
            snapshots,
            client,
            snapshot: Snapshot {
                link: Link::Disconnected {
                    since: Instant::now(),
                },
                status: None,
                bank: None,
                params: None,
                tuner: None,
                ports: None,
                settings: None,
                projects: None,
                items: None,
                module_types: Vec::new(),
                irs: Vec::new(),
                last_message: None,
                failed_set: None,
                refused_remove: None,
                refused_render: None,
                autosave_offer: None,
            },
            stop,
            throttle: Throttle::default(),
            locals: HashMap::new(),
            feedback: Feedback::default(),
            tuner_open: false,
            muted_by_us: false,
            tuner_mute: config.tuner_mute,
            clips: [ClipLatch::default(); 2],
            holds: [PeakHold::default(); 2],
            open_units,
            removing: BTreeSet::new(),
            removed: None,
            bank_query: String::new(),
            save_as_name: None,
            add_draft: AddDraft::default(),
            ports_draft: None,
            settings_draft: None,
            configured_bank_kind: config.bank_kind,
            new_project: String::new(),
            project_view: ProjectView::default(),
            mixer: MixerView::default(),
            autosave_prompt: None,
        }
    }
}

impl App {
    fn release_stale_holds(&mut self, snapshot: &Snapshot) {
        if snapshot.failed_set != self.snapshot.failed_set
            && let Some((address, _)) = &snapshot.failed_set
        {
            self.throttle.forget(address);
        }
        if active_preset(snapshot) != active_preset(&self.snapshot) {
            self.throttle = Throttle::default();
            return;
        }
        let present: HashSet<String> = snapshot
            .params
            .as_ref()
            .map(param_rows)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|(_, rows)| rows.into_iter().map(|row| row.address))
            .collect();
        self.throttle.retain(|address| present.contains(address));
    }
}

impl App {
    /// The last [`Command::RackRemove`] among `commands` becomes the footer's Undo, from the `get` payload shown when it was sent; a `load` or a Bank kind change drops it, even of the same Preset, and the footer drops it when the active Preset changes ([`crate::model::shown_undo`]); an IR-folder-only settings change keeps it.
    fn remember_undo(&mut self, commands: &[Command], now: Instant) {
        let mut aimed = None;
        for command in commands {
            match command {
                Command::ShowRack(target) => aimed = Some(*target),
                Command::RackRemove { id } => {
                    let status = self.snapshot.status.as_ref();
                    let removed = self
                        .snapshot
                        .params
                        .as_ref()
                        .and_then(|params| RemovedUnit::new(params, status, id, now));
                    if let Some(mut removed) = removed {
                        if let Some(target) = aimed {
                            aim_restore(&mut removed.restore, status, target);
                        }
                        self.removed = Some(removed);
                    }
                }
                Command::Load(_)
                | Command::SetSettings {
                    bank_kind: Some(_), ..
                }
                | Command::ProjectNew(_)
                | Command::ProjectOpen(_)
                | Command::ProjectClose
                | Command::RestoreAutosave(_) => self.removed = None,
                _ => {}
            }
        }
    }
}

impl App {
    /// Aims every Rack request at the Rack the window shows ([`aim_rack_commands`]) and takes the window's own commands out: Ctrl+M or `Mixer` opens or closes the mixer panel, Ctrl+Alt+R or `Render` opens the Render dialog.
    fn keep_window_commands(&mut self, commands: Vec<Command>) -> Vec<Command> {
        let focus = RackFocus {
            mixer_open: self.mixer.open,
            shown: self.mixer.rack,
            selected: self.snapshot.status.as_ref().and_then(selected_index),
        };
        let (mut commands, focus) = aim_rack_commands(focus, commands);
        self.mixer.open = focus.mixer_open;
        self.mixer.rack = focus.shown;
        if commands.contains(&Command::OpenRenderDialog) {
            self.mixer.render = Some(RenderDraft::new(&self.snapshot));
        }
        commands
            .retain(|command| !matches!(command, Command::ToggleMixer | Command::OpenRenderDialog));
        commands
    }
}

/// A Module goes back to the Rack its remove was aimed at; a Track and a Bus are named, so that Undo finds them after another is removed.
fn aim_restore(restore: &mut Restore, status: Option<&Value>, target: MixerTarget) {
    let name = |key: &str, index: usize| {
        status.and_then(|status| status[key][index]["name"].as_str().map(str::to_string))
    };
    match target {
        MixerTarget::Track(index) => {
            restore.track =
                Some(name("tracks", index).map_or(TrackRef::Index(index), TrackRef::Name));
        }
        MixerTarget::Bus(index) => {
            restore.track = None;
            restore.bus = Some(name("buses", index).map_or(BusRef::Index(index), BusRef::Name));
        }
        MixerTarget::Master => {
            restore.track = None;
            restore.master = true;
        }
    }
}

fn active_preset(snapshot: &Snapshot) -> (Option<&Value>, Option<&Value>) {
    let status = snapshot.status.as_ref();
    (
        status.map(|status| &status["preset"]),
        status.map(|status| &status["program"]),
    )
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.stop.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let mut latest = None;
        let now = Instant::now();
        for snapshot in self.snapshots.try_iter() {
            if let Some(status) = &snapshot.status {
                for (((_, key), clip), hold) in
                    METERS.iter().zip(&mut self.clips).zip(&mut self.holds)
                {
                    clip.observe(peak_db(status, key));
                    hold.observe(peak_db(status, key), now);
                }
                self.mixer.observe(status, now);
            }
            latest = Some(snapshot);
        }
        if let Some(snapshot) = latest {
            if snapshot.last_message != self.snapshot.last_message
                && let Some((text, at)) = &snapshot.last_message
            {
                self.feedback.show(text.clone(), *at);
            }
            self.release_stale_holds(&snapshot);
            if snapshot.refused_remove != self.snapshot.refused_remove
                && let Some((id, _)) = &snapshot.refused_remove
            {
                self.removing.remove(id);
                if self
                    .removed
                    .as_ref()
                    .is_some_and(|removed| &removed.id == id)
                {
                    self.removed = None;
                }
            }
            if snapshot.autosave_offer != self.snapshot.autosave_offer
                && let Some((offer, _)) = &snapshot.autosave_offer
            {
                self.autosave_prompt = Some(offer.clone());
            }
            if snapshot.params != self.snapshot.params
                && let Some(params) = &snapshot.params
            {
                forget_removed_units(&mut self.open_units, &mut self.removing, params);
            }
            self.snapshot = snapshot;
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let snapshot = &self.snapshot;
        let now = Instant::now();
        let rows = snapshot
            .bank
            .as_ref()
            .map(|list| bank_rows(list).0)
            .unwrap_or_default();
        let project_status = snapshot
            .status
            .as_ref()
            .filter(|status| project_name(Some(status)).is_some());
        let items = snapshot.items.as_ref().map(item_blocks).unwrap_or_default();
        let mut commands: Vec<Command> = pressed_keys(ui)
            .into_iter()
            .filter_map(|key| match project_status {
                Some(status) => project_key_to_command(key, status, &items),
                None => key_to_command(key, &rows),
            })
            .collect();
        commands.extend(self.throttle.tick(now));
        Panel::bottom("status").show(ui, |ui| {
            commands.extend(footer(ui, snapshot, &self.feedback, &mut self.removed, now));
        });
        let mut tuner = Tuner {
            open: &mut self.tuner_open,
            muted_by_us: &mut self.muted_by_us,
            mute_default: self.tuner_mute,
        };
        let mut panel = SidebarView {
            rows: &rows,
            bank_kind: bank_kind(snapshot.settings.as_ref(), self.configured_bank_kind),
            query: &mut self.bank_query,
            save_as_name: &mut self.save_as_name,
            add: &mut self.add_draft,
            stereo_rack: self.mixer.shows_stereo_rack(),
            tuner: &mut tuner,
            ports: &mut self.ports_draft,
            settings: &mut self.settings_draft,
            new_project: &mut self.new_project,
        };
        let sidebar_frame = Frame::side_top_panel(ui.style())
            .fill(widgets::colour(palette().sidebar))
            .inner_margin(SIDEBAR_MARGIN);
        Panel::left("sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .frame(sidebar_frame)
            .show(ui, |ui| commands.extend(sidebar(ui, snapshot, &mut panel)));
        if project_status.is_some() {
            let feedback = (&mut self.feedback, now);
            commands.extend(project_panels(
                ui,
                snapshot,
                (&mut self.project_view, self.mixer.open),
                feedback,
            ));
        } else {
            self.project_view = ProjectView::default();
            self.mixer = MixerView::default();
        }
        commands.extend(open_render_dialog(ui.ctx(), snapshot, &mut self.mixer));
        if let Some(offer) = self.autosave_prompt.clone()
            && let Some(restore) = autosave_prompt(ui.ctx(), &offer.name)
        {
            if restore {
                commands.push(Command::RestoreAutosave(offer.folder));
            }
            self.autosave_prompt = None;
        }
        let mut knobs = Knobs {
            throttle: &mut self.throttle,
            locals: &mut self.locals,
            feedback: &mut self.feedback,
            revision: revision(snapshot),
            now,
        };
        let mut view = RackView {
            knobs: &mut knobs,
            tuner_open: self.tuner_open,
            muted_by_tuner: self.tuner_open && self.muted_by_us,
            clips: &mut self.clips,
            holds: &self.holds,
            open_units: &mut self.open_units,
        };
        CentralPanel::default()
            .frame(Frame::central_panel(ui.style()).inner_margin(RACK_MARGIN))
            .show(ui, |ui| {
                commands.extend(central_view(ui, snapshot, &mut self.mixer, &mut view));
            });
        let commands = self.keep_window_commands(commands);
        remember_removes(&mut self.removing, &commands);
        self.remember_undo(&commands, now);
        for command in commands {
            // An Err means the client thread has ended; on_exit joins it.
            let _ = self.commands.send(command);
        }
        ui.ctx().request_repaint_after(TICK);
        if self.throttle.has_pending() {
            ui.ctx().request_repaint_after(SET_INTERVAL);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(OPEN_UNITS_KEY, open_units_to_string(&self.open_units));
    }

    fn on_exit(&mut self) {
        // An Err means the client thread has already ended; joining it is still right.
        let commands = quit_commands(self.tuner_open, self.muted_by_us);
        info!("quit: queued {commands:?}");
        for command in commands {
            let _ = self.commands.send(command);
        }
        if let Some(client) = self.client.take()
            && client.join().is_err()
        {
            error!("client thread panicked");
        }
    }
}

/// The keys [`key_active`] lets through. Keys match the key event's own modifiers: an injected or fast press releases Ctrl within the same frame, so the frame's modifier state reads none.
fn pressed_keys(ui: &Ui) -> Vec<KeyEvent> {
    let text_edit_focused = ui.ctx().text_edit_focused();
    let widget_focused = ui.ctx().memory(|memory| memory.focused().is_some());
    let keys = ui.input_mut(|input| {
        let mut keys: Vec<KeyEvent> = input
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => key_event(*key, *modifiers),
                _ => None,
            })
            .collect();
        if input.consume_key(Modifiers::COMMAND, Key::S) {
            keys.push(KeyEvent::CtrlS);
        }
        if input.consume_key(Modifiers::COMMAND | Modifiers::ALT, Key::R) {
            keys.push(KeyEvent::CtrlAltR);
        }
        if input.consume_key(Modifiers::COMMAND, Key::R) {
            keys.push(KeyEvent::CtrlR);
        }
        if input.consume_key(Modifiers::COMMAND, Key::M) {
            keys.push(KeyEvent::CtrlM);
        }
        keys
    });
    keys.into_iter()
        .filter(|&key| key_active(key, text_edit_focused, widget_focused))
        .collect()
}

/// Digits with their modifiers; Space, R, Home and End only without one.
fn key_event(key: Key, modifiers: Modifiers) -> Option<KeyEvent> {
    if let Some((digit, _)) = (1..)
        .zip(DIGIT_KEYS)
        .find(|(_, digit_key)| *digit_key == key)
    {
        return Some(KeyEvent::Digit(
            digit,
            KeyModifiers {
                ctrl: modifiers.ctrl,
                alt: modifiers.alt,
            },
        ));
    }
    if !modifiers.is_none() {
        return None;
    }
    match key {
        Key::Space => Some(KeyEvent::Space),
        Key::R => Some(KeyEvent::R),
        Key::Home => Some(KeyEvent::Home),
        Key::End => Some(KeyEvent::End),
        _ => None,
    }
}

/// The transport bar, then the Tracks with their timeline, above the rack.
fn project_panels(
    ui: &mut Ui,
    snapshot: &Snapshot,
    (view, mixer_open): (&mut ProjectView, bool),
    feedback: (&mut Feedback, Instant),
) -> Vec<Command> {
    let mut commands = Vec::new();
    Panel::top("transport")
        .frame(Frame::side_top_panel(ui.style()).inner_margin(Margin::symmetric(12, 6)))
        .show(ui, |ui| {
            commands.extend(transport_bar(ui, snapshot, mixer_open, feedback));
        });
    let (timeline_id, timeline_height) = if mixer_open {
        ("timeline_with_mixer", TIMELINE_HEIGHT_WITH_MIXER)
    } else {
        ("timeline", TIMELINE_HEIGHT)
    };
    Panel::top(timeline_id)
        .resizable(true)
        .default_size(timeline_height)
        .frame(Frame::new().fill(widgets::colour(palette().rail)))
        .show(ui, |ui| commands.extend(tracks_view(ui, snapshot, view)));
    commands
}

/// The Rack alone with no Project open; with one, the mixer panel while it is open with the Rack of the column chosen in it below, else the selected Track's Rack.
fn central_view(
    ui: &mut Ui,
    snapshot: &Snapshot,
    mixer: &mut MixerView,
    view: &mut RackView,
) -> Vec<Command> {
    let Some(status) = snapshot
        .status
        .as_ref()
        .filter(|status| project_name(Some(status)).is_some())
    else {
        return rack_view(ui, snapshot, view);
    };
    let mut commands = Vec::new();
    if mixer.open {
        commands.extend(mixer_panel(ui, snapshot, mixer));
        ui.add_space(MIXER_GAP);
    }
    let shown = if mixer.open {
        mixer.rack
    } else {
        selected_index(status).map(MixerTarget::Track)
    };
    if let Some(target) = shown {
        rack_heading(ui, status, target);
        commands.extend(rack_view(ui, snapshot, view));
    }
    commands
}

fn open_render_dialog(
    ctx: &egui::Context,
    snapshot: &Snapshot,
    mixer: &mut MixerView,
) -> Vec<Command> {
    let Some(draft) = &mut mixer.render else {
        return Vec::new();
    };
    let (commands, open) = render_dialog(ctx, snapshot, draft);
    if !open {
        mixer.render = None;
    }
    commands
}

fn selected_index(status: &Value) -> Option<usize> {
    track_rows(status)
        .into_iter()
        .find(|row| row.selected)
        .map(|row| row.index)
}

/// Names the Track, Bus or Master whose Rack is shown.
fn rack_heading(ui: &mut Ui, status: &Value, target: MixerTarget) {
    let name = match target {
        MixerTarget::Track(index) => status["tracks"][index]["name"].as_str(),
        MixerTarget::Bus(index) => status["buses"][index]["name"].as_str(),
        MixerTarget::Master => Some(MASTER),
    };
    let heading = format!("{} — Rack", name.unwrap_or_default());
    ui.label(RichText::new(heading).size(RACK_HEADING_SIZE).strong());
}

fn revision(snapshot: &Snapshot) -> u64 {
    snapshot
        .status
        .as_ref()
        .and_then(|status| status["revision"].as_u64())
        .unwrap_or_default()
}
