use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};

use eframe::egui_wgpu::{RenderState, Renderer, RendererOptions};
use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Role, TouchPhase,
    Vec2, pos2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup};
use egui_system_font_provider::SystemFontProvider;
use serde_json::{Value, json};
use xguitare_core::control::TrackRef;
use xguitare_core::control::TransportAction;
use xguitare_core::project::LoopRegion;
use xguitare_core::render::{RenderBits, RenderRange};
use xguitare_core::{BankKind, Config, ModuleRegistry};
use xguitare_gui::app::App;
use xguitare_gui::mixer_model::{MixerTarget, ShownRack};
use xguitare_gui::model::{Command, Link, Message, ModuleType, Restore, Snapshot, TrackEdit};
use xguitare_gui::project_model::AutosaveOffer;
use xguitare_gui::theme::palette;

const WINDOW: Vec2 = Vec2::new(1440.0, 900.0);
const DEFAULT_WINDOW: Vec2 = Vec2::new(1280.0, 800.0);
const MIN_WINDOW: Vec2 = Vec2::new(1024.0, 640.0);
const FULL_HD_WINDOW: Vec2 = Vec2::new(1920.0, 1080.0);
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

/// The `channels` the `modules` payload gives for a `Channels` value's Debug name.
fn channels_text(debug: &str) -> &'static str {
    match debug {
        "Mono" => "mono",
        "Stereo" => "stereo",
        _ => "mono_to_stereo",
    }
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
    let module_types = ModuleRegistry::default()
        .specs()
        .map(|spec| ModuleType {
            type_name: spec.type_name.to_string(),
            channels: channels_text(&format!("{:?}", spec.channels)).to_string(),
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

/// The Project with the brief's mixer content and two Buses: Bass DI clipped and sending pre-fader to Room, Bass Amp muted and sending to Delay, Click not to Master and sending to Room, Room soloed.
const MIXER_TRACKS: [&str; 3] = ["Bass DI", "Bass Amp", "Click"];

fn mixer_snapshot() -> Snapshot {
    let mut snapshot = project_snapshot("playing");
    for (track, name) in MIXER_TRACKS.into_iter().enumerate() {
        snapshot.status.as_mut().unwrap()["tracks"][track]["name"] = json!(name);
        snapshot.items.as_mut().unwrap()["tracks"][track]["name"] = json!(name);
    }
    let items = snapshot.items.as_mut().unwrap();
    items["tracks"][0]["items"][0]["takes"] = json!(["takes/bass-di-1.wav", "takes/bass-di-2.wav"]);
    items["tracks"][1]["items"][0]["takes"] = json!(["takes/bass-amp-1.wav"]);
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
        {"id": 0, "name": "Room", "volume_db": -4.0, "pan": 0.0, "mute": false, "solo": true,
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

/// [`mixer_snapshot`] with eight Buses, Bass DI sending to each.
fn eight_buses_snapshot() -> Snapshot {
    let mut snapshot = mixer_snapshot();
    let status = snapshot.status.as_mut().unwrap();
    let names = [
        "Room", "Delay", "Hall", "Plate", "Echo", "Chorus", "Crush", "Sub",
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

/// [`mixer_snapshot`] with the brief's Bass DI Rack: compressor, eq, blend, cabinet, limiter.
fn bass_di_rack_snapshot() -> Snapshot {
    let mut snapshot = mixer_snapshot();
    let funk = funk_params();
    let of = |module: &str| -> Vec<Value> {
        funk["params"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|param| param["module"] == module)
            .cloned()
            .collect()
    };
    let eq = EQ_SPEC.map(|(name, min, max, default, unit)| {
        let value = EQ_EXAMPLE
            .iter()
            .find(|(param, _)| *param == name)
            .map_or(default, |(_, value)| *value);
        param("eq", name, value, (min, max, default, unit, false))
    });
    let blend = [
        param("blend", "blend", 100.0, (0.0, 100.0, 100.0, "%", false)),
        param("blend", "drive", 40.0, (0.0, 100.0, 40.0, "%", false)),
        param("blend", "level", 0.0, (-24.0, 12.0, 0.0, "dB", false)),
        param("blend", "treble", 0.0, (-12.0, 12.0, 0.0, "dB", false)),
    ];
    let limiter = [
        param("limiter", "input", 6.0, (0.0, 18.0, 0.0, "dB", false)),
        param("limiter", "ceiling", -1.0, (-12.0, 0.0, -1.0, "dB", false)),
    ];
    let params: Vec<Value> = of("compressor")
        .into_iter()
        .chain(eq)
        .chain(blend)
        .chain(of("cabinet"))
        .chain(limiter)
        .collect();
    snapshot.params = Some(json!({ "params": params }));
    snapshot
}

/// [`bass_di_rack_snapshot`] recording on the armed Bass DI past the count-in, the playhead at `frames`.
fn recording_snapshot(frames: u64) -> Snapshot {
    let mut snapshot = bass_di_rack_snapshot();
    let status = snapshot.status.as_mut().unwrap();
    status["transport"] = json!("recording");
    status["count_in_remaining"] = json!(0);
    status["position"]["frames"] = json!(frames);
    snapshot
}

/// [`harness_with`] whose App takes later Snapshots from the returned sender.
fn streamed_harness(
    size: Vec2,
    snapshot: Snapshot,
) -> (Harness<'static, App>, Sender<Snapshot>, TempDir) {
    let (config, dir) = bass_config();
    let (commands, _sent) = mpsc::channel();
    let (updates, snapshots) = mpsc::channel();
    updates.send(snapshot).unwrap();
    let mut harness = Harness::builder()
        .with_size(size)
        .with_context_setup(|ctx| ctx.add_font_provider(Arc::new(SystemFontProvider::new())))
        .renderer(shared_gpu_renderer())
        .build_eframe(move |cc| App::with_updates(cc, &config, snapshots, commands));
    harness.run_steps(2);
    (harness, updates, dir)
}

fn click_button(harness: &mut Harness<App>, label: &str) {
    harness.get_by_role_and_label(Role::Button, label).click();
    harness.run_steps(2);
}

fn project_harness(transport: &str) -> (Harness<'static, App>, Receiver<Command>, TempDir) {
    harness_with(WINDOW, project_snapshot(transport))
}

/// Clicks the On/Off switch named `name`: the Module's name, or `Tuner`.
fn click_unit_switch(harness: &mut Harness<App>, name: &str) {
    harness.get_by_role_and_label(Role::Switch, name).click();
    harness.run_steps(2);
}

fn switch_is_on(harness: &Harness<App>, name: &str) -> bool {
    harness
        .get_by_role_and_label(Role::Switch, name)
        .accesskit_node()
        .toggled()
        == Some(egui::accesskit::Toggled::True)
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

// One wgpu device for every harness: creating one per test thread fails under parallel runs.
static GPU: OnceLock<RenderState> = OnceLock::new();

fn shared_gpu_renderer() -> WgpuTestRenderer {
    let gpu =
        GPU.get_or_init(|| create_render_state(default_wgpu_setup(), RendererOptions::PREDICTABLE));
    let renderer = Renderer::new(&gpu.device, gpu.target_format, RendererOptions::PREDICTABLE);
    WgpuTestRenderer::from_render_state(RenderState {
        renderer: Arc::new(egui::mutex::RwLock::new(renderer)),
        ..gpu.clone()
    })
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
        .renderer(shared_gpu_renderer())
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
    click_unit_switch(&mut tuner, "Tuner");
    write_png(&mut tuner, "tuner-open");

    for (size, suffix) in [(DEFAULT_WINDOW, "1280x800"), (MIN_WINDOW, "1024x640")] {
        let (mut save_as, _, _dir) = harness_sized(size);
        save_as.get_by_label("Save as new").click();
        write_png(&mut save_as, &format!("save-as-open-{suffix}"));
    }

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
    switch_focus
        .get_by_role_and_label(Role::Switch, "Compressor")
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

fn choice(module: &str, name: &str, value: f64, labels: &[&str]) -> Value {
    let mut entry = param(
        module,
        name,
        value,
        (0.0, (labels.len() - 1) as f64, 0.0, "", true),
    );
    entry["labels"] = json!(labels);
    entry
}

/// The Funk Rack with an autowah after the compressor: band-pass, sweeping up.
fn autowah_snapshot() -> Snapshot {
    let mut snapshot = funk_snapshot();
    let autowah = [
        choice("autowah", "mode", 1.0, &["lp", "bp", "hp"]),
        choice("autowah", "direction", 0.0, &["up", "down"]),
        param(
            "autowah",
            "sensitivity",
            0.0,
            (-24.0, 36.0, 0.0, "dB", false),
        ),
        param("autowah", "attack", 5.0, (0.5, 50.0, 5.0, "ms", false)),
        param(
            "autowah",
            "decay",
            150.0,
            (20.0, 1000.0, 150.0, "ms", false),
        ),
        param(
            "autowah",
            "freq_low",
            200.0,
            (80.0, 800.0, 200.0, "Hz", false),
        ),
        param(
            "autowah",
            "freq_high",
            2500.0,
            (400.0, 4000.0, 2500.0, "Hz", false),
        ),
        param("autowah", "q", 3.0, (0.5, 12.0, 3.0, "", false)),
        param("autowah", "dry", 50.0, (0.0, 100.0, 50.0, "%", false)),
        param("autowah", "wet", 100.0, (0.0, 100.0, 100.0, "%", false)),
    ];
    let params = snapshot.params.as_mut().unwrap()["params"]
        .as_array_mut()
        .unwrap();
    let after_compressor = params
        .iter()
        .position(|param| param["module"] == "preamp")
        .unwrap();
    params.splice(after_compressor..after_compressor, autowah);
    snapshot
}

const AUTOWAH_CLOSED_INDEX: usize = 1;

#[test]
fn autowah_snapshots() {
    for (size, name) in [
        (DEFAULT_WINDOW, "autowah-open-1280x800"),
        (MIN_WINDOW, "autowah-open-1024x640"),
    ] {
        let (mut open, _, _dir) = harness_with(size, autowah_snapshot());
        open_unit(&mut open, AUTOWAH_CLOSED_INDEX);
        write_png(&mut open, name);
    }
    let (mut closed, _, _dir) = harness_with(DEFAULT_WINDOW, autowah_snapshot());
    write_png(&mut closed, "autowah-closed");
}

#[test]
fn open_autowah_grows_to_hold_its_wrapped_rows() {
    for size in [DEFAULT_WINDOW, MIN_WINDOW, WINDOW] {
        let (mut harness, _, _dir) = harness_with(size, autowah_snapshot());
        open_unit(&mut harness, AUTOWAH_CLOSED_INDEX);
        let unit = harness.get_by_label("autowah unit").rect();
        let next_unit = unit_top(&harness, "preamp");
        let fields: Vec<_> = harness
            .get_all_by_role(Role::TextInput)
            .map(|field| field.rect())
            .filter(|field| unit.x_range().contains(field.center().x))
            .filter(|field| field.top() >= unit.top() && field.top() < next_unit)
            .collect();
        assert_eq!(fields.len(), 8, "at {size:?}");
        for field in fields {
            assert!(
                unit.contains_rect(field),
                "{field:?} outside {unit:?} at {size:?}"
            );
        }
    }
}

#[test]
fn choice_buttons_send_the_choice() {
    let (mut harness, sent, _dir) = harness_with(DEFAULT_WINDOW, autowah_snapshot());
    open_unit(&mut harness, AUTOWAH_CLOSED_INDEX);
    harness
        .get_by_role_and_label(Role::RadioButton, "Mode, HP")
        .click();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::RadioButton, "Direction, Up")
        .focus();
    harness.run_steps(2);
    harness.key_press(Key::ArrowDown);
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [
            Command::Set {
                param: "autowah.mode".to_string(),
                value: 2.0,
            },
            Command::Set {
                param: "autowah.direction".to_string(),
                value: 1.0,
            },
        ]
    );
    assert!(
        harness
            .get_by_role_and_label(Role::RadioButton, "Direction, Down")
            .is_focused()
    );
}

#[test]
fn a_choice_group_is_one_tab_stop() {
    let (mut harness, sent, _dir) = harness_with(DEFAULT_WINDOW, autowah_snapshot());
    open_unit(&mut harness, AUTOWAH_CLOSED_INDEX);
    let focused = |harness: &Harness<App>, name: &str| {
        harness
            .get_by_role_and_label(Role::RadioButton, name)
            .is_focused()
    };
    harness
        .get_by_role_and_label(Role::RadioButton, "Mode, BP")
        .focus();
    harness.run_steps(2);
    harness.key_press(Key::Tab);
    harness.run_steps(2);
    assert!(focused(&harness, "Direction, Up"));
    harness.key_press(Key::Tab);
    harness.run_steps(2);
    assert!(
        harness
            .get_all_by_role(Role::RadioButton)
            .all(|radio| !radio.is_focused())
    );
    for _ in 0..2 {
        harness.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
        harness.run_steps(2);
    }
    assert!(focused(&harness, "Mode, BP"));
    harness.key_press(Key::ArrowRight);
    harness.run_steps(2);
    assert!(focused(&harness, "Mode, HP"));
    assert_eq!(
        sent_now(&sent),
        [Command::Set {
            param: "autowah.mode".to_string(),
            value: 2.0,
        }]
    );
    assert_eq!(
        harness
            .get_by_role_and_label(Role::RadioButton, "Mode, HP")
            .accesskit_node()
            .toggled(),
        Some(egui::accesskit::Toggled::True)
    );
}

const FOCUS_CROP_MARGIN: f32 = 8.0;
const FOCUS_CROP_TOP: u32 = 16;
const FOCUS_CROP_GAP: u32 = 16;

/// Whether `pixel` shows `rgb` within a few levels per channel.
fn is_near(pixel: [u8; 4], (r, g, b): (u8, u8, u8)) -> bool {
    [r, g, b]
        .iter()
        .zip(pixel)
        .all(|(want, got)| want.abs_diff(got) <= PIXEL_TOLERANCE)
}

const PIXEL_TOLERANCE: u8 = 12;

/// A keyboard-focused button keeps the fill it has at rest inside its focus ring; only a press darkens it.
#[test]
fn focused_buttons_keep_their_rest_fill() {
    let (mut harness, _, _dir) = harness_with(DEFAULT_WINDOW, autowah_snapshot());
    let colours = palette();
    let scale = harness.ctx.pixels_per_point();
    for (label, rest) in [("Save as new", colours.panel_open), ("Save", colours.text)] {
        let button = harness.get_by_role_and_label(Role::Button, label);
        let rect = button.rect();
        button.focus();
        harness.run_steps(2);
        let image = harness.render().unwrap();
        let inside = |rect: egui::Rect| {
            (
                ((rect.left() + 6.0) * scale) as u32,
                (rect.center().y * scale) as u32,
            )
        };
        let (x, y) = inside(rect);
        assert!(
            is_near(image.get_pixel(x, y).0, rest),
            "focused {label} is {:?}",
            image.get_pixel(x, y).0
        );
    }
    let rect = harness
        .get_by_role_and_label(Role::Button, "Save as new")
        .rect();
    harness.event(Event::PointerMoved(rect.center()));
    harness.run_steps(1);
    pointer_button(&mut harness, rect.center(), true);
    harness.run_steps(2);
    let image = harness.render().unwrap();
    let (x, y) = (
        ((rect.left() + 6.0) * scale) as u32,
        (rect.center().y * scale) as u32,
    );
    assert!(
        is_near(image.get_pixel(x, y).0, colours.well),
        "pressed Save as new is {:?}",
        image.get_pixel(x, y).0
    );
}

/// The focused knob, button, choice group (one choice hovered) and field side by side, as the states mockup's focus row; the knob, button and lit choice wear the focus ring above their top edge, the field only its lit edge.
#[test]
fn focus_states_snapshot() {
    let (mut harness, _, _dir) = harness_with(DEFAULT_WINDOW, autowah_snapshot());
    open_unit(&mut harness, AUTOWAH_CLOSED_INDEX);
    let colours = palette();
    let mut shots = Vec::new();
    let knob = harness.get_by_role_and_label(Role::Slider, "Threshold");
    let rect = knob.rect();
    knob.focus();
    harness.run_steps(2);
    shots.push((harness.render().unwrap(), rect, rect, "knob"));
    let button = harness.get_by_role_and_label(Role::Button, "Save as new");
    let rect = button.rect();
    button.focus();
    harness.run_steps(2);
    shots.push((harness.render().unwrap(), rect, rect, "button"));
    let lit = harness.get_by_role_and_label(Role::RadioButton, "Mode, BP");
    let lit_rect = lit.rect();
    let hovered = harness
        .get_by_role_and_label(Role::RadioButton, "Mode, LP")
        .rect();
    let last = harness
        .get_by_role_and_label(Role::RadioButton, "Mode, HP")
        .rect();
    let rect = lit_rect.union(hovered).union(last);
    lit.focus();
    harness.event(Event::PointerMoved(hovered.center()));
    harness.run_steps(2);
    shots.push((harness.render().unwrap(), rect, lit_rect, "choice"));
    let field = harness.get_by_role_and_label(Role::TextInput, "Threshold");
    let rect = field.rect();
    field.focus();
    harness.run_steps(2);
    shots.push((harness.render().unwrap(), rect, rect, "field"));
    let scale = harness.ctx.pixels_per_point();
    let at = |x: f32, y: f32| ((x * scale) as u32, (y * scale) as u32);
    for (image, _, widget, what) in &shots {
        let ring = (2..=4).any(|out| {
            let (x, y) = at(widget.center().x, widget.top() - out as f32);
            is_near(image.get_pixel(x, y).0, colours.focus_ring)
        });
        if *what == "field" {
            assert!(!ring, "a focused field wears no focus ring");
            let (x, y) = at(widget.center().x, widget.top() + 1.0);
            assert!(
                is_near(image.get_pixel(x, y).0, colours.lit),
                "a focused field has a lit edge"
            );
        } else {
            assert!(ring, "no focus ring above the focused {what}");
        }
    }
    let mut canvas = shots[0].0.clone();
    let mut case = *canvas.get_pixel(0, 0);
    let (r, g, b) = palette().case;
    case.0 = [r, g, b, 255];
    canvas.pixels_mut().for_each(|pixel| *pixel = case);
    let mut left = FOCUS_CROP_GAP;
    for (image, rect, _, _) in &shots {
        let rect = rect.expand(FOCUS_CROP_MARGIN);
        let (x0, y0) = ((rect.left() * scale) as u32, (rect.top() * scale) as u32);
        let (width, height) = (
            (rect.width() * scale) as u32,
            (rect.height() * scale) as u32,
        );
        for y in 0..height {
            for x in 0..width {
                canvas.put_pixel(
                    left + x,
                    FOCUS_CROP_TOP + y,
                    *image.get_pixel(x0 + x, y0 + y),
                );
            }
        }
        left += width + FOCUS_CROP_GAP;
    }
    fs::create_dir_all(snapshot_dir()).unwrap();
    canvas
        .save(snapshot_dir().join("focus-states.png"))
        .unwrap();
}

#[test]
fn closed_chips_switch_two_choices_and_open_for_more() {
    let (mut harness, sent, _dir) = harness_with(DEFAULT_WINDOW, autowah_snapshot());
    harness
        .get_by_role_and_label(Role::Button, "Direction, Up, selected")
        .click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::Set {
            param: "autowah.direction".to_string(),
            value: 1.0,
        }]
    );
    assert_eq!(harness.query_all_by_label("▼").count(), 0);
    harness
        .get_by_role_and_label(Role::Button, "Mode, BP, selected")
        .click();
    harness.run_steps(2);
    assert_eq!(harness.query_all_by_label("▼").count(), 1);
    assert_eq!(sent_now(&sent), []);
}

/// The Funk Rack with a blend after the compressor, at its defaults.
fn blend_snapshot() -> Snapshot {
    let mut snapshot = funk_snapshot();
    let blend = [
        param("blend", "blend", 100.0, (0.0, 100.0, 100.0, "%", false)),
        param("blend", "drive", 40.0, (0.0, 100.0, 40.0, "%", false)),
        param("blend", "level", 0.0, (-24.0, 12.0, 0.0, "dB", false)),
        param("blend", "bass", 0.0, (-12.0, 12.0, 0.0, "dB", false)),
        choice("blend", "bass_freq", 1.0, &["40 Hz", "80 Hz"]),
        param("blend", "mid", 0.0, (-12.0, 12.0, 0.0, "dB", false)),
        choice("blend", "mid_freq", 0.0, &["500 Hz", "1000 Hz"]),
        param("blend", "treble", 0.0, (-12.0, 12.0, 0.0, "dB", false)),
        param("blend", "presence", 50.0, (0.0, 100.0, 50.0, "%", false)),
        param("blend", "crossover", 20.0, (20.0, 400.0, 20.0, "Hz", false)),
    ];
    let params = snapshot.params.as_mut().unwrap()["params"]
        .as_array_mut()
        .unwrap();
    let after_compressor = params
        .iter()
        .position(|param| param["module"] == "preamp")
        .unwrap();
    params.splice(after_compressor..after_compressor, blend);
    snapshot
}

const BLEND_CLOSED_INDEX: usize = 1;

#[test]
fn blend_snapshots() {
    let (mut open, _, _dir) = harness_with(DEFAULT_WINDOW, blend_snapshot());
    open_unit(&mut open, BLEND_CLOSED_INDEX);
    write_png(&mut open, "blend-open-1280x800");
    let (mut closed, _, _dir) = harness_with(DEFAULT_WINDOW, blend_snapshot());
    write_png(&mut closed, "blend-closed");
}

/// The Funk Rack with `module` spliced in before the first Parameter of `before`.
fn with_module(module: Vec<Value>, before: &str) -> Snapshot {
    let mut snapshot = funk_snapshot();
    let params = snapshot.params.as_mut().unwrap()["params"]
        .as_array_mut()
        .unwrap();
    let at = params
        .iter()
        .position(|param| param["module"] == before)
        .unwrap();
    params.splice(at..at, module);
    snapshot
}

#[test]
fn limiter_closed_snapshot() {
    let limiter = vec![
        param("limiter", "input", 6.0, (0.0, 18.0, 0.0, "dB", false)),
        param("limiter", "ceiling", -1.0, (-12.0, 0.0, -1.0, "dB", false)),
        param(
            "limiter",
            "release",
            100.0,
            (10.0, 1000.0, 100.0, "ms", false),
        ),
        param("limiter", "lookahead", 1.0, (0.0, 5.0, 1.0, "ms", false)),
    ];
    let (mut closed, _, _dir) = harness_with(DEFAULT_WINDOW, with_module(limiter, "cabinet"));
    write_png(&mut closed, "limiter-closed");
}

const SHAPER_CLOSED_INDEX: usize = 1;

#[test]
fn shaper_open_snapshot() {
    let shaper = vec![
        param("shaper", "attack", 6.0, (-15.0, 15.0, 0.0, "dB", false)),
        param("shaper", "sustain", -4.0, (-24.0, 24.0, 0.0, "dB", false)),
        param("shaper", "output", 0.0, (-12.0, 12.0, 0.0, "dB", false)),
        param("shaper", "mix", 100.0, (0.0, 100.0, 100.0, "%", false)),
    ];
    let (mut open, _, _dir) = harness_with(DEFAULT_WINDOW, with_module(shaper, "preamp"));
    open_unit(&mut open, SHAPER_CLOSED_INDEX);
    write_png(&mut open, "shaper-open");
}

const EQ_SPEC: [(&str, f64, f64, f64, &str); 13] = [
    ("low_cut", 10.0, 300.0, 10.0, "Hz"),
    ("low_freq", 20.0, 500.0, 100.0, "Hz"),
    ("low_gain", -15.0, 15.0, 0.0, "dB"),
    ("mid1_freq", 40.0, 5000.0, 400.0, "Hz"),
    ("mid1_gain", -15.0, 15.0, 0.0, "dB"),
    ("mid1_q", 0.3, 10.0, 0.707, ""),
    ("mid2_freq", 200.0, 10000.0, 2500.0, "Hz"),
    ("mid2_gain", -15.0, 15.0, 0.0, "dB"),
    ("mid2_q", 0.3, 10.0, 0.707, ""),
    ("high_freq", 1000.0, 20000.0, 8000.0, "Hz"),
    ("high_gain", -15.0, 15.0, 0.0, "dB"),
    ("high_cut", 1000.0, 20000.0, 20000.0, "Hz"),
    ("output", -12.0, 12.0, 0.0, "dB"),
];

/// The Funk Rack with an eq before the preamp, its Parameters at `values` and the rest at their defaults.
fn eq_snapshot(values: &[(&str, f64)]) -> Snapshot {
    let mut snapshot = funk_snapshot();
    let eq = EQ_SPEC.map(|(name, min, max, default, unit)| {
        let value = values
            .iter()
            .find(|(param, _)| *param == name)
            .map_or(default, |(_, value)| *value);
        param("eq", name, value, (min, max, default, unit, false))
    });
    let params = snapshot.params.as_mut().unwrap()["params"]
        .as_array_mut()
        .unwrap();
    let before_preamp = params
        .iter()
        .position(|param| param["module"] == "preamp")
        .unwrap();
    params.splice(before_preamp..before_preamp, eq);
    snapshot
}

/// Blueprint frame 3.
const EQ_EXAMPLE: [(&str, f64); 5] = [
    ("low_cut", 40.0),
    ("low_gain", 4.0),
    ("mid1_gain", -3.0),
    ("mid2_gain", 2.0),
    ("high_cut", 12_000.0),
];
const EQ_CLOSED_INDEX: usize = 1;
/// More than the footer below the Rack's view.
const FOOTER_ALLOWANCE: f32 = 60.0;

/// Scrolls the eq unit to the top of the Rack's view when the window cuts its bottom off, then hovers the knob left of the value field named `field`.
fn hover_knob_of(harness: &mut Harness<App>, window: Vec2, field: &str) {
    let unit = harness.get_by_label("eq unit").rect();
    if unit.bottom() > window.y - FOOTER_ALLOWANCE {
        harness.event(Event::PointerMoved(unit.center()));
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -unit.top()),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(20);
    }
    let field = harness.get_by_role_and_label(Role::TextInput, field).rect();
    let knob = pos2(field.left() - 25.0, field.top());
    harness.hover_at(knob);
    harness.run_steps(2);
}

#[test]
fn eq_snapshots() {
    for (size, name) in [
        (DEFAULT_WINDOW, "eq-open-1280x800"),
        (MIN_WINDOW, "eq-open-1024x640"),
    ] {
        let (mut open, _, _dir) = harness_with(size, eq_snapshot(&EQ_EXAMPLE));
        open_unit(&mut open, EQ_CLOSED_INDEX);
        hover_knob_of(&mut open, size, "Mid 1 Freq");
        write_png(&mut open, name);
    }
    let (mut defaults, _, _dir) = harness_with(DEFAULT_WINDOW, eq_snapshot(&[]));
    open_unit(&mut defaults, EQ_CLOSED_INDEX);
    write_png(&mut defaults, "eq-defaults");
    let (mut closed, _, _dir) = harness_with(DEFAULT_WINDOW, eq_snapshot(&EQ_EXAMPLE));
    write_png(&mut closed, "eq-closed");
    let (mut flat, _, _dir) = harness_with(DEFAULT_WINDOW, eq_snapshot(&[]));
    flat.get_by_label("flat");
    write_png(&mut flat, "eq-closed-flat");
    let mut bypassed = eq_snapshot(&EQ_EXAMPLE);
    for param in bypassed.params.as_mut().unwrap()["params"]
        .as_array_mut()
        .unwrap()
    {
        param["bypass"] = json!(param["module"] == "eq");
    }
    let (mut bypassed, _, _dir) = harness_with(DEFAULT_WINDOW, bypassed);
    assert!(!switch_is_on(&bypassed, "EQ"));
    write_png(&mut bypassed, "eq-closed-bypassed");
}

#[test]
fn open_eq_holds_its_curve_and_band_knobs() {
    for size in [DEFAULT_WINDOW, MIN_WINDOW, WINDOW] {
        let (mut harness, _, _dir) = harness_with(size, eq_snapshot(&EQ_EXAMPLE));
        open_unit(&mut harness, EQ_CLOSED_INDEX);
        let unit = harness.get_by_label("eq unit").rect();
        let curve = harness.get_by_role(Role::Image).rect();
        assert!(unit.contains_rect(curve), "curve outside at {size:?}");
        let next_unit = unit_top(&harness, "preamp");
        let fields: Vec<_> = harness
            .get_all_by_role(Role::TextInput)
            .map(|field| field.rect())
            .filter(|field| unit.x_range().contains(field.center().x))
            .filter(|field| field.top() >= unit.top() && field.top() < next_unit)
            .collect();
        assert_eq!(fields.len(), 13, "at {size:?}");
        for field in fields {
            assert!(
                unit.contains_rect(field),
                "{field:?} outside {unit:?} at {size:?}"
            );
        }
        harness.get_by_role_and_label(Role::TextInput, "Mid 1 Freq");
        harness.get_by_role_and_label(Role::Slider, "Mid 1 Freq");
        harness.get_by_role_and_label(Role::Slider, "Mid 2 Q");
        harness.get_by_role_and_label(Role::Slider, "Threshold");
        harness.get_by_role_and_label(Role::Label, "EQ");
    }
}

#[test]
fn closed_eq_shows_a_named_mini_curve_on_one_row() {
    let (harness, _, _dir) = harness_with(DEFAULT_WINDOW, eq_snapshot(&EQ_EXAMPLE));
    let curve = harness.get_by_role_and_label(
        Role::Image,
        "EQ curve: Low cut 40 Hz; Low 4 dB at 100 Hz; Mid 1 −3 dB at 400 Hz, Q 0.71; \
         Mid 2 2 dB at 2.5 kHz, Q 0.71; High 0 dB at 8 kHz; High cut 12 kHz; Output 0 dB",
    );
    assert_eq!(curve.rect().size(), Vec2::new(160.0, 36.0));
    assert_eq!(unit_height(&harness, "eq unit"), 64.0);
}

fn metronome_snapshot() -> Snapshot {
    let mut snapshot = funk_snapshot();
    snapshot.status.as_mut().unwrap()["metronome"] = json!({
        "on": true, "bpm": 90.0, "meter": [3, 4], "volume_db": -12.0, "beat": 1, "beats_played": 1,
    });
    snapshot
}

#[test]
fn metronome_section_snapshot() {
    let (mut harness, _, _dir) = harness_with(WINDOW, metronome_snapshot());
    for title in ["Bass Bank", "Add Module"] {
        harness.get_by_label(title).click();
        harness.run_steps(2);
    }
    harness.get_by_label("Beat 1");
    write_png(&mut harness, "metronome");
}

#[test]
fn double_click_on_the_metronome_volume_knob_resets_it_to_its_default() {
    let (mut harness, sent, _dir) = harness_with(WINDOW, metronome_snapshot());
    for title in ["Bass Bank", "Add Module"] {
        harness.get_by_label(title).click();
        harness.run_steps(2);
    }
    harness.get_by_label("Volume");
    let knob = harness
        .get_by_role_and_label(Role::Slider, "Metronome volume")
        .rect()
        .center();
    harness.event(Event::PointerMoved(knob));
    harness.run_steps(1);
    // Queued events get a 0.25 s frame each, too far apart for egui's 0.3 s double click.
    for pressed in [true, false, true, false] {
        harness.input_mut().events.push(Event::PointerButton {
            pos: knob,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::Metronome {
            on: None,
            bpm: None,
            meter: None,
            volume: Some(-12.0),
        }]
    );
}

/// Where the window first sees the recording snapshot fixture recording (bar 2, beat 3), then the playhead (bar 3, beat 2).
const RECORDING_FROM: u64 = 144_000;
const PLAYHEAD: u64 = 230_400;
/// The eq among the closed units of [`bass_di_rack_snapshot`].
const BASS_DI_EQ_CLOSED_INDEX: usize = 1;

#[test]
fn project_snapshots() {
    write_png(&mut project_harness("playing").0, "project");

    let (mut counting, _, _dir) = project_harness("recording");
    write_png(&mut counting, "project-count-in");

    let (mut recording, updates, _dir) =
        streamed_harness(DEFAULT_WINDOW, recording_snapshot(RECORDING_FROM));
    updates.send(recording_snapshot(PLAYHEAD)).unwrap();
    write_png(&mut recording, "project-recording");

    for (size, suffix) in [
        (MIN_WINDOW, "1024x640"),
        (DEFAULT_WINDOW, "1280x800"),
        (FULL_HD_WINDOW, "1920x1080"),
    ] {
        let (mut view, _, _dir) = harness_with(size, bass_di_rack_snapshot());
        write_png(&mut view, &format!("view-arrange-{suffix}"));
        view.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
        if size == DEFAULT_WINDOW {
            view.run_steps(2);
            open_unit(&mut view, BASS_DI_EQ_CLOSED_INDEX);
        }
        write_png(&mut view, &format!("view-rack-{suffix}"));
    }

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
fn a_project_opens_in_arrange_and_the_switch_and_keys_change_the_view() {
    let (mut harness, sent, _dir) = harness_with(WINDOW, mixer_snapshot());
    let timeline = |harness: &Harness<App>| harness.query_by_label("Timeline ruler").is_some();
    let rack = |harness: &Harness<App>| harness.query_by_label("Bass DI — Rack").is_some();
    let mixer = |harness: &Harness<App>| harness.query_by_label("Rack of Master").is_some();
    assert!(timeline(&harness) && rack(&harness) && !mixer(&harness));
    click_button(&mut harness, "Rack");
    assert!(!timeline(&harness) && rack(&harness) && !mixer(&harness));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num3);
    harness.run_steps(2);
    assert!(!timeline(&harness) && !rack(&harness) && mixer(&harness));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::M);
    harness.run_steps(2);
    assert!(!timeline(&harness) && rack(&harness) && !mixer(&harness));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    harness.run_steps(2);
    assert!(timeline(&harness) && rack(&harness) && !mixer(&harness));
    assert!(
        !sent_now(&sent)
            .iter()
            .any(|command| matches!(command, Command::Load(_)))
    );
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
fn bank_kind_is_one_choice_group_moved_by_arrows() {
    let (mut harness, sent, _dir) = harness();
    for title in [
        "Preset",
        "Bass Bank",
        "Tuner and Mute",
        "Metronome",
        "Settings",
    ] {
        harness.get_by_label(title).click();
        harness.run_steps(2);
    }
    harness
        .get_by_role_and_label(Role::RadioButton, "Bass")
        .focus();
    harness.run_steps(2);
    harness.key_press(Key::ArrowLeft);
    harness.run_steps(2);
    assert!(
        harness
            .get_by_role_and_label(Role::RadioButton, "Guitar")
            .is_focused()
    );
    harness.get_by_label("Apply").click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::SetSettings {
            bank_kind: Some(BankKind::Guitar),
            ir_dir: None,
            projects_dir: None,
        }]
    );
}

#[test]
fn the_metronome_switch_turns_it_off() {
    let (mut harness, sent, _dir) = harness_with(WINDOW, metronome_snapshot());
    harness
        .get_by_role_and_label(Role::Switch, "Metronome")
        .click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::Metronome {
            on: Some(false),
            bpm: None,
            meter: None,
            volume: None,
        }]
    );
}

