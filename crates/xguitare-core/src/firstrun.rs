use std::ffi::OsString;
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};
use xguitare_dsp::synth_ir::{
    bass_cabinet, bass_cabinet_8x10, cabinet_2x12_alnico, cabinet_2x12_open, cabinet_4x10,
    cabinet_4x12, default_cabinet,
};

use xguitare_protocol::BankKind;

/// The sample rate the starter Cabinet IRs are synthesized at.
pub const STARTER_IR_RATE: u32 = 48_000;
pub(crate) const DEFAULT_IR: &str = "default-cab.wav";
pub(crate) const BASS_IR: &str = "bass-cab.wav";
pub(crate) const CAB_4X10_IR: &str = "cab-4x10.wav";
pub(crate) const CAB_2X12_OPEN_IR: &str = "cab-2x12-open.wav";
pub(crate) const CAB_2X12_ALNICO_IR: &str = "cab-2x12-alnico.wav";
pub(crate) const CAB_4X12_IR: &str = "cab-4x12.wav";
pub(crate) const BASS_CAB_8X10_IR: &str = "bass-cab-8x10.wav";
const STAGING_SUFFIX: &str = ".xguitare-staging";
const STARTER_PRESETS: &[(&str, &str)] = &[
    ("01-clean.toml", include_str!("../bank/01-clean.toml")),
    ("02-crunch.toml", include_str!("../bank/02-crunch.toml")),
    ("03-lead.toml", include_str!("../bank/03-lead.toml")),
    (
        "04-high-gain.toml",
        include_str!("../bank/04-high-gain.toml"),
    ),
    ("05-bass.toml", include_str!("../bank/05-bass.toml")),
    ("06-rock.toml", include_str!("../bank/06-rock.toml")),
    ("07-blues.toml", include_str!("../bank/07-blues.toml")),
    ("08-funk.toml", include_str!("../bank/08-funk.toml")),
    ("09-thrash.toml", include_str!("../bank/09-thrash.toml")),
    ("10-djent.toml", include_str!("../bank/10-djent.toml")),
    ("11-doom.toml", include_str!("../bank/11-doom.toml")),
    ("12-buzzsaw.toml", include_str!("../bank/12-buzzsaw.toml")),
    ("13-stoner.toml", include_str!("../bank/13-stoner.toml")),
    (
        "14-sub-octave.toml",
        include_str!("../bank/14-sub-octave.toml"),
    ),
    (
        "15-distortion.toml",
        include_str!("../bank/15-distortion.toml"),
    ),
    (
        "16-amp-tweed.toml",
        include_str!("../bank/16-amp-tweed.toml"),
    ),
    (
        "17-amp-american.toml",
        include_str!("../bank/17-amp-american.toml"),
    ),
    (
        "18-amp-chime.toml",
        include_str!("../bank/18-amp-chime.toml"),
    ),
    (
        "19-amp-crunch.toml",
        include_str!("../bank/19-amp-crunch.toml"),
    ),
    ("20-amp-lead.toml", include_str!("../bank/20-amp-lead.toml")),
    (
        "21-amp-modern.toml",
        include_str!("../bank/21-amp-modern.toml"),
    ),
];
const BASS_PRESETS: &[(&str, &str)] = &[
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
    ("06-rock.toml", include_str!("../bank-bass/06-rock.toml")),
    (
        "07-envelope-funk.toml",
        include_str!("../bank-bass/07-envelope-funk.toml"),
    ),
    (
        "08-modern-grind.toml",
        include_str!("../bank-bass/08-modern-grind.toml"),
    ),
    (
        "09-thrash.toml",
        include_str!("../bank-bass/09-thrash.toml"),
    ),
    ("10-djent.toml", include_str!("../bank-bass/10-djent.toml")),
    ("11-doom.toml", include_str!("../bank-bass/11-doom.toml")),
    (
        "12-buzzsaw.toml",
        include_str!("../bank-bass/12-buzzsaw.toml"),
    ),
    (
        "13-stoner.toml",
        include_str!("../bank-bass/13-stoner.toml"),
    ),
    (
        "14-synth-sub.toml",
        include_str!("../bank-bass/14-synth-sub.toml"),
    ),
    (
        "15-distortion.toml",
        include_str!("../bank-bass/15-distortion.toml"),
    ),
    (
        "16-amp-bass-valve.toml",
        include_str!("../bank-bass/16-amp-bass-valve.toml"),
    ),
    (
        "17-amp-bass-solid.toml",
        include_str!("../bank-bass/17-amp-bass-solid.toml"),
    ),
];
const STARTER_IRS: &[StarterIr] = &[
    (DEFAULT_IR, default_cabinet),
    (CAB_4X10_IR, cabinet_4x10),
    (CAB_2X12_OPEN_IR, cabinet_2x12_open),
    (CAB_2X12_ALNICO_IR, cabinet_2x12_alnico),
    (CAB_4X12_IR, cabinet_4x12),
];
const BASS_IRS: &[StarterIr] = &[
    (BASS_IR, bass_cabinet),
    (BASS_CAB_8X10_IR, bass_cabinet_8x10),
];

/// A Cabinet IR's file name and the function that synthesizes its samples at a sample rate.
pub(crate) type StarterIr = (&'static str, fn(u32) -> Vec<f32>);

/// The starter Presets of `kind` as (file name, TOML text), in Bank order.
pub(crate) fn starter_presets(kind: BankKind) -> &'static [(&'static str, &'static str)] {
    match kind {
        BankKind::Guitar => STARTER_PRESETS,
        BankKind::Bass => BASS_PRESETS,
    }
}

/// The Cabinet IRs the starter Presets of `kind` name.
pub(crate) fn starter_irs(kind: BankKind) -> &'static [StarterIr] {
    match kind {
        BankKind::Guitar => STARTER_IRS,
        BankKind::Bass => BASS_IRS,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Created {
    pub presets: usize,
    pub ir: bool,
}

/// Creates `bank_dir` with the starter Presets of `kind` and the Cabinet IRs they name, synthesized at `sample_rate`, unless `bank_dir` already exists; an existing directory is left untouched and the result is all zeros. The files are written into `.<bank name>.xguitare-staging` beside it (a stale one is replaced) and renamed into place, so an interrupted first run leaves no partial Bank.
///
/// # Errors
///
/// The I/O error when `bank_dir` has no final name or the staging directory, a Preset, a Cabinet IR or the final rename cannot be written.
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
    let presets = starter_presets(kind);
    for (file, text) in presets {
        fs::write(staging.join(file), text)?;
    }
    for (file, synthesize) in starter_irs(kind) {
        fs::write(
            staging.join(file),
            wav_bytes(&synthesize(sample_rate), sample_rate)?,
        )?;
    }
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

/// `samples` as a mono 32-bit float WAV file at `sample_rate`.
pub(crate) fn wav_bytes(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, io::Error> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut bytes = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut bytes, spec).map_err(io::Error::other)?;
    for &sample in samples {
        writer.write_sample(sample).map_err(io::Error::other)?;
    }
    writer.finalize().map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}
