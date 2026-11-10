use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};

use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Role, TouchPhase,
    Vec2, pos2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_system_font_provider::SystemFontProvider;
use serde_json::{Value, json};
use xguitare_core::control::TrackRef;
use xguitare_core::control::TransportAction;
use xguitare_core::project::LoopRegion;
use xguitare_core::render::{RenderBits, RenderRange};
use xguitare_core::{BankKind, Config};
use xguitare_gui::app::App;
use xguitare_gui::mixer_model::{MixerTarget, ShownRack};
use xguitare_gui::model::{Command, Link, ModuleType, Restore, Snapshot, TrackEdit};
use xguitare_gui::project_model::AutosaveOffer;

const WINDOW: Vec2 = Vec2::new(1440.0, 900.0);
const DEFAULT_WINDOW: Vec2 = Vec2::new(1280.0, 800.0);
/// Tall enough for the mixer columns to show whole above the Rack.
const TALL_WINDOW: Vec2 = Vec2::new(1440.0, 1200.0);
const MIN_WINDOW: Vec2 = Vec2::new(1024.0, 640.0);
const CAPTURE: &str = "Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO";
const LEFT: &str = "Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL";
const RIGHT: &str = "Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR";

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn bass_config() -> (Config, TempDir) {
    static HARNESSES: AtomicUsize = AtomicUsize::new(0);
    let dir = TempDir(PathBuf::from(format!(
        "/tmp/xguitare-snap-{}-{}",
        std::process::id(),
        HARNESSES.fetch_add(1, Ordering::Relaxed)
    )));
    let _ = fs::remove_dir_all(&dir.0);
    fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("config.toml");
    fs::write(&path, "bank_kind = \"bass\"\n").unwrap();
    (Config::load(Some(&path)).unwrap(), dir)
}

fn param(module: &str, name: &str, value: f64, spec: (f64, f64, f64, &str, bool)) -> Value {
    let (min, max, default, unit, integer) = spec;
    json!({"module": module, "type": module, "param": name, "value": value, "min": min,
        "max": max, "default": default, "unit": unit, "integer": integer})
}

fn funk_params() -> Value {
    let mut params = vec![
        param(
            "compressor",
            "threshold",
            -24.0,
            (-60.0, 0.0, -20.0, "dB", false),
        ),
        param("compressor", "ratio", 6.0, (1.0, 20.0, 4.0, "", false)),
        param("compressor", "attack", 5.0, (0.1, 100.0, 10.0, "ms", false)),
        param(
            "compressor",
            "release",
            80.0,
            (10.0, 1000.0, 100.0, "ms", false),
        ),
        param("compressor", "makeup", 3.0, (0.0, 24.0, 0.0, "dB", false)),
        param("preamp", "gain", 15.0, (0.0, 100.0, 40.0, "%", false)),
        param("preamp", "bias", 0.0, (-1.0, 1.0, 0.0, "", false)),
        param("preamp", "master", -8.0, (-60.0, 0.0, -12.0, "dB", false)),
        param("preamp", "stages", 2.0, (1.0, 4.0, 3.0, "", true)),
        param("tonestack", "bass", 55.0, (0.0, 100.0, 50.0, "%", false)),
        param("tonestack", "middle", 60.0, (0.0, 100.0, 50.0, "%", false)),
        param("tonestack", "treble", 55.0, (0.0, 100.0, 50.0, "%", false)),
        param("cabinet", "level", 12.0, (-20.0, 20.0, 0.0, "dB", false)),
        param("cabinet", "low_cut", 20.0, (20.0, 300.0, 20.0, "Hz", true)),
        param(
            "cabinet",
            "high_cut",
            20000.0,
            (2000.0, 20000.0, 20000.0, "Hz", true),
        ),
        param("chorus", "rate", 0.8, (0.1, 5.0, 1.0, "Hz", false)),
        param("chorus", "depth", 40.0, (0.0, 100.0, 50.0, "%", false)),
        param("chorus", "mix", 20.0, (0.0, 100.0, 40.0, "%", false)),
        param("delay-2", "time", 380.0, (10.0, 2000.0, 400.0, "ms", false)),
        param("delay-2", "feedback", 30.0, (0.0, 95.0, 35.0, "%", false)),
        param("delay-2", "mix", 20.0, (0.0, 100.0, 25.0, "%", false)),
    ];
    params
        .iter_mut()
        .filter(|param| param["module"] == "cabinet")
        .for_each(|param| param["ir"] = json!("bass-cab.wav"));
    params
        .iter_mut()
        .filter(|param| param["module"] == "delay-2")
        .for_each(|param| param["type"] = json!("delay"));
    json!({ "params": params })
}