#[test]
fn the_tuner_buttons_show_at_1280x800_and_the_metronome_scrolls_into_view() {
    let (mut harness, _, _dir) = harness_with(DEFAULT_WINDOW, metronome_snapshot());
    harness.run_steps(2);
    let bottom = DEFAULT_WINDOW.y;
    for label in ["Tuner", "Mute"] {
        let rect = harness.get_by_role_and_label(Role::Button, label).rect();
        assert!(rect.bottom() <= bottom, "{label} ends at {}", rect.bottom());
    }
    write_png(&mut harness, "left-panel-1280x800");
    let panel = harness.get_by_label("Add Module").rect();
    harness.event(Event::PointerMoved(panel.center()));
    harness.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -200.0),
        phase: TouchPhase::Move,
        source: MouseWheelSource::Unknown,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(20);
    let metronome = harness
        .get_by_role_and_label(Role::Switch, "Metronome")
        .rect();
    assert!(
        metronome.bottom() <= bottom,
        "the Metronome ends at {} after scrolling",
        metronome.bottom()
    );
    harness.get_by_label("Beat 1");
}

#[test]
fn only_a_bank_kind_change_in_settings_drops_the_undo() {
    let (mut harness, sent, _dir) = harness();
    let from = unit_header_point(&harness, "chorus");
    drag_and_release(&mut harness, from, pos2(100.0, 400.0));
    for title in [
        "Preset",
        "Bass Bank",
        "Tuner and Mute",
        "Metronome",
        "Settings",
    ] {
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
    assert!(switch_is_on(&harness, "Compressor"));
    assert!(!switch_is_on(&harness, "Chorus"));
    click_unit_switch(&mut harness, "Compressor");
    click_unit_switch(&mut harness, "Chorus");
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
    assert_eq!(unit_height(&harness, "compressor unit"), 168.0);
    assert_eq!(unit_height(&harness, "preamp unit"), 64.0);
}

#[test]
fn tuner_unit_switch_opens_and_closes_the_tuner_view_and_its_mute() {
    let (mut harness, sent, _dir) = harness();
    assert_eq!(unit_height(&harness, "Tuner unit"), 64.0);
    assert!(!switch_is_on(&harness, "Tuner"));
    click_unit_switch(&mut harness, "Tuner");
    assert_eq!(unit_height(&harness, "Tuner unit"), 168.0);
    assert!(switch_is_on(&harness, "Tuner"));
    assert_eq!(sent_now(&sent), [Command::Mute(true)]);
    click_unit_switch(&mut harness, "Tuner");
    assert_eq!(unit_height(&harness, "Tuner unit"), 64.0);
    assert_eq!(sent_now(&sent), [Command::Mute(false)]);
}

#[test]
fn five_knobs_fit_one_row_in_the_default_window() {
    let (harness, _, _dir) = harness_sized(DEFAULT_WINDOW);
    assert_eq!(unit_height(&harness, "compressor unit"), 64.0);
}

#[test]
fn closed_units_wrap_their_knobs_in_the_minimum_window() {
    let (harness, _, _dir) = harness_sized(MIN_WINDOW);
    assert_eq!(unit_height(&harness, "compressor unit"), 128.0);
    assert_eq!(unit_height(&harness, "tonestack unit"), 64.0);
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
fn invalid_typed_value_reverts_with_an_error_toast() {
    let (mut harness, sent, _dir) = harness();
    edit_value(&mut harness, "Threshold", "loud");
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), []);
    harness.get_by_label("Could not set the Parameter");
    harness.get_by_label("'loud' is not a number");
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
        "Add Module",
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
    for (size, suffix) in [
        (MIN_WINDOW, "1024x640"),
        (DEFAULT_WINDOW, "1280x800"),
        (FULL_HD_WINDOW, "1920x1080"),
    ] {
        let (mut mixer, _, _dir) = harness_with(size, mixer_snapshot());
        click_button(&mut mixer, "Mixer");
        write_png(&mut mixer, &format!("view-mixer-{suffix}"));
    }
    for (label, name) in [
        ("Volume Bass Amp", "mixer-fader-focused"),
        ("Mute Bass Amp", "mixer-mute-focused"),
        ("Send Bass DI to Room", "mixer-send-focused"),
    ] {
        let (mut focused, _, _dir) = harness_with(DEFAULT_WINDOW, mixer_snapshot());
        click_button(&mut focused, "Mixer");
        let in_mixer = focused
            .get_all_by_label(label)
            .max_by(|one, two| one.rect().top().total_cmp(&two.rect().top()))
            .unwrap();
        in_mixer.focus();
        write_png(&mut focused, name);
    }

    let (mut bus_rack, _, _dir) = harness_with(DEFAULT_WINDOW, mixer_snapshot());
    click_button(&mut bus_rack, "Mixer");
    click_button(&mut bus_rack, "Rack of Room");
    click_button(&mut bus_rack, "Rack");
    write_png(&mut bus_rack, "mixer-bus-rack");

    let (mut eight_buses, _, _dir) = harness_with(DEFAULT_WINDOW, eight_buses_snapshot());
    click_button(&mut eight_buses, "Mixer");
    write_png(&mut eight_buses, "mixer-eight-buses");
    let (mut eight_buses, _, _dir) = harness_with(MIN_WINDOW, eight_buses_snapshot());
    click_button(&mut eight_buses, "Mixer");
    let sends = eight_buses.get_by_label("Send Bass DI to Room").rect();
    eight_buses.event(Event::PointerMoved(sends.center()));
    eight_buses.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -200.0),
        phase: TouchPhase::Move,
        source: MouseWheelSource::Unknown,
        modifiers: Modifiers::NONE,
    });
    eight_buses.run_steps(2);
    write_png(&mut eight_buses, "mixer-eight-buses-scrolled");

    let file = "/home/you/Music/xguitare/demo-song/renders/demo-song.wav";
    let running = json!({"name": "demo-song", "range": "project", "progress": 0.42,
        "state": "running", "file": file});
    for (size, suffix) in [(DEFAULT_WINDOW, "1280x800"), (MIN_WINDOW, "1024x640")] {
        let (mut dialog, _, _dir) = harness_with(size, render_snapshot(running.clone()));
        dialog.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::R);
        write_png(&mut dialog, &format!("render-running-{suffix}"));
        let (mut idle, _, _dir) = harness_with(size, mixer_snapshot());
        idle.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::R);
        write_png(&mut idle, &format!("render-idle-{suffix}"));
    }

    let failed = json!({"name": "demo-song", "range": "project", "progress": 0.42,
        "state": "failed", "file": file, "error": "No space left on device (os error 28)"});
    let (mut dialog, _, _dir) = harness_with(WINDOW, render_snapshot(failed));
    click_button(&mut dialog, "Render");
    dialog.run_steps(2);
    dialog.get_by_role_and_label(Role::Alert, "No space left on device (os error 28)");
    write_png(&mut dialog, "render-failed");
}

