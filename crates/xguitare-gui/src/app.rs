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
use xguitare_client::ClientConfig;
use xguitare_protocol::{BankKind, BusRef, GetReply, TrackRef, to_payload};

use crate::client::{self, TICK};
use crate::metronome_model::{BeatLight, beat_light, metronome_key_to_command};
use crate::mixer::{MixerView, RenderDraft, mixer_panel, render_dialog};
use crate::mixer_model::{MASTER, MixerTarget, RackFocus, aim_rack_commands, shown_rack_column};
use crate::model::{
    BankRow, CleanTakesView, ClipLatch, Command, Feedback, HiddenFixed, KeyEvent, KeyModifiers,
    Link, PeakHold, PortSelection, RackLayout, RemovedUnit, Restore, SET_INTERVAL, SettingsDraft,
    Snapshot, Throttle, bank_kind, bank_rows, bank_rows_for_rack, esc_dismisses_toast,
    forget_removed_units, key_to_command, open_units_from_string, open_units_to_string, param_rows,
    peak_db, quit_commands, remember_removes,
};
use crate::project::{
    ProjectView, TIMELINE_HEIGHT, autosave_prompt, item_node_id, tracks_view, transport_bar,
};
use crate::project_model::{
    AutosaveOffer, KeyFocus, MainView, ViewSwitch, copied_text, esc_clears_selection, item_blocks,
    item_key_to_command, key_active, project_key_to_command, project_name, recording_from,
    restored_view, transport_view,
};
use crate::rack::{Knobs, METERS, RackView, rack_view};
use crate::sidebar::{
    AddDraft, SidebarView, Tuner, clean_takes_dialog, error_toast, footer, sidebar,
};
use crate::theme::{self, colour, palette};
use crate::widgets;

