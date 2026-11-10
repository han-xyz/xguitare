use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Role, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_system_font_provider::SystemFontProvider;
use serde_json::{Value, json};
use xguitare_core::Config;
use xguitare_gui::app::App;
use xguitare_gui::model::{Link, ModuleType, Snapshot};

const WINDOW: Vec2 = Vec2::new(1440.0, 900.0);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn bass_config() -> Config {
    let dir = TempDir(PathBuf::from(format!(
        "/tmp/xguitare-snap-{}",
        std::process::id()
    )));
    let _ = fs::remove_dir_all(&dir.0);
    fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("config.toml");
    fs::write(&path, "bank_kind = \"bass\"\n").unwrap();
    Config::load(Some(&path)).unwrap()
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
    let image = harness.render().unwrap();
    let dir = snapshot_dir();
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.png"));
    image.save(&path).unwrap();
    assert!(path.exists(), "{} was not written", path.display());
}

fn harness() -> Harness<'static, App> {
    let config = bass_config();
    let mut harness = Harness::builder()
        .with_size(WINDOW)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| ctx.add_font_provider(Arc::new(SystemFontProvider::new())))
        .wgpu()
        .build_eframe(move |cc| App::with_snapshot(cc, &config, funk_snapshot()));
    harness.run_steps(2);
    harness
}

#[test]
fn gui_snapshots() {
    write_png(&mut harness(), "default");

    let mut two_open = harness();
    for _ in 0..2 {
        two_open
            .get_all_by_label("▶")
            .next()
            .unwrap()
            .click_accesskit();
        two_open.run_steps(2);
    }
    write_png(&mut two_open, "two-units-open");

    let mut tuner = harness();
    tuner.get_by_role_and_label(Role::Button, "Tuner").click();
    write_png(&mut tuner, "tuner-open");

    let mut save_as = harness();
    save_as.get_by_label("Save as new").click();
    write_png(&mut save_as, "save-as-open");
}