#[test]
fn transport_bar_fits_one_row_at_1280() {
    let (harness, _, _dir) = harness_with(DEFAULT_WINDOW, mixer_snapshot());
    let play = harness.get_by_role_and_label(Role::Button, "▶ Play").rect();
    for label in ["Mixer", "Render", "3.2.60", "0:04.800"] {
        let rect = harness.get_by_label(label).rect();
        assert!(
            play.y_range().contains(rect.center().y) && rect.right() <= DEFAULT_WINDOW.x,
            "{label} at {rect:?} is not on Play's row in the window"
        );
    }
}

const PERMISSION_ERROR: &str = "bank/funk.toml: permission denied (os error 13)";
const SAVE_FAILED: &str = "Could not save Preset";

fn error_snapshot() -> Snapshot {
    let mut snapshot = funk_snapshot();
    snapshot.last_message = Some((
        Message::Error {
            title: SAVE_FAILED,
            detail: PERMISSION_ERROR.to_string(),
        },
        std::time::Instant::now(),
    ));
    snapshot
}

#[test]
fn an_error_shows_as_a_toast_at_the_bottom_right_until_dismissed() {
    for (size, suffix) in [(DEFAULT_WINDOW, "1280x800"), (MIN_WINDOW, "1024x640")] {
        let (mut harness, _, _dir) = harness_with(size, error_snapshot());
        let toast = harness.get_by_label(PERMISSION_ERROR).rect();
        assert!(
            toast.left() > size.x / 2.0 && toast.bottom() > size.y / 2.0,
            "the toast is at {toast:?}"
        );
        harness.get_by_role_and_label(Role::Alert, &format!("{SAVE_FAILED}. {PERMISSION_ERROR}"));
        write_png(&mut harness, &format!("error-toast-{suffix}"));
        click_button(&mut harness, "Dismiss");
        assert!(harness.query_by_label(PERMISSION_ERROR).is_none());
    }
}