const SIDEBAR_WIDTH: f32 = 260.0;
const SIDEBAR_MARGIN: Margin = Margin {
    left: theme::SPACE_M as i8,
    right: theme::SPACE_XS as i8,
    top: theme::SPACE_M as i8,
    bottom: theme::SPACE_M as i8,
};
const RACK_MARGIN: Margin = Margin::symmetric(8, 24);
const MIXER_MARGIN: Margin = Margin::same(8);
const TRANSPORT_MARGIN: Margin = Margin {
    left: 8,
    right: 8,
    top: 8,
    bottom: 4,
};
const TIMELINE_MARGIN: Margin = Margin {
    left: 8,
    right: 8,
    top: 0,
    bottom: 4,
};
/// The least the split under the Tracks and timeline leaves the Rack, which scrolls inside it.
const RACK_MIN_HEIGHT: f32 = 120.0;
/// The Tracks and timeline's default share of the height below the transport bar, up to [`TIMELINE_HEIGHT`].
const TIMELINE_SHARE: f32 = 0.45;
const OPEN_UNITS_KEY: &str = "open_units";
const VIEW_KEY: &str = "view";
const RACK_LAYOUT_KEY: &str = "rack_layout";
/// The Snap switch in eframe storage: `on` or `off`.
const SNAP_KEY: &str = "snap";
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
    rack_layout: RackLayout,
    hidden: Option<HiddenFixed>,
    bank_query: String,
    save_as_name: Option<String>,
    add_draft: AddDraft,
    ports_draft: Option<PortSelection>,
    settings_draft: Option<SettingsDraft>,
    configured_bank_kind: BankKind,
    new_project: String,
    project_view: ProjectView,
    mixer: MixerView,
    views: ViewSwitch,
    autosave_prompt: Option<AutosaveOffer>,
    clean_takes: Option<CleanTakesView>,
    beat_light: BeatLight,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        config: &ClientConfig,
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
        config: &ClientConfig,
        snapshot: Snapshot,
        commands: Sender<Command>,
    ) -> App {
        let (updates, snapshots) = mpsc::channel();
        updates
            .send(snapshot)
            .expect("the receiver is held in this scope");
        App::with_updates(cc, config, snapshots, commands)
    }

    /// An App with no client thread: each Snapshot sent on `snapshots` arrives as a client update and its commands go to `commands`.
    pub fn with_updates(
        cc: &eframe::CreationContext,
        config: &ClientConfig,
        snapshots: Receiver<Snapshot>,
        commands: Sender<Command>,
    ) -> App {
        let stop = Arc::new(AtomicBool::new(false));
        App::assemble(cc, config, commands, snapshots, None, stop)
    }

    fn assemble(
        cc: &eframe::CreationContext,
        config: &ClientConfig,
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
        let stored_view = cc.storage.and_then(|storage| storage.get_string(VIEW_KEY));
        let snap = cc
            .storage
            .and_then(|storage| storage.get_string(SNAP_KEY))
            .is_none_or(|stored| stored != "off");
        let rack_layout = cc
            .storage
            .and_then(|storage| storage.get_string(RACK_LAYOUT_KEY))
            .map(|stored| RackLayout::from_stored(&stored))
            .unwrap_or_default();
        cc.egui_ctx.set_theme(Theme::Dark);
        cc.egui_ctx.set_fonts(theme::fonts());
        cc.egui_ctx
            .all_styles_mut(|style| style.text_styles = theme::text_styles());
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
                peaks: HashMap::new(),
                module_types: Vec::new(),
                irs: Vec::new(),
                last_message: None,
                failed_set: None,
                refused_remove: None,
                refused_render: None,
                autosave_offer: None,
                unused_takes: None,
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
            rack_layout,
            hidden: None,
            bank_query: String::new(),
            save_as_name: None,
            add_draft: AddDraft::default(),
            ports_draft: None,
            settings_draft: None,
            configured_bank_kind: config.bank_kind,
            new_project: String::new(),
            project_view: ProjectView::new(snap),
            mixer: MixerView::default(),
            views: ViewSwitch::new(restored_view(stored_view.as_deref())),
            autosave_prompt: None,
            clean_takes: None,
            beat_light: BeatLight::default(),
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
                Command::ShowRack(shown) => {
                    aimed = self
                        .snapshot
                        .status
                        .as_ref()
                        .and_then(|status| shown.column(&to_payload(status)));
                }
                Command::RackRemove { id } => {
                    let status = self.snapshot.status.as_ref();
                    let removed = self
                        .snapshot
                        .params
                        .as_ref()
                        .and_then(|params| RemovedUnit::new(params, status, id, now));
                    if let Some(mut removed) = removed {
                        if let Some(target) = aimed {
                            aim_restore(
                                &mut removed.restore,
                                status.map(to_payload).as_ref(),
                                target,
                            );
                        }
                        self.removed = Some(removed);
                        self.hidden = None;
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
    /// Aims every Rack request at the Rack the window shows ([`aim_rack_commands`]) and takes the window's own commands out: the view switch, Ctrl+1, Ctrl+2, Ctrl+3 and Ctrl+M change the main view, Ctrl+Alt+R or `Render` opens the Render dialog.
    fn keep_window_commands(&mut self, commands: Vec<Command>) -> Vec<Command> {
        let status = self.snapshot.status.as_ref().map(to_payload);
        let focus = RackFocus::new(self.mixer.rack, status.as_ref());
        let (mut commands, focus) = aim_rack_commands(focus, commands);
        self.mixer.rack = focus.shown;
        for command in &commands {
            self.views.follow(command);
        }
        if commands.contains(&Command::OpenRenderDialog) {
            self.mixer.render = Some(RenderDraft::new(&self.snapshot));
        }
        let open_clean_takes = commands.contains(&Command::OpenCleanTakesDialog);
        commands.retain(|command| {
            !matches!(
                command,
                Command::ShowView(_)
                    | Command::ToggleMixer
                    | Command::OpenRenderDialog
                    | Command::OpenCleanTakesDialog
            )
        });
        if open_clean_takes {
            self.clean_takes = Some(CleanTakesView::Reading);
            commands.push(Command::RefreshUnusedTakes);
        }
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

fn active_preset(snapshot: &Snapshot) -> Option<(Option<&str>, Option<usize>)> {
    snapshot
        .status
        .as_ref()
        .map(|status| (status.preset.as_deref(), status.program))
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
                    clip.observe(peak_db(key(status)));
                    hold.observe(peak_db(key(status)), now);
                }
                self.mixer.observe(&to_payload(status), now);
            }
            latest = Some(snapshot);
        }
        if let Some(snapshot) = latest {
            if snapshot.last_message != self.snapshot.last_message
                && let Some((message, at)) = &snapshot.last_message
            {
                self.feedback.show(message.clone(), *at);
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
            if snapshot.unused_takes != self.snapshot.unused_takes
                && let Some((reply, _)) = &snapshot.unused_takes
                && self.clean_takes == Some(CleanTakesView::Reading)
            {
                self.clean_takes = Some(CleanTakesView::from_reply(reply));
            }
            if snapshot.params != self.snapshot.params
                && let Some(params) = &snapshot.params
            {
                forget_removed_units(&mut self.open_units, &mut self.removing, params);
                if let Some(previous) = &self.snapshot.params {
                    let ids = |params: &GetReply| -> Vec<String> {
                        param_rows(params).into_iter().map(|(id, _)| id).collect()
                    };
                    let (previous, ids) = (ids(previous), ids(params));
                    self.rack_layout.follow(&previous, &ids);
                    if let Some(hidden) = &mut self.hidden {
                        hidden.follow(&previous, &ids);
                    }
                }
            }
            self.beat_light = beat_light(self.snapshot.status.as_ref(), snapshot.status.as_ref());
            self.snapshot = snapshot;
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the window's panels laid out in drawing order"
    )]
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let snapshot = &self.snapshot;
        let now = Instant::now();
        let rows = snapshot
            .bank
            .as_ref()
            .map(|list| bank_rows_for_rack(bank_rows(list).0, self.mixer.shows_stereo_rack()))
            .unwrap_or_default();
        let main_view = snapshot
            .status
            .as_ref()
            .filter(|status| project_name(Some(status)).is_some())
            .map(|_| self.views.shown);
        let esc_taken = esc_taken(ui.ctx());
        let arrange = (main_view == Some(MainView::Arrange)).then_some(&self.project_view);
        let item_ids: Vec<u64> = snapshot
            .items
            .as_ref()
            .map(item_blocks)
            .unwrap_or_default()
            .iter()
            .map(|item| item.id)
            .collect();
        let mut commands: Vec<Command> = pressed_keys(ui, &item_ids)
            .into_iter()
            .filter_map(|key| key_command(key, snapshot, &rows, arrange))
            .collect();
        commands.extend(self.throttle.tick(now));
        Panel::bottom("status").show(ui, |ui| {
            let undo = (&mut self.removed, &mut self.hidden, &mut self.rack_layout);
            commands.extend(footer(ui, snapshot, &self.feedback, undo, now));
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
            beat_light: self.beat_light,
            layout: &mut self.rack_layout,
        };
        let sidebar_frame = Frame::side_top_panel(ui.style())
            .fill(colour(palette().case))
            .inner_margin(SIDEBAR_MARGIN);
        Panel::left("sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .frame(sidebar_frame)
            .show(ui, |ui| commands.extend(sidebar(ui, snapshot, &mut panel)));
        if let Some(main_view) = main_view {
            let feedback = (&mut self.feedback, now);
            commands.extend(project_panels(
                ui,
                snapshot,
                (&mut self.project_view, main_view),
                feedback,
            ));
        } else {
            self.project_view = ProjectView::new(self.project_view.snap);
            self.mixer = MixerView::default();
        }
        let dialog_open = self.mixer.render.is_some()
            || self.autosave_prompt.is_some()
            || self.clean_takes.is_some();
        let has_selection =
            main_view == Some(MainView::Arrange) && !self.project_view.selection.is_empty();
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            if esc_clears_selection(esc_taken, dialog_open, has_selection) {
                self.project_view.selection.clear();
            } else if esc_dismisses_toast(esc_taken.0, esc_taken.1, dialog_open) {
                self.feedback.dismiss();
            }
        }
        error_toast(ui.ctx(), &mut self.feedback);
        commands.extend(open_render_dialog(ui.ctx(), snapshot, &mut self.mixer));
        if let Some(view) = &self.clean_takes {
            let (dialog_commands, open) = clean_takes_dialog(ui.ctx(), snapshot, view);
            commands.extend(dialog_commands);
            if !open {
                self.clean_takes = None;
            }
        }
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
            tuner: &mut tuner,
            clips: &mut self.clips,
            holds: &self.holds,
            open_units: &mut self.open_units,
            layout: &mut self.rack_layout,
            hidden: &mut self.hidden,
        };
        let hidden_before = view.hidden.clone();
        let central = CentralPanel::default()
            .frame(Frame::central_panel(ui.style()).inner_margin(central_margin(main_view)))
            .show(ui, |ui| {
                central_view(ui, snapshot, main_view, &mut self.mixer, &mut view)
            });
        commands.extend(central.inner);
        if self.hidden.is_some() && self.hidden != hidden_before {
            self.removed = None;
        }
        widgets::paint_focus_ring(ui.ctx());
        let commands = self.keep_window_commands(commands);
        if let Some(Command::ItemSplit { at, .. }) = commands
            .iter()
            .rfind(|command| matches!(command, Command::ItemSplit { .. }))
        {
            self.project_view.split_at = Some(*at);
        }
        if commands.iter().any(|command| {
            matches!(
                command,
                Command::ItemDuplicate(_) | Command::ItemPaste { .. }
            )
        }) {
            self.project_view.select_new = true;
        }
        if let Some(Command::ItemCopy(ids)) = commands
            .iter()
            .rfind(|command| matches!(command, Command::ItemCopy(_)))
        {
            let items = self
                .snapshot
                .items
                .as_ref()
                .map(item_blocks)
                .unwrap_or_default();
            ui.ctx().copy_text(copied_text(&items, ids));
        }
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
        storage.set_string(VIEW_KEY, self.views.shown.stored().to_string());
        storage.set_string(RACK_LAYOUT_KEY, self.rack_layout.to_stored());
        let snap = if self.project_view.snap { "on" } else { "off" };
        storage.set_string(SNAP_KEY, snap.to_string());
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

/// The Metronome's M and T first, then while a Project is open the Item keys in Arrange (`arrange`) and the Project view's keys, else the Preset keys.
fn key_command(
    key: KeyEvent,
    snapshot: &Snapshot,
    rows: &[BankRow],
    arrange: Option<&ProjectView>,
) -> Option<Command> {
    let Some(status) = &snapshot.status else {
        return key_to_command(key, rows);
    };
    if let Some(command) = metronome_key_to_command(key, status) {
        return Some(command);
    }
    if project_name(Some(status)).is_none() {
        return key_to_command(key, rows);
    }
    let status = &to_payload(status);
    let items = snapshot.items.as_ref().map(item_blocks).unwrap_or_default();
    if let Some(view) = arrange
        && let Some(command) =
            item_key_to_command(key, status, &items, &view.selection, view.timeline)
    {
        return Some(command);
    }
    project_key_to_command(key, status, &items)
}

/// The keys [`key_active`] lets through. Keys match the key event's own modifiers: an injected or fast press releases Ctrl within the same frame, so the frame's modifier state reads none.
fn pressed_keys(ui: &Ui, item_ids: &[u64]) -> Vec<KeyEvent> {
    let text_edit_focused = ui.ctx().text_edit_focused();
    let focus = match ui.ctx().memory(egui::Memory::focused) {
        _ if text_edit_focused => KeyFocus::TextField,
        None => KeyFocus::Nothing,
        Some(id) if item_ids.iter().any(|&item| item_node_id(item) == id) => KeyFocus::Item,
        Some(_) => KeyFocus::Widget,
    };
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
                Event::Copy => Some(KeyEvent::CtrlC),
                Event::Paste(_) | Event::PasteImage(_) => Some(KeyEvent::CtrlV),
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
        if text_edit_focused {
            return keys;
        }
        if input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) {
            keys.push(KeyEvent::CtrlShiftZ);
        }
        if input.consume_key(Modifiers::COMMAND, Key::Z) {
            keys.push(KeyEvent::CtrlZ);
        }
        if input.consume_key(Modifiers::COMMAND, Key::D) {
            keys.push(KeyEvent::CtrlD);
        }
        if input.consume_key(Modifiers::COMMAND, Key::C) {
            keys.push(KeyEvent::CtrlC);
        }
        if input.consume_key(Modifiers::COMMAND, Key::V) {
            keys.push(KeyEvent::CtrlV);
        }
        keys
    });
    keys.into_iter()
        .filter(|&key| key_active(key, focus))
        .collect()
}

