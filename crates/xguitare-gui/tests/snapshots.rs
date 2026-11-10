use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Role, Vec2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_system_font_provider::SystemFontProvider;
use serde_json::{Value, json};
use xguitare_core::{BankKind, Config};
use xguitare_gui::app::App;
use xguitare_gui::model::{Command, Link, ModuleType, Restore, Snapshot};

const WINDOW: Vec2 = Vec2::new(1440.0, 900.0);
const DEFAULT_WINDOW: Vec2 = Vec2::new(1280.0, 800.0);
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
        })),
        module_types,
        irs: vec!["bass-cab.wav".to_string(), "guitar-cab.wav".to_string()],
        last_message: None,
        failed_set: None,
        refused_remove: None,
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
            values,
            bypass: false,
            ir: None,
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
    for title in ["Preset", "Bass Bank", "Audio", "Settings"] {
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
            },
        ]
    );
}

#[test]
fn a_tile_click_adds_at_the_chosen_place() {
    let (mut harness, sent, _dir) = harness();
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