#[test]
fn esc_dismisses_the_error_toast() {
    let (mut harness, _, _dir) = harness_with(DEFAULT_WINDOW, error_snapshot());
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert!(harness.query_by_label(SAVE_FAILED).is_none());
}

#[test]
fn a_unit_chevron_is_a_full_pointer_target() {
    let (harness, _, _dir) = harness();
    let chevron = harness.get_all_by_label(CLOSED).next().unwrap().rect();
    assert!(
        chevron.width() >= 24.0 && chevron.height() >= 24.0,
        "the chevron is {chevron:?}"
    );
}

#[test]
fn a_lost_daemon_shows_its_banner_and_pauses_the_rack() {
    let mut snapshot = funk_snapshot();
    snapshot.link = Link::Disconnected {
        since: std::time::Instant::now(),
    };
    snapshot.status = None;
    snapshot.tuner = None;
    let (mut small, _, _dir) = harness_with(MIN_WINDOW, snapshot.clone());
    write_png(&mut small, "disconnected-1024x640");
    let (mut harness, sent, _dir) = harness_with(DEFAULT_WINDOW, snapshot);
    harness.get_by_label_contains("Daemon disconnected");
    write_png(&mut harness, "disconnected-1280x800");
    harness
        .get_by_role_and_label(Role::Switch, "Compressor")
        .click();
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), []);
}

