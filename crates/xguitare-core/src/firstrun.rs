use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};
use xguitare_dsp::synth_ir::{bass_cabinet, default_cabinet};

use crate::BankKind;

pub(crate) const DEFAULT_IR: &str = "default-cab.wav";
pub(crate) const BASS_IR: &str = "bass-cab.wav";
const STAGING_SUFFIX: &str = ".xguitare-staging";
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
const BASS_PRESETS: [(&str, &str); 5] = [
    ("01-blues.toml", include_str!("../bank-bass/01-blues.toml")),
    ("02-funk.toml", include_str!("../bank-bass/02-funk.toml")),
    (
        "03-primus-slap.toml",
        include_str!("../bank-bass/03-primus-slap.toml"),
    ),
    (
        "04-primus-fuzz.toml",
        include_str!("../bank-bass/04-primus-fuzz.toml"),
    ),
    (
        "05-primus-flange.toml",
        include_str!("../bank-bass/05-primus-flange.toml"),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Created {
    pub presets: usize,
    pub ir: bool,
}

/// Creates `bank_dir` with the starter Presets of `kind` and their cabinet IR synthesized at `sample_rate`, unless `bank_dir` already exists; an existing directory is left untouched and the result is all zeros. The files are written into `.<bank name>.xguitare-staging` beside it (a stale one is replaced) and renamed into place, so an interrupted first run leaves no partial Bank.
pub fn ensure_bank(
    bank_dir: &Path,
    kind: BankKind,
    sample_rate: u32,
) -> Result<Created, io::Error> {
    if bank_dir.exists() {
        return Ok(Created {
            presets: 0,
            ir: false,
        });
    }
    let staging = staging_dir(bank_dir)?;
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let (presets, ir_file, ir) = match kind {
        BankKind::Guitar => (STARTER_PRESETS, DEFAULT_IR, default_cabinet(sample_rate)),
        BankKind::Bass => (BASS_PRESETS, BASS_IR, bass_cabinet(sample_rate)),
    };
    for (file, text) in presets {
        fs::write(staging.join(file), text)?;
    }
    write_ir(&staging.join(ir_file), &ir, sample_rate).map_err(io::Error::other)?;
    fs::rename(&staging, bank_dir)?;
    Ok(Created {
        presets: presets.len(),
        ir: true,
    })
}

fn staging_dir(bank_dir: &Path) -> Result<PathBuf, io::Error> {
    let name = bank_dir.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("bank dir {} has no final name", bank_dir.display()),
        )
    })?;
    let mut staging = OsString::from(".");
    staging.push(name);
    staging.push(STAGING_SUFFIX);
    Ok(bank_dir.with_file_name(staging))
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