fn funk_snapshot() -> Snapshot {
    let names = [
        "Blues",
        "Funk",
        "Primus Slap",
        "Primus Fuzz",
        "Primus Flange",
    ];
    let entries: Vec<Value> = (1..)
        .zip(names)
        .map(|(program, name)| json!({"program": program, "name": name, "valid": true}))
        .collect();
    let module_types = [
        ("gain", "mono"),
        ("gate", "mono"),
        ("compressor", "mono"),
        ("drive", "mono"),
        ("fuzz", "mono"),
        ("preamp", "mono"),
        ("tonestack", "mono"),
        ("cabinet", "mono"),
        ("delay", "stereo"),
        ("chorus", "mono_to_stereo"),
        ("flanger", "mono_to_stereo"),
        ("reverb", "stereo"),
    ]
    .into_iter()
    .map(|(type_name, channels)| ModuleType {
        type_name: type_name.to_string(),
        channels: channels.to_string(),
    })
    .collect();
    Snapshot {
        link: Link::Connected,
        status: Some(json!({
            "preset": "Funk", "program": 2, "sample_rate": 48000, "buffer_size": 128,
            "revision": 1, "muted": false, "modified": false,
            "input_peak_db": -14.0, "output_peak_db": -6.0, "xruns": 0,
        })),
        bank: Some(json!({"active": 2, "entries": entries})),
        params: Some(funk_params()),
        tuner: Some(json!({"confident": true, "note": "A1", "hz": 55.1, "cents": 4})),
        ports: Some(json!({
            "capture": [CAPTURE, "Scarlett Solo 4th Gen Input 1 Inst/Line:capture_FL"],
            "playback": [LEFT, RIGHT],
            "connected": {"input": CAPTURE, "output_left": LEFT, "output_right": RIGHT},
        })),
        settings: Some(json!({
            "bank_kind": "bass",
            "bank_dir": "/home/you/.config/xguitare/bank-bass",
            "ir_dir": "/home/you/.config/xguitare/bank-bass",
            "projects_dir": "/home/you/Music/xguitare",
        })),
        projects: Some(json!({"projects": [
            {"folder": "demo-song", "name": "Demo Song", "valid": true},
            {"folder": "live", "name": "Live", "valid": true},
        ]})),
        module_types,
        irs: vec!["bass-cab.wav".to_string(), "guitar-cab.wav".to_string()],
        last_message: None,
        failed_set: None,
        refused_remove: None,
        refused_render: None,
        items: None,
        autosave_offer: None,
    }
}

fn bypassed_snapshot(modules: &[&str]) -> Snapshot {
    let mut snapshot = funk_snapshot();
    let params = snapshot.params.as_mut().unwrap()["params"]
        .as_array_mut()
        .unwrap();
    for param in params {
        let bypass = modules.contains(&param["module"].as_str().unwrap());
        param["bypass"] = json!(bypass);
    }
    snapshot
}

fn muted_snapshot() -> Snapshot {
    let mut snapshot = funk_snapshot();
    snapshot.status.as_mut().unwrap()["muted"] = json!(true);
    snapshot
}

/// Three Tracks at 120 bpm in 4/4: Guitar armed with two Takes on one Item, Bass muted with one Item, Keys empty; the playhead in bar 3, the loop on over bars 2 to 4.
fn project_snapshot(transport: &str) -> Snapshot {
    let mut snapshot = funk_snapshot();
    let status = snapshot.status.as_mut().unwrap();
    let count_in_remaining = if transport == "recording" { 3 } else { 0 };
    let project = json!({
        "project": "Demo Song", "transport": transport, "selected": 0, "disk_overflows": 0,
        "count_in_remaining": count_in_remaining, "loaded_audio_bytes": 61_440_000,
        "position": {"frames": 230_400, "bars_beats": "3.2"},
        "tempo": {"bpm": 120.0, "meter": [4, 4]},
        "loop": {"start": 96_000, "end": 384_000, "on": true},
        "click": {"on": true, "count_in": 1},
        "tracks": [
            {"name": "Guitar", "input": CAPTURE, "volume_db": -3.0, "pan": -0.25,
             "mute": false, "solo": false, "armed": true, "input_peak_db": -14.0},
            {"name": "Bass", "input": null, "volume_db": 0.0, "pan": 0.0,
             "mute": true, "solo": false, "armed": false, "input_peak_db": -120.0},
            {"name": "Keys", "input": null, "volume_db": -6.0, "pan": 0.5,
             "mute": false, "solo": false, "armed": false, "input_peak_db": -120.0},
        ],
    });
    for (key, value) in project.as_object().unwrap() {
        status[key] = value.clone();
    }
    snapshot.items = Some(json!({"sample_rate": 48000, "tracks": [
        {"id": 0, "name": "Guitar", "items": [{"position": 96_000, "bars_beats": "2.1",
         "offset": 0, "length": 288_000, "take": 1,
         "takes": ["takes/guitar-1.wav", "takes/guitar-2.wav"]}]},
        {"id": 1, "name": "Bass", "items": [{"position": 0, "bars_beats": "1.1", "offset": 0,
         "length": 480_000, "take": 0, "takes": ["takes/bass-1.wav"]}]},
        {"id": 2, "name": "Keys", "items": []},
    ]}));
    snapshot
}

/// The Project with two Buses: Guitar clipped and sending pre-fader to Verb, Bass muted and sending to Delay, Keys not to Master and sending to Verb, Verb soloed.
fn mixer_snapshot() -> Snapshot {
    let mut snapshot = project_snapshot("playing");
    let status = snapshot.status.as_mut().unwrap();
    let mixer = [
        (
            json!([{"bus": 0, "level_db": -6.0, "pre": true}]),
            [-2.1, 0.0],
            true,
            true,
        ),
        (
            json!([{"bus": 1, "level_db": -12.0, "pre": false}]),
            [-120.0, -120.0],
            false,
            true,
        ),
        (
            json!([{"bus": 0, "level_db": -3.0, "pre": false}]),
            [-18.4, -12.7],
            false,
            false,
        ),
    ];
    for (track, (sends, peak, clip, to_master)) in mixer.into_iter().enumerate() {
        let entry = &mut status["tracks"][track];
        entry["sends"] = sends;
        entry["peak_db"] = json!(peak);
        entry["clip"] = json!(clip);
        entry["to_master"] = json!(to_master);
    }
    status["buses"] = json!([
        {"id": 0, "name": "Verb", "volume_db": -4.0, "pan": 0.0, "mute": false, "solo": true,
         "peak_db": [-14.5, -15.2], "clip": false},
        {"id": 1, "name": "Delay", "volume_db": 0.0, "pan": 0.3, "mute": false, "solo": false,
         "peak_db": [-120.0, -120.0], "clip": false},
    ]);
    status["master"] = json!({"volume_db": -1.5, "peak_db": [-6.3, -5.9], "clip": false});
    status["render"] = json!(null);
    snapshot.params = Some(json!({"params": [
        param("reverb", "size", 70.0, (0.0, 100.0, 50.0, "%", false)),
        param("reverb", "damping", 40.0, (0.0, 100.0, 50.0, "%", false)),
        param("reverb", "mix", 100.0, (0.0, 100.0, 30.0, "%", false)),
    ]}));
    snapshot
}

