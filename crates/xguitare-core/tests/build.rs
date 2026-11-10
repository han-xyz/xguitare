use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use xguitare_core::build::{build_arrangement, build_rack, swap_arrangement};
use xguitare_core::engine::{Command, engine_queues};
use xguitare_core::firstrun::ensure_bank;
use xguitare_core::project::{Item, LoopRegion, Project, Take, Track};
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

fn project_with(tracks: Vec<Track>) -> Project {
    Project {
        name: "p".to_string(),
        sample_rate: 48_000,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 0.0,
        click: false,
        count_in: 0,
        tracks,
    }
}

#[test]
fn arrangement_builds_one_rack_per_track_and_swaps_in() {
    let dir = temp_dir("arrangement");
    let config = config(&dir.0);
    let preset = Preset::parse("name = \"G\"\n[[module]]\ntype = \"gain\"\n").unwrap();
    let mut with_item = Track::from_preset("b", &preset);
    with_item.items.push(Item {
        position: 0,
        offset: 0,
        take: 0,
        takes: vec![Take {
            file: "takes/b-1.wav".to_string(),
            frames: 2,
        }],
    });
    let project = project_with(vec![Track::from_preset("a", &preset), with_item]);
    let takes = HashMap::from([("takes/b-1.wav".to_string(), Arc::from(vec![0.5, 0.5]))]);
    let (mut arrangement, warnings) =
        build_arrangement(&config, &project, &takes, 48_000.0, 128).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(arrangement.take_readers().len(), 2);

    let (mut control, mut audio, _tap) = engine_queues(4);
    swap_arrangement(&mut control, arrangement).unwrap();
    assert!(matches!(
        audio.commands.pop(),
        Ok(Command::SwapArrangement(_))
    ));
}

#[test]
fn arrangement_with_an_unloaded_take_is_an_error() {
    let dir = temp_dir("arrangement-unloaded");
    let config = config(&dir.0);
    let mut t = Track::from_preset("a", &Preset::parse("name = \"G\"\n").unwrap());
    t.items.push(Item {
        position: 0,
        offset: 0,
        take: 0,
        takes: vec![Take {
            file: "takes/a-1.wav".to_string(),
            frames: 2,
        }],
    });
    let error = build_arrangement(
        &config,
        &project_with(vec![t]),
        &HashMap::new(),
        48_000.0,
        128,
    )
    .err()
    .unwrap();
    assert_eq!(error.to_string(), "take 'takes/a-1.wav' is not loaded");
}