const SAVED: &str = "saved bank/funk.toml";

/// The footer's top, read off the [`SAVED`] notice it shows.
fn footer_top(harness: &Harness<App>) -> f32 {
    let text = harness.get_by_label(SAVED).rect();
    let margin = egui::Frame::side_top_panel(&harness.ctx.global_style()).inner_margin;
    text.top() - f32::from(margin.top)
}

#[test]
fn send_knob_names_its_level_with_the_unit() {
    let (mut harness, _, _dir) = harness_with(MIN_WINDOW, mixer_snapshot());
    click_button(&mut harness, "Mixer");
    let value = harness.get_by_label("Send Bass DI to Room").value();
    assert!(
        value.as_deref().is_some_and(|value| value.contains("dB")),
        "the Send knob's value is {value:?}"
    );
}

#[test]
fn mixer_faders_and_switches_show_whole_in_the_mixer_view() {
    for (size, snapshot, last_whole, fader_height) in [
        (MIN_WINDOW, mixer_snapshot(), "Room", 240.0),
        (WINDOW, mixer_snapshot(), "Delay", 320.0),
        (FULL_HD_WINDOW, mixer_snapshot(), "Delay", 320.0),
        (DEFAULT_WINDOW, mixer_snapshot(), "Delay", 320.0),
        (DEFAULT_WINDOW, eight_buses_snapshot(), "Delay", 232.0),
    ] {
        let mut snapshot = snapshot;
        snapshot.last_message = Some((
            Message::Notice(SAVED.to_string()),
            std::time::Instant::now(),
        ));
        let (mut harness, _, _dir) = harness_with(size, snapshot);
        click_button(&mut harness, "Mixer");
        let footer_top = footer_top(&harness);
        let fader = harness.get_by_label("Volume Bass DI").rect().height();
        assert!(
            (fader - fader_height).abs() <= 2.0,
            "the fader at {size:?} is {fader} px, not {fader_height}"
        );
        // Master's name sits its strip's margin and edge (7 px) inside its strip, which sits two gaps and the rule (17 px) right of the strips' viewport.
        let scroll_right = harness.get_by_label("Rack of Master").rect().left() - 24.0;
        for label in [
            format!("Rack of {last_whole}"),
            format!("Solo {last_whole}"),
        ] {
            let right = harness.get_by_label(&label).rect().right();
            assert!(
                right <= scroll_right,
                "{label} at {size:?} ends at {right}, past the strips' edge at {scroll_right}"
            );
        }
        for name in ["Room", "Bass DI", "Bass Amp", "Click", "Master"] {
            let mut labels = vec![format!("Volume {name}"), format!("Volume {name} value")];
            if name != "Master" {
                labels.push(format!("Solo {name}"));
            }
            for label in labels {
                for node in harness.get_all_by_label(&label) {
                    let bottom = node.rect().bottom();
                    assert!(
                        bottom < footer_top,
                        "{label} at {size:?} ends at {bottom}, below the footer at {footer_top}"
                    );
                }
            }
        }
    }
}