/// Digits with their modifiers; Alt+↑ and Alt+↓; Space, R, Home, End, M, T, S, Delete or Backspace, ← and → only without a modifier.
fn key_event(key: Key, modifiers: Modifiers) -> Option<KeyEvent> {
    if let Some((digit, _)) = (1..)
        .zip(DIGIT_KEYS)
        .find(|(_, digit_key)| *digit_key == key)
    {
        return Some(KeyEvent::Digit(
            digit,
            KeyModifiers {
                ctrl: modifiers.ctrl || modifiers.command,
                alt: modifiers.alt,
            },
        ));
    }
    if modifiers == Modifiers::ALT {
        return match key {
            Key::ArrowUp => Some(KeyEvent::AltUp),
            Key::ArrowDown => Some(KeyEvent::AltDown),
            _ => None,
        };
    }
    if !modifiers.is_none() {
        return None;
    }
    match key {
        Key::Space => Some(KeyEvent::Space),
        Key::R => Some(KeyEvent::R),
        Key::Home => Some(KeyEvent::Home),
        Key::End => Some(KeyEvent::End),
        Key::M => Some(KeyEvent::M),
        Key::T => Some(KeyEvent::T),
        Key::S => Some(KeyEvent::S),
        Key::Delete | Key::Backspace => Some(KeyEvent::Delete),
        Key::ArrowLeft => Some(KeyEvent::Left),
        Key::ArrowRight => Some(KeyEvent::Right),
        _ => None,
    }
}

