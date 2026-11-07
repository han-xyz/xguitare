use std::fs;
use std::path::{Path, PathBuf};

use xguitare_core::build::build_rack;
use xguitare_core::firstrun::ensure_bank;
use xguitare_core::{Bank, BankEntry, BankKind, Config, Preset};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-build-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn config(dir: &Path) -> Config {
    let path = dir.join("config");
    fs::write(&path, "").unwrap();
    let mut config = Config::load(Some(&path)).unwrap();
    config.bank_dir = dir.join("bank");
    config.ir_dir = dir.join("bank");
    config
}

#[test]
fn starter_preset_builds_without_warnings() {
    let dir = temp_dir("starter");
    let config = config(&dir.0);
    ensure_bank(&config.bank_dir, BankKind::Guitar, 48_000).unwrap();
    let bank = Bank::load_dir(&config.bank_dir).unwrap();
    let BankEntry::Valid { preset, .. } = &bank.entries[0] else {
        panic!("the first starter Preset should be valid");
    };
    let (_, warnings) = build_rack(&config, 48_000.0, 128, preset).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn missing_ir_warns_like_load() {
    let dir = temp_dir("missing-ir");
    let config = config(&dir.0);
    let preset =
        Preset::parse("name = \"Cab\"\n[[module]]\ntype = \"cabinet\"\nir = \"missing.wav\"\n")
            .unwrap();
    let (_, warnings) = build_rack(&config, 48_000.0, 128, &preset).unwrap();
    let texts: Vec<String> = warnings.iter().map(ToString::to_string).collect();
    assert_eq!(
        texts,
        [format!(
            "cabinet 'cabinet': {}: No such file or directory (os error 2)",
            config.bank_dir.join("missing.wav").display()
        )]
    );
}