#[test]
fn dragging_the_arrange_split_resizes_the_rack() {
    let (mut harness, _, _dir) = harness_with(WINDOW, mixer_snapshot());
    let heading = harness.get_by_label("Bass DI — Rack").rect().top();
    let edge = pos2(700.0, heading - 24.0);
    drag_and_release(&mut harness, edge, edge - Vec2::new(0.0, 100.0));
    let moved = harness.get_by_label("Bass DI — Rack").rect().top();
    assert!(
        (heading - 100.0 - moved).abs() < 8.0,
        "the Rack heading moved from {heading} to {moved}"
    );
}

#[test]
fn mixer_columns_send_their_commands() {
    let (mut harness, sent, _dir) = harness_with(WINDOW, mixer_snapshot());
    harness.key_press_modifiers(Modifiers::COMMAND, Key::M);
    harness.run_steps(2);
    for label in [
        "Clip Bass DI",
        "pre Bass DI Room",
        "to Master Click",
        "Solo Room",
        "+ Add Bus",
        "Rack of Room",
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
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    harness.run_steps(2);
    assert!(harness.query_by_label("Room — Rack").is_some());
    click_button(&mut harness, "cabinet (mono_to_stereo)");
    assert_eq!(sent_now(&sent), []);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::M);
    harness.run_steps(2);
    assert!(harness.query_by_label("Rack of Master").is_some());
    harness.key_press_modifiers(Modifiers::COMMAND, Key::M);
    harness.run_steps(2);
    assert_eq!(sent_now(&sent), []);
    assert!(harness.query_by_label("Rack of Master").is_none());
    assert!(harness.query_by_label("Room — Rack").is_some());
}