/// [`mixer_snapshot`] with eight Buses, Guitar sending to each.
fn eight_buses_snapshot() -> Snapshot {
    let mut snapshot = mixer_snapshot();
    let status = snapshot.status.as_mut().unwrap();
    let names = [
        "Verb", "Delay", "Room", "Plate", "Echo", "Chorus", "Crush", "Sub",
    ];
    status["buses"] = names
        .iter()
        .enumerate()
        .map(|(id, name)| {
            json!({"id": id, "name": name, "volume_db": 0.0, "pan": 0.0, "mute": false,
                "solo": false, "peak_db": [-120.0, -120.0], "clip": false})
        })
        .collect();
    status["tracks"][0]["sends"] = (0..names.len())
        .map(|bus| json!({"bus": bus, "level_db": -6.0, "pre": false}))
        .collect();
    snapshot
}

fn render_snapshot(render: Value) -> Snapshot {
    let mut snapshot = mixer_snapshot();
    snapshot.status.as_mut().unwrap()["render"] = render;
    snapshot
}

fn click_button(harness: &mut Harness<App>, label: &str) {
    harness.get_by_role_and_label(Role::Button, label).click();
    harness.run_steps(2);
}

fn project_harness(transport: &str) -> (Harness<'static, App>, Receiver<Command>, TempDir) {
    harness_with(WINDOW, project_snapshot(transport))
}

fn click_unit_switch(harness: &mut Harness<App>, unit: &str, label: &str) {
    let unit_rect = harness.get_by_label(&format!("{unit} unit")).rect();
    harness
        .get_all_by_role_and_label(Role::Button, label)
        .find(|switch| unit_rect.contains_rect(switch.rect()))
        .unwrap_or_else(|| panic!("no {label} switch in the {unit} unit"))
        .click();
    harness.run_steps(2);
}

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .unwrap()
        .join("gui-snapshots")
}

fn write_png(harness: &mut Harness<App>, name: &str) {
    harness.run_steps(2);
    harness.check_accessibility();
    let image = harness.render().unwrap();
    let dir = snapshot_dir();
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.png"));
    image.save(&path).unwrap();
    assert!(path.exists(), "{} was not written", path.display());
}

fn harness() -> (Harness<'static, App>, Receiver<Command>, TempDir) {
    harness_sized(WINDOW)
}

fn harness_sized(size: Vec2) -> (Harness<'static, App>, Receiver<Command>, TempDir) {
    harness_with(size, funk_snapshot())
}

fn harness_with(
    size: Vec2,
    snapshot: Snapshot,
) -> (Harness<'static, App>, Receiver<Command>, TempDir) {
    let (config, dir) = bass_config();
    let (commands, sent) = mpsc::channel();
    let mut harness = Harness::builder()
        .with_size(size)
        .with_context_setup(|ctx| ctx.add_font_provider(Arc::new(SystemFontProvider::new())))
        .wgpu()
        .build_eframe(move |cc| App::with_snapshot(cc, &config, snapshot, commands));
    harness.run_steps(2);
    (harness, sent, dir)
}

fn sent_now(sent: &Receiver<Command>) -> Vec<Command> {
    sent.try_iter().collect()
}

fn unit_height(harness: &Harness<App>, unit: &str) -> f32 {
    harness.get_by_label(unit).rect().height()
}

fn open_unit(harness: &mut Harness<App>, nth: usize) {
    harness
        .get_all_by_label(CLOSED)
        .nth(nth)
        .unwrap()
        .click_accesskit();
    harness.run_steps(2);
}

fn edit_value(harness: &mut Harness<App>, param: &str, text: &str) {
    harness
        .get_by_role_and_label(Role::TextInput, param)
        .focus();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness
        .get_by_role_and_label(Role::TextInput, param)
        .type_text(text);
    harness.run_steps(2);
}

const CLOSED: &str = "▶";
const DRAG_STEPS: usize = 6;

fn press_and_drag(harness: &mut Harness<App>, from: Pos2, to: Pos2) {
    harness.event(Event::PointerMoved(from));
    harness.run_steps(1);
    pointer_button(harness, from, true);
    for step in 1..=DRAG_STEPS {
        let t = step as f32 / DRAG_STEPS as f32;
        harness.event(Event::PointerMoved(from + (to - from) * t));
        harness.run_steps(1);
    }
}

fn pointer_button(harness: &mut Harness<App>, pos: Pos2, pressed: bool) {
    harness.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(1);
}

fn drag_and_release(harness: &mut Harness<App>, from: Pos2, to: Pos2) {
    press_and_drag(harness, from, to);
    pointer_button(harness, to, false);
    harness.run_steps(2);
}

