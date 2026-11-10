use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use egui::{CentralPanel, Event, Frame, Key, Margin, Modifiers, Panel, Theme, Ui};
use serde_json::Value;
use tracing::{error, info};
use xguitare_core::{BankKind, Config};

use crate::client::{self, TICK};
use crate::model::{
    ClipLatch, Command, Feedback, KeyEvent, KeyModifiers, Link, PeakHold, PortSelection,
    RemovedUnit, SET_INTERVAL, SettingsDraft, Snapshot, Throttle, bank_kind, bank_rows,
    forget_removed_units, key_to_command, open_units_from_string, open_units_to_string, palette,
    param_rows, peak_db, quit_commands, remember_removes, shortcuts_active,
};
use crate::rack::{Knobs, METERS, RackView, rack_view};
use crate::sidebar::{AddDraft, SidebarView, Tuner, footer, sidebar};
use crate::widgets;

const SIDEBAR_WIDTH: f32 = 260.0;
const SIDEBAR_MARGIN: Margin = Margin::symmetric(20, 24);
const RACK_MARGIN: Margin = Margin::symmetric(8, 24);
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
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        config: Config,
        config_path: Option<PathBuf>,
        stop: Arc<AtomicBool>,
    ) -> App {
        let ctx = cc.egui_ctx.clone();
        let (commands, snapshots, client) =
            client::spawn(config.clone(), config_path, Arc::clone(&stop), move || {
                ctx.request_repaint()
            });
        App::assemble(cc, &config, commands, snapshots, Some(client), stop)
    }

    /// An App with no client thread: `snapshot` arrives as the only client update and its commands go to `commands`.
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
                module_types: Vec::new(),
                irs: Vec::new(),
                last_message: None,
                failed_set: None,
                refused_remove: None,
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
        for command in commands {
            match command {
                Command::RackRemove { id } => {
                    let status = self.snapshot.status.as_ref();
                    let removed = self
                        .snapshot
                        .params
                        .as_ref()
                        .and_then(|params| RemovedUnit::new(params, status, id, now));
                    if removed.is_some() {
                        self.removed = removed;
                    }
                }
                Command::Load(_)
                | Command::SetSettings {
                    bank_kind: Some(_), ..
                } => self.removed = None,
                _ => {}
            }
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
        let mut commands: Vec<Command> = pressed_keys(ui)
            .into_iter()
            .filter_map(|key| key_to_command(key, &rows))
            .collect();
        commands.extend(self.throttle.tick(now));
        Panel::bottom("status").show(ui, |ui| {
            commands.extend(footer(ui, snapshot, &self.feedback, &mut self.removed, now))
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
            tuner: &mut tuner,
            ports: &mut self.ports_draft,
            settings: &mut self.settings_draft,
        };
        let sidebar_frame = Frame::side_top_panel(ui.style())
            .fill(widgets::colour(palette().sidebar))
            .inner_margin(SIDEBAR_MARGIN);
        Panel::left("sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .frame(sidebar_frame)
            .show(ui, |ui| commands.extend(sidebar(ui, snapshot, &mut panel)));
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
            .show(ui, |ui| commands.extend(rack_view(ui, snapshot, &mut view)));
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

/// No shortcut while a text field takes typing. Keys match the key event's own modifiers: an injected or fast press releases Ctrl within the same frame, so the frame's modifier state reads none.
fn pressed_keys(ui: &Ui) -> Vec<KeyEvent> {
    if !shortcuts_active(ui.ctx().text_edit_focused()) {
        return Vec::new();
    }
    ui.input_mut(|input| {
        let mut keys: Vec<KeyEvent> = input
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => (1..)
                    .zip(DIGIT_KEYS)
                    .find(|(_, digit_key)| digit_key == key)
                    .map(|(digit, _)| {
                        KeyEvent::Digit(
                            digit,
                            KeyModifiers {
                                ctrl: modifiers.ctrl,
                                alt: modifiers.alt,
                            },
                        )
                    }),
                _ => None,
            })
            .collect();
        if input.consume_key(Modifiers::COMMAND, Key::S) {
            keys.push(KeyEvent::CtrlS);
        }
        keys
    })
}

fn revision(snapshot: &Snapshot) -> u64 {
    snapshot
        .status
        .as_ref()
        .and_then(|status| status["revision"].as_u64())
        .unwrap_or_default()
}