#[test]
fn enter_on_a_strip_name_shows_its_rack_view_and_a_click_stays_in_the_mixer() {
    let (mut harness, sent, _dir) = harness_with(DEFAULT_WINDOW, mixer_snapshot());
    let mixer = |harness: &Harness<App>| harness.query_by_label("Rack of Master").is_some();
    click_button(&mut harness, "Mixer");
    click_button(&mut harness, "Rack of Master");
    assert!(mixer(&harness));
    harness.get_by_label("Rack of Room").focus();
    harness.run_steps(1);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert!(!mixer(&harness));
    assert!(harness.query_by_label("Room — Rack").is_some());
    assert!(sent_now(&sent).ends_with(&[Command::ShowRack(ShownRack::Bus(0))]));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::M);
    harness.run_steps(2);
    assert!(mixer(&harness));
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

#[test]
fn clicking_the_selected_track_shows_its_rack_again() {
    let (mut harness, sent, _dir) = harness_with(DEFAULT_WINDOW, mixer_snapshot());
    click_button(&mut harness, "Mixer");
    click_button(&mut harness, "Rack of Room");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    harness.run_steps(2);
    assert!(harness.query_by_label("Room — Rack").is_some());
    sent_now(&sent);
    harness.get_by_label("Track Bass DI").click();
    harness.run_steps(2);
    assert!(sent_now(&sent).contains(&Command::ShowRack(ShownRack::Track)));
    assert!(harness.query_by_label("Bass DI — Rack").is_some());
    assert!(harness.query_by_label("Room — Rack").is_none());
}
