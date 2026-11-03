use std::fs;
use std::path::PathBuf;

use xguitare_core::firstrun::{Created, ensure_bank};
use xguitare_core::{Bank, BankEntry};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-firstrun-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    TempDir(dir)
}

#[test]
fn creates_bank_when_missing() {
    let temp = temp_dir("missing");
    let dir = temp.0.join("bank");
    let created = ensure_bank(&dir, 48_000).unwrap();
    assert_eq!(
        created,
        Created {
            presets: 5,
            ir: true
        }
    );
    let mut files: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "01-clean.toml",
            "02-crunch.toml",
            "03-lead.toml",
            "04-high-gain.toml",
            "05-bass.toml",
            "default-cab.wav"
        ]
    );
    let bank = Bank::load_dir(&dir).unwrap();
    let mut names = Vec::new();
    for entry in &bank.entries {
        let BankEntry::Valid { preset, .. } = entry else {
            panic!("invalid starter preset: {entry:?}");
        };
        let (_, warnings) = preset.build(48_000.0, 128, &dir, 100.0).unwrap();
        assert!(warnings.is_empty(), "{}: {warnings:?}", preset.name);
        names.push(preset.name.as_str());
    }
    assert_eq!(names, ["Clean", "Crunch", "Lead", "High Gain", "Bass"]);
}

#[test]
fn leaves_existing_bank_alone() {
    let temp = temp_dir("existing");
    let dir = temp.0.join("bank");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mine.toml");
    fs::write(&file, "name = \"Mine\"\n").unwrap();
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    let created = ensure_bank(&dir, 48_000).unwrap();
    assert_eq!(
        created,
        Created {
            presets: 0,
            ir: false
        }
    );
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), modified);
}

#[test]
fn interrupted_first_run_is_retried() {
    let temp = temp_dir("interrupted");
    let dir = temp.0.join("bank");
    let staging = temp.0.join("bank.tmp");
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("01-clean.toml"), "name = \"Half\"\n").unwrap();
    let created = ensure_bank(&dir, 48_000).unwrap();
    assert_eq!(
        created,
        Created {
            presets: 5,
            ir: true
        }
    );
    let mut files: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "01-clean.toml",
            "02-crunch.toml",
            "03-lead.toml",
            "04-high-gain.toml",
            "05-bass.toml",
            "default-cab.wav"
        ]
    );
    assert!(!staging.exists());
    let bank = Bank::load_dir(&dir).unwrap();
    assert!(matches!(&bank.entries[0], BankEntry::Valid { preset, .. } if preset.name == "Clean"));
}

#[test]
fn trailing_slash_bank_dir_still_creates_bank() {
    let temp = temp_dir("trailing");
    let dir = temp.0.join("bank");
    let with_slash = PathBuf::from(format!("{}/", dir.display()));
    let created = ensure_bank(&with_slash, 48_000).unwrap();
    assert_eq!(created.presets, 5);
    assert_eq!(Bank::load_dir(&dir).unwrap().entries.len(), 5);
    let mut siblings: Vec<String> = fs::read_dir(&temp.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    siblings.sort();
    assert_eq!(siblings, ["bank"]);
}
