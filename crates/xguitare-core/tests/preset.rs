use std::fs;
use std::path::{Path, PathBuf};

use xguitare_core::{BuildWarning, Preset, PresetError, PresetModule};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-preset-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn assert_passthrough_with_ir_warning(bank_dir: &Path, file: &str) {
    let preset = Preset::parse(&format!(
        "name = \"X\"\n[[module]]\ntype = \"cabinet\"\nid = \"cab\"\nir = \"{file}\"\n"
    ))
    .unwrap();
    let (mut rack, warnings) = preset.build(48_000.0, 128, bank_dir, 100.0).unwrap();
    assert_eq!(warnings.len(), 1);
    let BuildWarning::Ir { id, error } = &warnings[0];
    assert_eq!(id, "cab");
    assert!(error.contains(file), "{error}");
    let input: Vec<f32> = (0..128).map(|n| n as f32 / 128.0).collect();
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    rack.process(&input, &mut left, &mut right);
    assert_eq!(left.as_slice(), input.as_slice());
    assert_eq!(right.as_slice(), input.as_slice());
}

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
    let (mut rack, warnings) = preset.build(48_000.0, 128, Path::new("."), 100.0).unwrap();
    assert!(warnings.is_empty());
    let input = [0.5; 128];
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    rack.process(&input, &mut left, &mut right);
    assert!(left[0] < 0.5 && left[0] > 0.0);
}

#[test]
fn missing_ir_builds_passthrough_with_warning() {
    let dir = temp_dir("missing-ir");
    assert_passthrough_with_ir_warning(&dir.0, "absent.wav");
}

#[test]
fn zero_length_ir_builds_passthrough_with_warning() {
    let dir = temp_dir("empty-ir");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    hound::WavWriter::create(dir.0.join("empty.wav"), spec)
        .unwrap()
        .finalize()
        .unwrap();
    assert_passthrough_with_ir_warning(&dir.0, "empty.wav");
}

#[test]
fn full_chain_parses_and_builds() {
    let preset = Preset::parse(include_str!("fixtures/full-chain.toml")).unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let (mut rack, warnings) = preset.build(48_000.0, 128, &fixtures, 100.0).unwrap();
    assert!(
        matches!(warnings.as_slice(), [BuildWarning::Ir { id, .. }] if id == "cabinet"),
        "{warnings:?}"
    );
    let types: Vec<&str> = rack
        .modules()
        .iter()
        .map(|(_, module)| module.spec().type_name)
        .collect();
    assert_eq!(
        types,
        [
            "gate",
            "drive",
            "preamp",
            "tonestack",
            "cabinet",
            "delay",
            "reverb"
        ]
    );
    let input = [0.25; 128];
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    rack.process(&input, &mut left, &mut right);
    assert!(left.iter().chain(&right).all(|s| s.is_finite()));
}

#[test]
fn layout_error_from_rack() {
    let cabinet = "[[module]]\ntype = \"cabinet\"\nir = \"cab.wav\"\n";
    let stereo_first = format!("name = \"X\"\n[[module]]\ntype = \"delay\"\n{cabinet}");
    let mono_last = format!("name = \"X\"\n{cabinet}[[module]]\ntype = \"gain\"\n");
    for (text, expected) in [
        (stereo_first, "stereo module 'delay' before the cabinet"),
        (mono_last, "mono module 'gain' after the cabinet"),
    ] {
        let error = Preset::parse(&text).unwrap_err();
        assert!(matches!(error, PresetError::Layout(_)), "{error:?}");
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn build_starts_at_preset_values() {
    let preset = Preset::parse(
        "name = \"X\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 6.0206\n",
    )
    .unwrap();
    let (mut rack, _) = preset.build(48_000.0, 128, Path::new("."), 100.0).unwrap();
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

#[test]
fn ir_path_outside_bank_is_error() {
    let cabinet = |ir: &str| {
        Preset::parse(&format!(
            "name = \"X\"\n[[module]]\ntype = \"cabinet\"\nid = \"cab\"\nir = \"{ir}\"\n"
        ))
    };
    for ir in [
        "/etc/passwd",
        "../x.wav",
        "a/../b.wav",
        "./x.wav",
        "irs/./cab.wav",
        "",
    ] {
        let error = cabinet(ir).unwrap_err();
        assert_eq!(
            error.to_string(),
            "cabinet 'cab' ir must be a relative path inside the bank directory",
            "{ir}"
        );
    }
    let preset = cabinet("irs/cab.wav").unwrap();
    assert_eq!(preset.modules[0].ir.as_deref(), Some("irs/cab.wav"));
}