/// A point on the unit's name, which drags the unit by its header.
fn unit_header_point(harness: &Harness<App>, unit: &str) -> Pos2 {
    let rect = harness.get_by_label(&format!("{unit} unit")).rect();
    pos2(rect.left() + 130.0, rect.center().y)
}

fn unit_top(harness: &Harness<App>, unit: &str) -> f32 {
    harness.get_by_label(&format!("{unit} unit")).rect().top()
}

fn tile_centre(harness: &Harness<App>, label: &str) -> Pos2 {
    harness
        .get_by_role_and_label(Role::Button, label)
        .rect()
        .center()
}

#[test]
fn gui_snapshots() {
    write_png(&mut harness().0, "default");

    let (mut two_open, _, _dir) = harness();
    open_unit(&mut two_open, 0);
    open_unit(&mut two_open, 0);
    assert_eq!(two_open.get_all_by_label("▼").count(), 2);
    write_png(&mut two_open, "two-units-open");

    let (mut tuner, _, _dir) = harness();
    tuner.get_by_role_and_label(Role::Button, "Tuner").click();
    write_png(&mut tuner, "tuner-open");

    let (mut save_as, _, _dir) = harness();
    save_as.get_by_label("Save as new").click();
    write_png(&mut save_as, "save-as-open");

    let (mut sections, _, _dir) = harness();
    for title in ["Audio", "Settings"] {
        sections.get_by_label(title).click();
        sections.run_steps(2);
    }
    write_png(&mut sections, "left-panel-open");

    let (mut editing, _, _dir) = harness();
    edit_value(&mut editing, "Threshold", "-30");
    write_png(&mut editing, "value-editing");

    write_png(&mut harness_sized(MIN_WINDOW).0, "minimum-window");
    write_png(&mut harness_sized(DEFAULT_WINDOW).0, "default-window");

    let (mut bypassed, _, _dir) =
        harness_with(WINDOW, bypassed_snapshot(&["compressor", "chorus"]));
    open_unit(&mut bypassed, 0);
    write_png(&mut bypassed, "units-bypassed");

    write_png(&mut harness_with(WINDOW, muted_snapshot()).0, "mute-banner");

    let (mut switch_focus, _, _dir) = harness();
    let unit_rect = switch_focus.get_by_label("compressor unit").rect();
    switch_focus
        .get_all_by_role_and_label(Role::Button, "On")
        .find(|switch| unit_rect.contains_rect(switch.rect()))
        .unwrap()
        .focus();
    write_png(&mut switch_focus, "switch-focused");

    let (mut header_drag, _, _dir) = harness();
    let from = unit_header_point(&header_drag, "chorus");
    let to = pos2(from.x, unit_top(&header_drag, "preamp") + 10.0);
    press_and_drag(&mut header_drag, from, to);
    write_png(&mut header_drag, "header-drag");

    let (mut palette_drag, _, _dir) = harness();
    let from = tile_centre(&palette_drag, "gate (mono)");
    let to = pos2(800.0, unit_top(&palette_drag, "preamp") + 10.0);
    press_and_drag(&mut palette_drag, from, to);
    write_png(&mut palette_drag, "palette-drag");

    let (mut undo, _, _dir) = harness();
    let from = unit_header_point(&undo, "chorus");
    drag_and_release(&mut undo, from, pos2(100.0, 400.0));
    write_png(&mut undo, "undo-footer");
}

#[test]
fn project_snapshots() {
    write_png(&mut project_harness("playing").0, "project");

    let (mut recording, _, _dir) = project_harness("recording");
    write_png(&mut recording, "project-recording");

    let (mut section, _, _dir) = harness();
    section.get_by_label("Open a Project").click();
    section.run_steps(2);
    write_png(&mut section, "project-section");

    let mut offered = project_snapshot("stopped");
    offered.autosave_offer = Some((
        AutosaveOffer {
            folder: "demo-song".to_string(),
            name: "Demo Song".to_string(),
        },
        std::time::Instant::now(),
    ));
    write_png(&mut harness_with(WINDOW, offered).0, "restore-prompt");
}

#[test]
fn project_window_has_transport_tracks_and_the_rack() {
    let (harness, _, _dir) = project_harness("stopped");
    for label in [
        "▶ Play",
        "■ Stop",
        "● Record",
        "Loop",
        "Click",
        "+ Add Track",
    ] {
        harness.get_by_role_and_label(Role::Button, label);
    }
    for track in ["Guitar", "Bass", "Keys"] {
        harness.get_by_label(&format!("Track {track}"));
    }
    harness.get_by_label("compressor unit");
}

#[test]
fn project_keys_do_nothing_without_a_project() {
    let (mut harness, sent, _dir) = harness();
    for key in [Key::Space, Key::R, Key::Home, Key::End] {
        harness.key_press(key);
        harness.run_steps(1);
    }
    harness.key_press_modifiers(Modifiers::COMMAND, Key::R);
    harness.run_steps(1);
    harness.key_press(Key::Num2);
    harness.run_steps(1);
    assert_eq!(sent_now(&sent), [Command::Load("1".to_string())]);
}

#[test]
fn undo_after_selecting_another_track_restores_to_the_first() {
    let (mut harness, sent, _dir) = project_harness("stopped");
    let from = unit_header_point(&harness, "compressor");
    drag_and_release(&mut harness, from, pos2(100.0, 600.0));
    harness.get_by_label("Track Bass").click();
    harness.run_steps(2);
    harness.get_by_role_and_label(Role::Button, "Undo").click();
    harness.run_steps(2);
    let sent = sent_now(&sent);
    assert_eq!(
        sent[..3],
        [
            Command::ShowRack(ShownRack::Track),
            Command::RackRemove {
                id: "compressor".to_string(),
            },
            Command::TrackSelect(1),
        ]
    );
    let Command::Restore(restore) = &sent[3] else {
        panic!("no Restore in {sent:?}");
    };
    assert_eq!(restore.track, Some(TrackRef::Name("Guitar".to_string())));
}

