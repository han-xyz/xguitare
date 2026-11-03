use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};
use xguitare_dsp::synth_ir::default_cabinet;

const DEFAULT_IR: &str = "default-cab.wav";
const STARTER_PRESETS: [(&str, &str); 5] = [
    ("01-clean.toml", include_str!("../bank/01-clean.toml")),
    ("02-crunch.toml", include_str!("../bank/02-crunch.toml")),
    ("03-lead.toml", include_str!("../bank/03-lead.toml")),
    (
        "04-high-gain.toml",
        include_str!("../bank/04-high-gain.toml"),
    ),
    ("05-bass.toml", include_str!("../bank/05-bass.toml")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Created {
    pub presets: usize,
    pub ir: bool,
}

/// Creates `bank_dir` with the starter Presets and the default cabinet IR synthesized at `sample_rate`, unless `bank_dir` already exists; an existing directory is left untouched and the result is all zeros. The files are written into `<bank_dir>.tmp` (a stale one is replaced) and renamed into place, so an interrupted first run leaves no partial Bank.
pub fn ensure_bank(bank_dir: &Path, sample_rate: u32) -> Result<Created, io::Error> {
    if bank_dir.exists() {
        return Ok(Created {
            presets: 0,
            ir: false,
        });
    }
    let staging = staging_dir(bank_dir);
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    for (file, text) in STARTER_PRESETS {
        fs::write(staging.join(file), text)?;
    }
    write_ir(
        &staging.join(DEFAULT_IR),
        &default_cabinet(sample_rate),
        sample_rate,
    )
    .map_err(io::Error::other)?;
    fs::rename(&staging, bank_dir)?;
    Ok(Created {
        presets: STARTER_PRESETS.len(),
        ir: true,
    })
}

fn staging_dir(bank_dir: &Path) -> PathBuf {
    let mut name = bank_dir.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

fn write_ir(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), hound::Error> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec)?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()
}
