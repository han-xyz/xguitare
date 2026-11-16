use std::fs;
use std::path::PathBuf;

use xguitare_core::firstrun::{Created, ensure_bank};
use xguitare_core::{Bank, BankEntry, BankKind, ModuleRegistry};

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
fn guitar_bank_seeds_thirteen_presets_and_ir() {
    let temp = temp_dir("missing");
    let dir = temp.0.join("bank");
    let created = ensure_bank(&dir, BankKind::Guitar, 48_000).unwrap();
    assert_eq!(
        created,
        Created {
            presets: 13,
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
            "06-rock.toml",
            "07-blues.toml",
            "08-funk.toml",
            "09-thrash.toml",
            "10-djent.toml",
            "11-doom.toml",
            "12-buzzsaw.toml",
            "13-stoner.toml",
            "default-cab.wav"
        ]
    );
    let racks = built_racks(&dir);
    let names: Vec<&str> = racks.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Clean",
            "Crunch",
            "Lead",
            "High Gain",
            "Bass",
            "Rock",
            "Blues",
            "Funk",
            "Thrash",
            "Djent",
            "Doom",
            "Buzzsaw",
            "Stoner"
        ]
    );
    let types: Vec<&[&str]> = racks[8..]
        .iter()
        .map(|(_, types)| types.as_slice())
        .collect();
    assert_eq!(
        types,
        [
            &["gate", "drive", "preamp", "eq", "cabinet"][..],
            &["gate", "drive", "preamp", "eq", "limiter", "cabinet"],
            &["gate", "fuzz", "preamp", "tonestack", "cabinet", "reverb"],
            &["gate", "drive", "eq", "preamp", "cabinet"],
            &[
                "gate",
                "drive",
                "fuzz",
                "preamp",
                "tonestack",
                "cabinet",
                "delay",
                "reverb"
            ],
        ]
    );
}

fn built_racks(dir: &std::path::Path) -> Vec<(String, Vec<&'static str>)> {
    Bank::load_dir(dir)
        .unwrap()
        .entries
        .iter()
        .map(|entry| {
            let BankEntry::Valid { preset, .. } = entry else {
                panic!("invalid starter preset: {entry:?}");
            };
            let (rack, warnings) = preset.build(48_000.0, 128, dir, dir, 100.0).unwrap();
            assert!(warnings.is_empty(), "{}: {warnings:?}", preset.name);
            let types = rack
                .modules()
                .iter()
                .map(|(_, module)| module.spec().type_name)
                .collect();
            (preset.name.clone(), types)
        })
        .collect()
}

#[test]
fn bass_bank_seeds_thirteen_presets_and_ir() {
    let temp = temp_dir("bass");
    let dir = temp.0.join("bank-bass");
    let created = ensure_bank(&dir, BankKind::Bass, 48_000).unwrap();
    assert_eq!(
        created,
        Created {
            presets: 13,
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
            "01-blues.toml",
            "02-funk.toml",
            "03-primus-slap.toml",
            "04-primus-fuzz.toml",
            "05-primus-flange.toml",
            "06-rock.toml",
            "07-envelope-funk.toml",
            "08-modern-grind.toml",
            "09-thrash.toml",
            "10-djent.toml",
            "11-doom.toml",
            "12-buzzsaw.toml",
            "13-stoner.toml",
            "bass-cab.wav"
        ]
    );
    let racks = built_racks(&dir);
    let names: Vec<&str> = racks.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Blues",
            "Funk",
            "Primus Slap",
            "Primus Fuzz",
            "Primus Flange",
            "Rock",
            "Envelope Funk",
            "Modern Grind",
            "Thrash",
            "Djent",
            "Doom",
            "Buzzsaw",
            "Stoner"
        ]
    );
    let types: Vec<&[&str]> = racks[6..]
        .iter()
        .map(|(_, types)| types.as_slice())
        .collect();
    assert_eq!(
        types,
        [
            &["compressor", "autowah", "eq", "cabinet"][..],
            &["eq", "blend", "limiter", "cabinet"],
            &["compressor", "blend", "eq", "limiter", "cabinet"],
            &["gate", "compressor", "eq", "blend", "limiter", "cabinet"],
            &["blend", "eq", "limiter", "cabinet"],
            &["gate", "blend", "eq", "limiter", "cabinet"],
            &["compressor", "blend", "eq", "limiter", "cabinet"],
        ]
    );
    let samples: Vec<f32> = hound::WavReader::open(dir.join("bass-cab.wav"))
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    let expected = xguitare_dsp::synth_ir::bass_cabinet(48_000);
    assert_eq!(samples.len(), expected.len());
    for (got, want) in samples.iter().zip(&expected) {
        assert!((got - want).abs() < 1e-6);
    }
}