#[test]
fn no_project_shows_no_transport() {
    let (harness, _, _dir) = harness();
    assert!(harness.query_by_label("▶ Play").is_none());
    assert!(harness.query_by_label("Timeline ruler").is_none());
}

#[test]
fn track_switches_send_track_set() {
    let (mut harness, sent, _dir) = project_harness("stopped");
    for label in ["Arm Guitar", "Mute Bass", "Solo Keys"] {
        harness.get_by_label(label).click();
        harness.run_steps(2);
    }
    let edit = |track, edit| Command::TrackSet { track, edit };
    assert_eq!(
        sent_now(&sent),
        [
            edit(
                0,
                TrackEdit {
                    arm: Some(false),
                    ..TrackEdit::default()
                }
            ),
            edit(
                1,
                TrackEdit {
                    mute: Some(false),
                    ..TrackEdit::default()
                }
            ),
            edit(
                2,
                TrackEdit {
                    solo: Some(true),
                    ..TrackEdit::default()
                }
            ),
        ]
    );
}

#[test]
fn clicking_a_track_row_selects_it() {
    let (mut harness, sent, _dir) = project_harness("stopped");
    harness.get_by_label("Track Keys").click();
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), [Command::TrackSelect(2)]);
}

#[test]
fn transport_buttons_and_keys_send_transport() {
    let (mut harness, sent, _dir) = project_harness("stopped");
    harness
        .get_by_role_and_label(Role::Button, "● Record")
        .click();
    harness.run_steps(2);
    harness.key_press(Key::Space);
    harness.run_steps(1);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::R);
    harness.run_steps(1);
    harness.key_press(Key::Home);
    harness.run_steps(1);
    harness.key_press(Key::Num1);
    harness.run_steps(1);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(1);
    assert_eq!(
        sent_now(&sent),
        [
            Command::Transport(TransportAction::Record),
            Command::Transport(TransportAction::Play),
            Command::Transport(TransportAction::Record),
            Command::Locate(0),
            Command::ProjectSave,
        ]
    );
}

#[test]
fn loop_key_turns_the_loop_off() {
    let (mut harness, sent, _dir) = project_harness("stopped");
    harness.key_press(Key::R);
    harness.run_steps(1);
    assert_eq!(
        sent_now(&sent),
        [Command::Loop(LoopRegion {
            start: 96_000,
            end: 384_000,
            on: false,
        })]
    );
}

#[test]
fn clicking_the_ruler_locates() {
    let (mut harness, sent, _dir) = project_harness("stopped");
    let ruler = harness.get_by_label("Timeline ruler").rect();
    let at = pos2(ruler.left() + 48.0, ruler.center().y);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    pointer_button(&mut harness, at, true);
    pointer_button(&mut harness, at, false);
    harness.run_steps(1);
    assert_eq!(sent_now(&sent), [Command::Locate(23_040)]);
}

#[test]
fn dragging_the_loop_start_sends_the_snapped_region() {
    let (mut harness, sent, _dir) = project_harness("stopped");
    let start = harness.get_by_label("Loop start").rect().center();
    drag_and_release(&mut harness, start, start - Vec2::new(52.0, 0.0));
    assert_eq!(
        sent_now(&sent),
        [Command::Loop(LoopRegion {
            start: 72_000,
            end: 384_000,
            on: true,
        })]
    );
}

#[test]
fn restore_prompt_restores_the_autosave() {
    let mut offered = project_snapshot("stopped");
    offered.autosave_offer = Some((
        AutosaveOffer {
            folder: "demo-song".to_string(),
            name: "Demo Song".to_string(),
        },
        std::time::Instant::now(),
    ));
    let (mut harness, sent, _dir) = harness_with(WINDOW, offered);
    harness.get_by_label("Restore").click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::RestoreAutosave("demo-song".to_string())]
    );
    assert!(harness.query_by_label("Open saved").is_none());
}

#[test]
fn new_and_open_project_from_the_left_panel() {
    let (mut harness, sent, _dir) = harness();
    harness
        .get_by_role_and_label(Role::TextInput, "Name")
        .focus();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::TextInput, "Name")
        .type_text("Live");
    harness.run_steps(2);
    harness.get_by_role_and_label(Role::Button, "New").click();
    harness.run_steps(2);
    harness.get_by_label("Open a Project").click();
    harness.run_steps(2);
    harness.get_by_label("Demo Song").click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [
            Command::ProjectNew("Live".to_string()),
            Command::RefreshProjects,
            Command::ProjectOpen("demo-song".to_string()),
        ]
    );
}

#[test]
fn unit_dragged_by_its_header_moves_to_the_gap() {
    let (mut harness, sent, _dir) = harness();
    let from = unit_header_point(&harness, "chorus");
    let to = pos2(from.x, unit_top(&harness, "preamp") + 10.0);
    drag_and_release(&mut harness, from, to);
    assert_eq!(harness.query_all_by_label("▼").count(), 0);
    assert_eq!(
        sent_now(&sent),
        [Command::RackMove {
            id: "chorus".to_string(),
            to: 1,
        }]
    );
}

