use std::path::Path;

use xguitare_core::{BuildWarning, Preset, PresetModule};

fn error(text: &str) -> String {
    Preset::parse(text).unwrap_err().to_string()
}

#[test]
fn syntax_error_reports_line_and_col() {
    let message = error("name = \"Crunch\"\n\n[[module]\ntype = \"gain\"\n");
    assert!(message.starts_with("line 3:"), "{message}");
}

#[test]
fn unknown_module_type() {
    let message = error("name = \"X\"\n[[module]]\ntype = \"fuzz\"\n");
    assert_eq!(message, "unknown module type 'fuzz'");
}

#[test]
fn unknown_param() {
    let message = error("name = \"X\"\n[[module]]\ntype = \"gain\"\n[module.params]\ndrive = 1\n");
    assert_eq!(message, "module 'gain' has no parameter 'drive'");
}

#[test]
fn duplicate_id() {
    let message = error("name = \"X\"\n[[module]]\ntype = \"gain\"\n[[module]]\ntype = \"gain\"\n");
    assert_eq!(message, "duplicate module id 'gain'");
}

#[test]
fn cabinet_without_ir() {
    let message = error("name = \"X\"\n[[module]]\ntype = \"cabinet\"\nid = \"cab\"\n");
    assert_eq!(message, "cabinet 'cab' has no ir");
}

#[test]
fn integer_param_value() {
    let preset =
        Preset::parse("name = \"X\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 3\n")
            .unwrap();
    assert_eq!(preset.modules[0].params, vec![("level".to_string(), 3.0)]);
}

#[test]
fn omitted_params_take_defaults() {
    let preset =
        Preset::parse("name = \"X\"\n[[module]]\ntype = \"gain\"\nid = \"boost\"\n").unwrap();
    assert_eq!(
        preset.modules,
        vec![PresetModule {
            id: "boost".to_string(),
            type_name: "gain".to_string(),
            ir: None,
            params: vec![("level".to_string(), 0.0)],
        }]
    );
}

#[test]
fn to_toml_round_trips() {
    let preset = Preset::parse(
        "name = \"Crunch\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 2.5\n\
         [[module]]\ntype = \"cabinet\"\nir = \"default-cab.wav\"\n",
    )
    .unwrap();
    assert_eq!(Preset::parse(&preset.to_toml()).unwrap(), preset);
}

#[test]
fn build_gain_preset_runs() {
    let preset =
        Preset::parse("name = \"X\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = -6\n")
            .unwrap();
    let (mut rack, warnings) = preset.build(48_000.0, 128, Path::new("."), 100.0).unwrap();
    assert!(warnings.is_empty());
    let input = [0.5; 128];
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    rack.process(&input, &mut left, &mut right);
    assert!(left[0] < 0.5 && left[0] > 0.0);
}

#[test]
fn missing_ir_builds_passthrough_with_warning() {
    let preset = Preset::parse(
        "name = \"X\"\n[[module]]\ntype = \"cabinet\"\nid = \"cab\"\nir = \"absent.wav\"\n",
    )
    .unwrap();
    let bank_dir = std::env::temp_dir().join(format!("xguitare-preset-{}-ir", std::process::id()));
    let (mut rack, warnings) = preset.build(48_000.0, 128, &bank_dir, 100.0).unwrap();
    assert_eq!(warnings.len(), 1);
    let BuildWarning::Ir { id, error } = &warnings[0];
    assert_eq!(id, "cab");
    assert!(error.contains("absent.wav"), "{error}");
    let input: Vec<f32> = (0..128).map(|n| n as f32 / 128.0).collect();
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    rack.process(&input, &mut left, &mut right);
    assert_eq!(left.as_slice(), input.as_slice());
    assert_eq!(right.as_slice(), input.as_slice());
}
