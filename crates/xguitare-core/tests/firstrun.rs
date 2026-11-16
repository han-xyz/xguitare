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
fn guitar_bank_seeds_fifteen_presets_and_ir() {
    let temp = temp_dir("missing");
    let dir = temp.0.join("bank");
    let created = ensure_bank(&dir, BankKind::Guitar, 48_000).unwrap();
    assert_eq!(
        created,
        Created {
            presets: 15,
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
            "14-sub-octave.toml",
            "15-distortion.toml",
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
            "Stoner",
            "Sub Octave",
            "Distortion"
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
                "octave",
                "drive",
                "fuzz",
                "preamp",
                "tonestack",
                "cabinet",
                "delay",
                "reverb"
            ],
            &["gate", "octave", "compressor", "preamp", "cabinet"],
            &["gate", "drive", "preamp", "tonestack", "cabinet", "reverb"],
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
fn bass_bank_seeds_fifteen_presets_and_ir() {
    let temp = temp_dir("bass");
    let dir = temp.0.join("bank-bass");
    let created = ensure_bank(&dir, BankKind::Bass, 48_000).unwrap();
    assert_eq!(
        created,
        Created {
            presets: 15,
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
            "14-synth-sub.toml",
            "15-distortion.toml",
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
            "Stoner",
            "Synth Sub",
            "Distortion"
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
            &["compressor", "octave", "blend", "eq", "limiter", "cabinet"],
            &["compressor", "octave", "eq", "limiter", "cabinet"],
            &["compressor", "blend", "limiter", "cabinet"],
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
            presets: 15,
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
            "14-sub-octave.toml",
            "15-distortion.toml",
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
    assert_eq!(created.presets, 15);
    assert_eq!(Bank::load_dir(&dir).unwrap().entries.len(), 15);
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
    assert_eq!(built, 30);
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

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK: usize = 128;

fn render(preset: &xguitare_core::Preset, dir: &std::path::Path, input: &[f32]) -> [Vec<f32>; 2] {
    let (mut rack, _) = preset.build(SAMPLE_RATE, BLOCK, dir, dir, 100.0).unwrap();
    let mut output = [
        Vec::with_capacity(input.len()),
        Vec::with_capacity(input.len()),
    ];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    for block in input.as_chunks::<BLOCK>().0 {
        rack.process(block, &mut left, &mut right);
        output[0].extend_from_slice(&left);
        output[1].extend_from_slice(&right);
    }
    output
}

struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

// ITU-R BS.1770-4 K-weighting at 48 kHz: the high shelf, then the high-pass.
const K_WEIGHTING: [Biquad; 2] = [
    Biquad {
        b: [
            1.535_124_859_586_97,
            -2.691_696_189_406_38,
            1.198_392_810_852_85,
        ],
        a: [-1.690_659_293_182_41, 0.732_480_774_215_85],
    },
    Biquad {
        b: [1.0, -2.0, 1.0],
        a: [-1.990_047_454_833_98, 0.990_072_250_366_21],
    },
];

fn k_weighted(channel: &[f32]) -> Vec<f64> {
    let mut signal: Vec<f64> = channel.iter().map(|&x| f64::from(x)).collect();
    for Biquad { b, a } in &K_WEIGHTING {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for x in &mut signal {
            let y = b[0] * *x + b[1] * x1 + b[2] * x2 - a[0] * y1 - a[1] * y2;
            (x2, x1, y2, y1) = (x1, *x, y1, y);
            *x = y;
        }
    }
    signal
}

fn short_term_max_lufs(channels: &[&[f32]]) -> f32 {
    let window = (3.0 * SAMPLE_RATE) as usize;
    let hop = (0.1 * SAMPLE_RATE) as usize;
    let squares: Vec<Vec<f64>> = channels
        .iter()
        .map(|channel| k_weighted(channel).iter().map(|x| x * x).collect())
        .collect();
    (0..=channels[0].len() - window)
        .step_by(hop)
        .map(|start| {
            let power: f64 = squares
                .iter()
                .map(|channel| channel[start..start + window].iter().sum::<f64>() / window as f64)
                .sum();
            (-0.691 + 10.0 * power.log10()) as f32
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

fn true_peak_db(channels: &[&[f32]]) -> f32 {
    let mut peak = 0.0f32;
    for channel in channels {
        let mut oversampler = xguitare_dsp::oversample::Oversampler4x::new();
        oversampler.prepare(BLOCK);
        for block in channel.chunks(BLOCK) {
            peak = oversampler
                .upsample(block)
                .iter()
                .fold(peak, |max, x| max.max(x.abs()));
        }
    }
    20.0 * peak.log10()
}

fn sine(freq_hz: f32, amplitude: f32, phase: f32) -> Vec<f32> {
    (0..(4.0 * SAMPLE_RATE) as usize)
        .map(|n| {
            amplitude * (std::f32::consts::TAU * freq_hz * n as f32 / SAMPLE_RATE + phase).sin()
        })
        .collect()
}

#[test]
fn meter_reads_a_1_khz_sine_at_minus_26_lufs() {
    let tone = sine(1000.0, 10f32.powf(-23.0 / 20.0), 0.0);
    let loudness = short_term_max_lufs(&[&tone]);
    assert!((loudness + 26.0).abs() <= 0.1, "{loudness:.3} LUFS");
}

#[test]
fn meter_sums_the_channels_power() {
    let tone = sine(1000.0, 10f32.powf(-23.0 / 20.0), 0.0);
    let loudness = short_term_max_lufs(&[&tone, &tone]);
    assert!((loudness + 23.0).abs() <= 0.1, "{loudness:.3} LUFS");
}

#[test]
fn meter_weights_100_hz_1_2_db_down() {
    let amplitude = 0.1f32;
    let unweighted = -0.691 + 10.0 * (amplitude * amplitude / 2.0).log10();
    let loudness = short_term_max_lufs(&[&sine(100.0, amplitude, 0.0)]);
    assert!(
        (loudness - unweighted + 1.2).abs() <= 0.3,
        "{loudness:.3} LUFS, unweighted {unweighted:.3}"
    );
}

#[test]
fn meter_reads_the_true_peak_between_samples() {
    let tone = sine(SAMPLE_RATE / 4.0, 0.5, std::f32::consts::FRAC_PI_4);
    let peak = true_peak_db(&[&tone]);
    assert!((peak + 6.0).abs() <= 0.3, "{peak:.3} dBTP");
}

struct Level {
    loudness_lufs: f32,
    true_peak_db: f32,
}

const TARGET_GUITAR_LUFS: f32 = -13.0;
const TARGET_BASS_LUFS: f32 = -14.0;
const TRUE_PEAK_CEILING_DB: f32 = -1.0;

fn bank_levels(kind: BankKind, notes_hz: &[f32]) -> Vec<(String, Level)> {
    let temp = temp_dir(&format!("loudness-{kind:?}"));
    let dir = temp.0.join("bank");
    ensure_bank(&dir, kind, 48_000).unwrap();
    let input = plucked(notes_hz, SAMPLE_RATE, 4.0, -18.0);
    Bank::load_dir(&dir)
        .unwrap()
        .entries
        .iter()
        .map(|entry| {
            let BankEntry::Valid { preset, .. } = entry else {
                panic!("invalid starter preset: {entry:?}");
            };
            let [left, right] = render(preset, &dir, &input);
            let channels = [left.as_slice(), right.as_slice()];
            let level = Level {
                loudness_lufs: short_term_max_lufs(&channels),
                true_peak_db: true_peak_db(&channels),
            };
            (preset.name.clone(), level)
        })
        .collect()
}

fn assert_matched_below_ceiling(kind: BankKind, notes_hz: &[f32], target_lufs: f32) {
    let levels = bank_levels(kind, notes_hz);
    assert_eq!(levels.len(), 15);
    let failures: Vec<String> = levels
        .iter()
        .filter_map(|(name, level)| {
            let Level {
                loudness_lufs,
                true_peak_db,
            } = *level;
            let clips = true_peak_db > TRUE_PEAK_CEILING_DB;
            let too_loud = loudness_lufs > target_lufs + 0.05;
            let too_quiet =
                loudness_lufs < target_lufs - 0.2 && true_peak_db < TRUE_PEAK_CEILING_DB - 0.2;
            (clips || too_loud || too_quiet).then(|| {
                format!(
                    "{name}: {loudness_lufs:.2} LUFS, {true_peak_db:.2} dBTP, target {target_lufs} LUFS"
                )
            })
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn guitar_presets_match_loudness_below_true_peak_ceiling() {
    assert_matched_below_ceiling(BankKind::Guitar, &GUITAR_NOTES_HZ, TARGET_GUITAR_LUFS);
}

#[test]
fn bass_presets_match_loudness_below_true_peak_ceiling() {
    assert_matched_below_ceiling(BankKind::Bass, &BASS_NOTES_HZ, TARGET_BASS_LUFS);
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
        assert_eq!(files.len(), 15);
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