#[test]
fn unit_dragged_by_its_ear_moves_to_the_gap() {
    let (mut harness, sent, _dir) = harness();
    let unit = harness.get_by_label("compressor unit").rect();
    let from = pos2(unit.left() + 15.0, unit.center().y);
    let to = pos2(from.x, unit_top(&harness, "chorus") + 50.0);
    drag_and_release(&mut harness, from, to);
    assert_eq!(
        sent_now(&sent),
        [Command::RackMove {
            id: "compressor".to_string(),
            to: 4,
        }]
    );
}

#[test]
fn unit_dropped_outside_the_rack_is_removed_and_undo_restores_it() {
    let (mut harness, sent, _dir) = harness();
    let from = unit_header_point(&harness, "chorus");
    drag_and_release(&mut harness, from, pos2(100.0, 400.0));
    assert_eq!(
        sent_now(&sent),
        [Command::RackRemove {
            id: "chorus".to_string(),
        }]
    );
    harness.get_by_label("Removed chorus \u{2014}");
    harness.get_by_role_and_label(Role::Button, "Undo").click();
    harness.run_steps(2);
    let values = [("rate", 0.8), ("depth", 40.0), ("mix", 20.0)]
        .map(|(param, value)| (param.to_string(), value))
        .to_vec();
    assert_eq!(
        sent_now(&sent),
        [Command::Restore(Restore {
            type_name: "chorus".to_string(),
            after: Some("cabinet".to_string()),
            at: 4,
            values,
            bypass: false,
            ir: None,
            track: None,
            bus: None,
            master: false,
        })]
    );
    assert!(harness.query_by_label("Undo").is_none());
}

#[test]
fn loading_a_preset_drops_the_undo() {
    let (mut harness, sent, _dir) = harness();
    let from = unit_header_point(&harness, "chorus");
    drag_and_release(&mut harness, from, pos2(100.0, 400.0));
    harness.get_by_role_and_label(Role::Button, "Undo");
    harness.key_press(Key::Num2);
    harness.run_steps(2);
    assert!(harness.query_by_label("Undo").is_none());
    assert_eq!(
        sent_now(&sent),
        [
            Command::RackRemove {
                id: "chorus".to_string(),
            },
            Command::Load("1".to_string()),
        ]
    );
}

#[test]
fn only_a_bank_kind_change_in_settings_drops_the_undo() {
    let (mut harness, sent, _dir) = harness();
    let from = unit_header_point(&harness, "chorus");
    drag_and_release(&mut harness, from, pos2(100.0, 400.0));
    for title in ["Preset", "Bass Bank", "Settings"] {
        harness.get_by_label(title).click();
        harness.run_steps(2);
    }
    edit_value(&mut harness, "IR folder", "/home/you/irs");
    harness.get_by_label("Apply").click();
    harness.run_steps(2);
    harness.get_by_role_and_label(Role::Button, "Undo");
    harness.get_by_label("Guitar").click();
    harness.run_steps(2);
    harness.get_by_label("Apply").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Undo").is_none());
    assert_eq!(
        sent_now(&sent),
        [
            Command::RackRemove {
                id: "chorus".to_string(),
            },
            Command::SetSettings {
                bank_kind: None,
                ir_dir: Some(PathBuf::from("/home/you/irs")),
                projects_dir: None,
            },
            Command::SetSettings {
                bank_kind: Some(BankKind::Guitar),
                ir_dir: None,
                projects_dir: None,
            },
        ]
    );
}

#[test]
fn unit_released_below_the_rack_moves_to_the_end() {
    let (mut harness, sent, _dir) = harness();
    let from = unit_header_point(&harness, "compressor");
    drag_and_release(&mut harness, from, pos2(from.x, 700.0));
    assert_eq!(
        sent_now(&sent),
        [Command::RackMove {
            id: "compressor".to_string(),
            to: 5,
        }]
    );
}

#[test]
fn tile_dragged_into_the_rack_adds_at_the_gap() {
    let (mut harness, sent, _dir) = harness();
    let from = tile_centre(&harness, "gate (mono)");
    let to = pos2(800.0, unit_top(&harness, "preamp") + 10.0);
    drag_and_release(&mut harness, from, to);
    assert_eq!(
        sent_now(&sent),
        [Command::RackAdd {
            type_name: "gate".to_string(),
            after: Some("compressor".to_string()),
            at: None,
        }]
    );
}

#[test]
fn tile_dropped_outside_the_rack_adds_nothing() {
    let (mut harness, sent, _dir) = harness();
    let from = tile_centre(&harness, "gate (mono)");
    drag_and_release(&mut harness, from, pos2(from.x + 40.0, from.y + 80.0));
    assert_eq!(sent_now(&sent), []);
}

#[test]
fn unit_switch_sends_bypass() {
    let (mut harness, sent, _dir) = harness_with(WINDOW, bypassed_snapshot(&["chorus"]));
    click_unit_switch(&mut harness, "compressor", "On");
    click_unit_switch(&mut harness, "chorus", "Off");
    assert_eq!(harness.query_all_by_label("▼").count(), 0);
    assert_eq!(
        sent_now(&sent),
        [
            Command::Bypass {
                id: "compressor".to_string(),
                on: true,
            },
            Command::Bypass {
                id: "chorus".to_string(),
                on: false,
            },
        ]
    );
}

#[test]
fn muted_output_shows_the_banner() {
    let (harness, _, _dir) = harness_with(WINDOW, funk_snapshot());
    assert!(harness.query_by_label_contains("Output muted").is_none());
    let (mut harness, sent, _dir) = harness_with(WINDOW, muted_snapshot());
    harness.get_by_label("Output muted \u{2014} press Mute to unmute");
    harness.get_by_role_and_label(Role::Button, "Tuner").click();
    harness.run_steps(2);
    harness.get_by_label("Output muted \u{2014} press Mute to unmute");
    harness.get_by_role_and_label(Role::Button, "Tuner").click();
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), []);
}