/// The transport bar, then in Arrange the Tracks with their timeline above the Rack, over a split that leaves the Rack at least [`RACK_MIN_HEIGHT`].
fn project_panels(
    ui: &mut Ui,
    snapshot: &Snapshot,
    (view, main_view): (&mut ProjectView, MainView),
    feedback: (&mut Feedback, Instant),
) -> Vec<Command> {
    let mut commands = Vec::new();
    Panel::top("transport")
        .frame(
            Frame::new()
                .fill(colour(palette().case))
                .inner_margin(TRANSPORT_MARGIN),
        )
        .show(ui, |ui| {
            commands.extend(transport_bar(ui, snapshot, main_view, feedback));
        });
    if let Some(transport) = snapshot
        .status
        .as_ref()
        .map(to_payload)
        .as_ref()
        .and_then(transport_view)
    {
        view.recording_from = recording_from(view.recording_from, &transport);
    }
    if main_view != MainView::Arrange {
        return commands;
    }
    let below = ui.available_height();
    let max_height = (below - RACK_MIN_HEIGHT).max(0.0);
    Panel::top("timeline")
        .resizable(true)
        .default_size(TIMELINE_HEIGHT.min(below * TIMELINE_SHARE))
        .size_range(0.0..=max_height)
        .frame(
            Frame::new()
                .fill(colour(palette().case))
                .inner_margin(TIMELINE_MARGIN),
        )
        .show(ui, |ui| commands.extend(tracks_view(ui, snapshot, view)));
    commands
}

