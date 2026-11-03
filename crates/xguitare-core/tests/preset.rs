use std::path::Path;

use xguitare_core::{Preset, PresetModule};

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
        "name = \"Crunch\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 0.1\n\
         [[module]]\ntype = \"cabinet\"\nir = \"default-cab.wav\"\n",
    )
    .unwrap();
    let text = preset.to_toml();
    assert!(text.contains("level = 0.1\n"), "{text}");
    assert_eq!(Preset::parse(&text).unwrap(), preset);
}

#[test]
fn build_gain_preset_runs() {
    let preset =
        Preset::parse("name = \"X\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = -6\n")
            .unwrap();
    let mut rack = preset.build(48_000.0, 128, Path::new(".")).unwrap();
    let input = [0.5; 128];
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    rack.process(&input, &mut left, &mut right);
    assert!(left[0] < 0.5 && left[0] > 0.0);
}

#[test]
fn build_starts_at_preset_values() {
    let preset = Preset::parse(
        "name = \"X\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 6.0206\n",
    )
    .unwrap();
    let mut rack = preset.build(48_000.0, 128, Path::new(".")).unwrap();
    let input: Vec<f32> = (0..128)
        .map(|i| 0.25 * (std::f32::consts::TAU * 1_000.0 * i as f32 / 48_000.0).sin())
        .collect();
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    rack.process(&input, &mut left, &mut right);
    let peak = |block: &[f32]| block.iter().fold(0.0_f32, |max, s| max.max(s.abs()));
    let ratio = peak(&left) / peak(&input);
    assert!((ratio - 2.0).abs() < 1e-3, "{ratio}");
}

#[test]
fn non_finite_param_is_error() {
    let message =
        error("name = \"X\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = nan\n");
    assert_eq!(
        message,
        "module 'gain' parameter 'level' is not a finite number"
    );
}

#[test]
fn out_of_range_param_is_clamped() {
    let preset =
        Preset::parse("name = \"X\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 100\n")
            .unwrap();
    assert_eq!(preset.modules[0].params, vec![("level".to_string(), 20.0)]);
}