#[test]
fn units_have_fixed_heights() {
    let (mut harness, _, _dir) = harness();
    assert_eq!(unit_height(&harness, "compressor unit"), 64.0);
    assert_eq!(unit_height(&harness, "chorus unit"), 64.0);
    open_unit(&mut harness, 0);
    assert_eq!(unit_height(&harness, "compressor unit"), 190.0);
    assert_eq!(unit_height(&harness, "preamp unit"), 64.0);
}

#[test]
fn five_knobs_fit_one_row_in_the_default_window() {
    let (harness, _, _dir) = harness_sized(DEFAULT_WINDOW);
    assert_eq!(unit_height(&harness, "compressor unit"), 64.0);
}

#[test]
fn closed_units_wrap_their_knobs_in_the_minimum_window() {
    let (harness, _, _dir) = harness_sized(MIN_WINDOW);
    assert_eq!(unit_height(&harness, "compressor unit"), 192.0);
    assert_eq!(unit_height(&harness, "tonestack unit"), 128.0);
    for param in ["Threshold", "Ratio", "Attack", "Release", "Makeup"] {
        let field = harness.get_by_role_and_label(Role::TextInput, param).rect();
        let unit = harness.get_by_label("compressor unit").rect();
        assert!(
            unit.contains_rect(field),
            "{param} field {field:?} outside {unit:?}"
        );
    }
}

#[test]
fn closed_unit_knobs_stay_left_of_the_move_buttons() {
    let wrap_point = (1100..=1140).step_by(4).map(|width| width as f32);
    for width in [1024.0, 1280.0, 1440.0, 1920.0]
        .into_iter()
        .chain(wrap_point)
    {
        let (harness, _, _dir) = harness_sized(Vec2::new(width, 900.0));
        for unit in ["compressor", "preamp", "tonestack", "cabinet", "chorus"] {
            let unit_rect = harness.get_by_label(&format!("{unit} unit")).rect();
            let move_left = harness
                .get_all_by_role_and_label(Role::Button, "↑")
                .chain(harness.get_all_by_role_and_label(Role::Button, "↓"))
                .map(|button| button.rect())
                .filter(|button| unit_rect.contains_rect(*button))
                .map(|button| button.left())
                .fold(f32::INFINITY, f32::min);
            let knobs_right = harness
                .get_all_by_role(Role::TextInput)
                .map(|field| field.rect())
                .filter(|field| unit_rect.contains_rect(*field))
                .map(|field| field.right())
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(move_left.is_finite() && knobs_right.is_finite());
            assert!(
                knobs_right < move_left,
                "{unit} at {width} px: knobs end at {knobs_right}, ↑/↓ start at {move_left}"
            );
        }
    }
}

#[test]
fn right_click_on_a_knob_focuses_its_value_field() {
    let (mut harness, _, _dir) = harness();
    let field = harness
        .get_by_role_and_label(Role::TextInput, "Threshold")
        .rect();
    let knob = egui::pos2(field.left() - 25.0, field.top() - 4.0);
    harness.event(egui::Event::PointerMoved(knob));
    harness.run_steps(1);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: knob,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(1);
    }
    harness.run_steps(2);
    let field = harness.get_by_role_and_label(Role::TextInput, "Threshold");
    assert!(field.is_focused());
    assert_eq!(field.value().as_deref(), Some("\u{2212}24"));
}

#[test]
fn typed_value_is_sent_on_enter() {
    let (mut harness, sent, _dir) = harness();
    edit_value(&mut harness, "Threshold", "\u{2212}30 dB");
    assert_eq!(sent_now(&sent), []);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::Set {
            param: "compressor.threshold".to_string(),
            value: -30.0,
        }]
    );
}

#[test]
fn invalid_typed_value_reverts_with_a_footer_message() {
    let (mut harness, sent, _dir) = harness();
    edit_value(&mut harness, "Threshold", "loud");
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), []);
    harness.get_by_label("error: 'loud' is not a number");
    let field = harness.get_by_role_and_label(Role::TextInput, "Threshold");
    assert_eq!(field.value().as_deref(), Some("\u{2212}24 dB"));
}

#[test]
fn escape_reverts_a_typed_value() {
    let (mut harness, sent, _dir) = harness();
    edit_value(&mut harness, "Threshold", "-30");
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), []);
    let field = harness.get_by_role_and_label(Role::TextInput, "Threshold");
    assert_eq!(field.value().as_deref(), Some("\u{2212}24 dB"));
}

#[test]
fn clicking_a_tile_adds_it_by_the_rack_rules() {
    let (mut harness, sent, _dir) = harness();
    assert_eq!(
        harness
            .get_by_role_and_label(Role::ComboBox, "Place")
            .value()
            .as_deref(),
        Some("by the Rack rules")
    );
    harness
        .get_by_role_and_label(Role::Button, "delay (stereo)")
        .click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::RackAdd {
            type_name: "delay".to_string(),
            after: Some("delay-2".to_string()),
            at: None,
        }]
    );
}