/// Narrower above and below in the mixer, so that its strips take the height.
fn central_margin(main_view: Option<MainView>) -> Margin {
    if main_view == Some(MainView::Mixer) {
        MIXER_MARGIN
    } else {
        RACK_MARGIN
    }
}

/// The Rack alone with no Project open; with one, the mixer panel in the Mixer view, else the shown Rack ([`shown_rack_column`]) under its heading.
fn central_view(
    ui: &mut Ui,
    snapshot: &Snapshot,
    main_view: Option<MainView>,
    mixer: &mut MixerView,
    view: &mut RackView,
) -> Vec<Command> {
    let (Some(main_view), Some(status)) = (main_view, snapshot.status.as_ref()) else {
        return rack_view(ui, snapshot, view);
    };
    if main_view == MainView::Mixer {
        return mixer_panel(ui, snapshot, mixer);
    }
    let status = &to_payload(status);
    let Some(target) = shown_rack_column(mixer.rack, status) else {
        return Vec::new();
    };
    rack_heading(ui, status, target);
    rack_view(ui, snapshot, view)
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

/// Names the Track, Bus or Master whose Rack is shown.
fn rack_heading(ui: &mut Ui, status: &Value, target: MixerTarget) {
    let name = match target {
        MixerTarget::Track(index) => status["tracks"][index]["name"].as_str(),
        MixerTarget::Bus(index) => status["buses"][index]["name"].as_str(),
        MixerTarget::Master => Some(MASTER),
    };
    let heading = format!("{} — Rack", name.unwrap_or_default());
    ui.label(RichText::new(heading).font(theme::heading()));
}

fn revision(snapshot: &Snapshot) -> u64 {
    snapshot.status.as_ref().map_or(0, |status| status.revision)
}

/// Whether a focused widget or an open menu takes this frame's Esc, read before the frame's widgets act on it.
fn esc_taken(ctx: &egui::Context) -> (bool, bool) {
    (
        ctx.memory(|memory| memory.focused().is_some()),
        egui::Popup::is_any_open(ctx),
    )
}