#[test]
fn leaves_existing_bank_alone() {
    let temp = temp_dir("existing");
    let dir = temp.0.join("bank");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mine.toml");
    fs::write(&file, "name = \"Mine\"\n").unwrap();
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    let created = ensure_bank(&dir, BankKind::Guitar, 48_000).unwrap();
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
    let staging = temp.0.join(".bank.xguitare-staging");
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("01-clean.toml"), "name = \"Half\"\n").unwrap();
    let created = ensure_bank(&dir, BankKind::Guitar, 48_000).unwrap();
    assert_eq!(
        created,
        Created {
            presets: 13,
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
            "06-rock.toml",
            "07-blues.toml",
            "08-funk.toml",
            "09-thrash.toml",
            "10-djent.toml",
            "11-doom.toml",
            "12-buzzsaw.toml",
            "13-stoner.toml",
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
    let unrelated = temp.0.join("bank.tmp");
    fs::create_dir_all(&unrelated).unwrap();
    let created = ensure_bank(&with_slash, BankKind::Guitar, 48_000).unwrap();
    assert_eq!(created.presets, 13);
    assert_eq!(Bank::load_dir(&dir).unwrap().entries.len(), 13);
    let mut siblings: Vec<String> = fs::read_dir(&temp.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    siblings.sort();
    assert_eq!(siblings, ["bank", "bank.tmp"]);
}

#[test]
fn starter_banks_build_without_warnings() {
    let mut built = 0;
    for (test, kind) in [
        ("starter-guitar", BankKind::Guitar),
        ("starter-bass", BankKind::Bass),
    ] {
        let temp = temp_dir(test);
        let dir = temp.0.join("bank");
        ensure_bank(&dir, kind, 48_000).unwrap();
        for entry in &Bank::load_dir(&dir).unwrap().entries {
            let BankEntry::Valid { preset, .. } = entry else {
                panic!("invalid starter preset: {entry:?}");
            };
            let (_, warnings) = preset.build(48_000.0, 128, &dir, &dir, 100.0).unwrap();
            assert!(warnings.is_empty(), "{}: {warnings:?}", preset.name);
            built += 1;
        }
    }
    assert_eq!(built, 26);
}

const BASS_NOTES_HZ: [f32; 4] = [41.2, 55.0, 73.4, 98.0];
const GUITAR_NOTES_HZ: [f32; 4] = [82.4, 110.0, 146.8, 196.0];

fn plucked(notes_hz: &[f32], sample_rate: f32, seconds: f32, rms_db: f32) -> Vec<f32> {
    const PLUCK_SECONDS: f32 = 0.5;
    let frames = (sample_rate * seconds) as usize;
    let mut signal: Vec<f32> = (0..frames)
        .map(|n| {
            let t = n as f32 / sample_rate;
            let pluck = (t / PLUCK_SECONDS) as usize;
            let since = t - pluck as f32 * PLUCK_SECONDS;
            let freq = notes_hz[pluck % notes_hz.len()];
            let tone: f32 = (1..=8)
                .map(|k| (std::f32::consts::TAU * freq * k as f32 * since).sin() / k as f32)
                .sum();
            tone * (-since * 4.0).exp()
        })
        .collect();
    let rms = (signal.iter().map(|x| x * x).sum::<f32>() / frames as f32).sqrt();
    let scale = 10f32.powf(rms_db / 20.0) / rms;
    for x in &mut signal {
        *x *= scale;
    }
    signal
}

struct Level {
    rms_db: f32,
    peak: f32,
}

fn output_level(preset: &xguitare_core::Preset, dir: &std::path::Path, input: &[f32]) -> Level {
    const BLOCK: usize = 128;
    let (mut rack, _) = preset.build(48_000.0, BLOCK, dir, dir, 100.0).unwrap();
    let mut sum = 0.0f64;
    let mut peak = 0.0f32;
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    for block in input.as_chunks::<BLOCK>().0 {
        rack.process(block, &mut left, &mut right);
        sum += left
            .iter()
            .chain(&right)
            .map(|x| f64::from(x * x))
            .sum::<f64>();
        peak = left
            .iter()
            .chain(&right)
            .fold(peak, |max, x| max.max(x.abs()));
    }
    let frames = input.len() / BLOCK * BLOCK;
    Level {
        rms_db: (10.0 * (sum / (2 * frames) as f64).log10()) as f32,
        peak,
    }
}

const METAL_PRESETS: [&str; 5] = ["Thrash", "Djent", "Doom", "Buzzsaw", "Stoner"];

fn levels(kind: BankKind, names: &[&str], notes_hz: &[f32]) -> Vec<Level> {
    let temp = temp_dir(&format!("loudness-{}", names[0]));
    let dir = temp.0.join("bank");
    ensure_bank(&dir, kind, 48_000).unwrap();
    let bank = Bank::load_dir(&dir).unwrap();
    let input = plucked(notes_hz, 48_000.0, 4.0, -18.0);
    names
        .iter()
        .map(|name| {
            let preset = bank
                .entries
                .iter()
                .find_map(|entry| match entry {
                    BankEntry::Valid { preset, .. } if preset.name == *name => Some(preset),
                    _ => None,
                })
                .unwrap();
            output_level(preset, &dir, &input)
        })
        .collect()
}

fn assert_within_2_db(name: &str, level: &Level, reference: &str, reference_level: &Level) {
    assert!(
        (level.rms_db - reference_level.rms_db).abs() <= 2.0,
        "{name}: {:.1} dB, {reference} {:.1} dB",
        level.rms_db,
        reference_level.rms_db
    );
}

#[test]
fn new_bass_presets_play_as_loud_as_funk() {
    let names = ["Funk", "Envelope Funk", "Modern Grind"]
        .into_iter()
        .chain(METAL_PRESETS)
        .collect::<Vec<_>>();
    let levels = levels(BankKind::Bass, &names, &BASS_NOTES_HZ);
    for (name, level) in names.iter().zip(&levels).skip(1) {
        assert_within_2_db(name, level, "Funk", &levels[0]);
    }
    for (name, level) in names.iter().zip(&levels).skip(3) {
        assert!(level.peak < 1.0, "{name}: peak {:.3}", level.peak);
    }
}

#[test]
fn metal_guitar_presets_play_as_loud_as_high_gain() {
    let names = ["High Gain"]
        .into_iter()
        .chain(METAL_PRESETS)
        .collect::<Vec<_>>();
    let levels = levels(BankKind::Guitar, &names, &GUITAR_NOTES_HZ);
    for (name, level) in names.iter().zip(&levels).skip(1) {
        assert_within_2_db(name, level, "High Gain", &levels[0]);
        assert!(level.peak < 1.0, "{name}: peak {:.3}", level.peak);
    }
}

#[test]
fn starter_preset_values_lie_inside_param_specs() {
    let registry = ModuleRegistry::default();
    for (test, kind) in [
        ("ranges-guitar", BankKind::Guitar),
        ("ranges-bass", BankKind::Bass),
    ] {
        let temp = temp_dir(test);
        let dir = temp.0.join("bank");
        ensure_bank(&dir, kind, 48_000).unwrap();
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        files.sort();
        assert_eq!(files.len(), 13);
        for file in files {
            let preset: toml::Table = fs::read_to_string(&file).unwrap().parse().unwrap();
            let file = file.file_name().unwrap().to_string_lossy();
            for module in preset["module"].as_array().unwrap() {
                let type_name = module["type"].as_str().unwrap();
                let spec = registry.spec(type_name).unwrap();
                let Some(params) = module.get("params") else {
                    continue;
                };
                for (name, value) in params.as_table().unwrap() {
                    let param = spec
                        .params
                        .iter()
                        .find(|param| param.name == name)
                        .unwrap_or_else(|| panic!("{file}: unknown {type_name}.{name}"));
                    let value = value
                        .as_float()
                        .or_else(|| value.as_integer().map(|v| v as f64))
                        .unwrap() as f32;
                    assert!(
                        (param.min..=param.max).contains(&value),
                        "{file}: {type_name}.{name} = {value} outside {}..{}",
                        param.min,
                        param.max
                    );
                }
            }
        }
    }
}