#[test]
fn audio_and_settings_apply() {
    let (mut harness, sent, _dir) = harness();
    for title in [
        "Project",
        "Preset",
        "Bass Bank",
        "Rack",
        "Audio",
        "Settings",
    ] {
        harness.get_by_label(title).click();
        harness.run_steps(2);
    }
    harness.get_all_by_label("Apply").next().unwrap().click();
    harness.run_steps(2);
    harness.get_by_label("Guitar").click();
    harness.run_steps(2);
    harness.get_all_by_label("Apply").nth(1).unwrap().click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [
            Command::SetPorts {
                input: CAPTURE.to_string(),
                output_left: LEFT.to_string(),
                output_right: RIGHT.to_string(),
            },
            Command::SetSettings {
                bank_kind: Some(BankKind::Guitar),
                ir_dir: None,
                projects_dir: None,
            },
        ]
    );
}

#[test]
fn a_tile_click_adds_at_the_chosen_place() {
    let (mut harness, sent, _dir) = harness();
    harness.get_by_label("Project").click();
    harness.run_steps(2);
    harness.get_by_label("Place").click();
    harness.run_steps(2);
    harness.get_by_label("at the front").click();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "gate (mono)")
        .click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::RackAdd {
            type_name: "gate".to_string(),
            after: None,
            at: Some(0),
        }]
    );
}

#[test]
fn typed_tempo_is_sent_on_enter_only() {
    let (mut harness, sent, _dir) = project_harness("stopped");
    harness.get_by_label("Tempo").focus();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.event(Event::Text("95".to_string()));
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), []);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::Tempo {
            bpm: Some(95.0),
            meter: None,
        }]
    );
}

#[test]
fn mixer_snapshots() {
    let (mut mixer, _, _dir) = harness_with(WINDOW, mixer_snapshot());
    click_button(&mut mixer, "Mixer");
    write_png(&mut mixer, "mixer");

    let (mut bus_rack, _, _dir) = harness_with(DEFAULT_WINDOW, mixer_snapshot());
    click_button(&mut bus_rack, "Mixer");
    click_button(&mut bus_rack, "Rack of Verb");
    write_png(&mut bus_rack, "mixer-bus-rack");

    let (mut eight_buses, _, _dir) = harness_with(DEFAULT_WINDOW, eight_buses_snapshot());
    click_button(&mut eight_buses, "Mixer");
    write_png(&mut eight_buses, "mixer-eight-buses");
    eight_buses.event(Event::PointerMoved(pos2(700.0, 420.0)));
    eight_buses.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -1000.0),
        phase: TouchPhase::Move,
        source: MouseWheelSource::Unknown,
        modifiers: Modifiers::NONE,
    });
    eight_buses.run_steps(2);
    write_png(&mut eight_buses, "mixer-eight-buses-scrolled");

    let file = "/home/you/Music/xguitare/demo-song/renders/demo-song.wav";
    let running = json!({"name": "demo-song", "range": "project", "progress": 0.42,
        "state": "running", "file": file});
    let (mut dialog, _, _dir) = harness_with(WINDOW, render_snapshot(running));
    dialog.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::R);
    write_png(&mut dialog, "render-running");

    let failed = json!({"name": "demo-song", "range": "project", "progress": 0.42,
        "state": "failed", "file": file, "error": "No space left on device (os error 28)"});
    let (mut dialog, _, _dir) = harness_with(WINDOW, render_snapshot(failed));
    click_button(&mut dialog, "Render");
    dialog.run_steps(2);
    write_png(&mut dialog, "render-failed");
}

#[test]
fn mixer_columns_send_their_commands() {
    let (mut harness, sent, _dir) = harness_with(TALL_WINDOW, mixer_snapshot());
    harness.key_press_modifiers(Modifiers::COMMAND, Key::M);
    harness.run_steps(2);
    for label in [
        "Clip Guitar",
        "pre Guitar Verb",
        "to Master Keys",
        "Solo Verb",
        "+ Add Bus",
        "Rack of Verb",
    ] {
        click_button(&mut harness, label);
    }
    assert_eq!(
        sent_now(&sent),
        [
            Command::ClipReset(MixerTarget::Track(0)),
            Command::SendSet {
                track: 0,
                bus: 0,
                level: None,
                pre: Some(false),
            },
            Command::TrackSet {
                track: 2,
                edit: TrackEdit {
                    to_master: Some(true),
                    ..TrackEdit::default()
                },
            },
            Command::BusSet {
                bus: 0,
                volume: None,
                pan: None,
                mute: None,
                solo: Some(false),
            },
            Command::BusAdd,
            Command::ShowRack(ShownRack::Bus(0)),
        ]
    );
    assert!(harness.query_by_label("Verb — Rack").is_some());
    click_button(&mut harness, "cabinet (mono)");
    assert_eq!(sent_now(&sent), []);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::M);
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), [Command::ShowRack(ShownRack::Track)]);
    assert!(harness.query_by_label("Rack of Master").is_none());
    assert!(harness.query_by_label("Guitar — Rack").is_some());
}

#[test]
fn render_dialog_sends_render_and_cancel() {
    let (mut harness, sent, _dir) = harness_with(WINDOW, mixer_snapshot());
    click_button(&mut harness, "Render");
    harness.run_steps(2);
    harness.get_by_label("16").click();
    harness.run_steps(2);
    harness
        .get_all_by_role_and_label(Role::Button, "Render")
        .last()
        .unwrap()
        .click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::Render {
            range: RenderRange::Project,
            bits: RenderBits::Pcm16,
            name: Some("demo-song".to_string()),
        }]
    );
    let running = json!({"name": "demo-song", "range": "loop", "progress": 0.5,
        "state": "running", "file": "/tmp/x.wav"});
    let (mut harness, sent, _dir) = harness_with(WINDOW, render_snapshot(running));
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::R);
    harness.run_steps(2);
    click_button(&mut harness, "Cancel");
    assert_eq!(sent_now(&sent), [Command::RenderCancel]);
}
