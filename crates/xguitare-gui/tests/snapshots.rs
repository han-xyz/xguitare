use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};

use egui::{Key, Modifiers, Role, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_system_font_provider::SystemFontProvider;
use serde_json::{Value, json};
use xguitare_core::{BankKind, Config};
use xguitare_gui::app::App;
use xguitare_gui::model::{Command, Link, ModuleType, Snapshot};

const WINDOW: Vec2 = Vec2::new(1440.0, 900.0);
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
    ];
    params
        .iter_mut()
        .filter(|param| param["module"] == "cabinet")
        .for_each(|param| param["ir"] = json!("bass-cab.wav"));
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
    let (config, dir) = bass_config();
    let (commands, sent) = mpsc::channel();
    let mut harness = Harness::builder()
        .with_size(size)
        .with_context_setup(|ctx| ctx.add_font_provider(Arc::new(SystemFontProvider::new())))
        .wgpu()
        .build_eframe(move |cc| App::with_snapshot(cc, &config, funk_snapshot(), commands));
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
fn add_module_after_the_rack_rules_default() {
    let (mut harness, sent, _dir) = harness();
    harness.get_by_label("Add Module").click();
    harness.run_steps(2);
    harness.get_by_label("delay (stereo)").click();
    harness.run_steps(2);
    assert_eq!(
        harness
            .get_by_role_and_label(Role::ComboBox, "Place")
            .value()
            .as_deref(),
        Some("after chorus")
    );
    harness.get_by_label("Add").click();
    harness.run_steps(2);
    assert_eq!(
        sent_now(&sent),
        [Command::RackAdd {
            type_name: "delay".to_string(),
            after: Some("chorus".to_string()),
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
fn place_chosen_before_the_type_is_kept() {
    let (mut harness, sent, _dir) = harness();
    harness.get_by_label("Place").click();
    harness.run_steps(2);
    harness.get_by_label("at the front").click();
    harness.run_steps(2);
    harness.get_by_label("Add Module").click();
    harness.run_steps(2);
    harness.get_by_label("gate (mono)").click();
    harness.run_steps(2);
    harness.get_by_label("Add").click();
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
