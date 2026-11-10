use std::fmt;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader};
use xguitare_dsp::modules::cabinet::CabinetIr;
use xguitare_dsp::resample::resample_offline;

const EMPTY_IR: &str = "empty impulse response";

#[derive(Debug)]
pub enum IrError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Unsupported {
        path: PathBuf,
        detail: String,
    },
    Decode {
        path: PathBuf,
        detail: String,
    },
    NonFinite {
        name: String,
    },
}

/// Reads at most `max_ms` of a 16/24/32-bit PCM or 32-bit float wav with one or two channels, refuses a non-finite sample, then resamples to `engine_rate`, truncates to `max_ms` and normalises to unit energy. `name` is the Cabinet IR name as written in the Preset, used in the non-finite error.
///
/// # Errors
///
/// An [`IrError`] when the file cannot be opened or decoded, has an unsupported format or channel count, or holds a non-finite sample.
pub fn load_ir(
    path: &Path,
    name: &str,
    engine_rate: u32,
    max_ms: f32,
) -> Result<CabinetIr, IrError> {
    let hound_error = |error| IrError::from_hound(path, error);
    let reader = WavReader::open(path).map_err(hound_error)?;
    let spec = reader.spec();
    let file_max_len = (spec.sample_rate as f32 * max_ms / 1000.0).ceil() as usize;
    let max_samples = file_max_len.saturating_mul(usize::from(spec.channels));
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, bits @ (16 | 24 | 32)) => {
            let full_scale = (1_i64 << (bits - 1)) as f32;
            reader
                .into_samples::<i32>()
                .take(max_samples)
                .map(|sample| sample.map(|value| value as f32 / full_scale))
                .collect::<Result<_, _>>()
                .map_err(hound_error)?
        }
        (SampleFormat::Float, 32) => reader
            .into_samples::<f32>()
            .take(max_samples)
            .collect::<Result<_, _>>()
            .map_err(hound_error)?,
        (format, bits) => {
            let format = match format {
                SampleFormat::Int => "int",
                SampleFormat::Float => "float",
            };
            return Err(IrError::unsupported(path, format!("{bits}-bit {format}")));
        }
    };
    let non_finite = || IrError::NonFinite {
        name: name.to_string(),
    };
    if !samples.iter().all(|sample| sample.is_finite()) {
        return Err(non_finite());
    }
    let max_len = (engine_rate as f32 * max_ms / 1000.0) as usize;
    let fit = |mut channel: Vec<f32>| {
        channel.truncate(file_max_len);
        let mut channel = if spec.sample_rate == engine_rate {
            channel
        } else {
            resample_offline(&channel, spec.sample_rate, engine_rate)
        };
        channel.truncate(max_len);
        if !channel.iter().all(|tap| tap.is_finite()) {
            return Err(non_finite());
        }
        if channel.is_empty() {
            return Err(IrError::Decode {
                path: path.to_path_buf(),
                detail: EMPTY_IR.to_string(),
            });
        }
        Ok(channel)
    };
    let mut ir = match spec.channels {
        1 => CabinetIr {
            left: fit(samples)?,
            right: None,
        },
        2 => {
            let left = samples.iter().step_by(2).copied().collect();
            let right = samples.iter().skip(1).step_by(2).copied().collect();
            CabinetIr {
                left: fit(left)?,
                right: Some(fit(right)?),
            }
        }
        n => return Err(IrError::unsupported(path, format!("{n} channels"))),
    };
    ir.normalise();
    Ok(ir)
}

impl IrError {
    fn unsupported(path: &Path, detail: String) -> IrError {
        IrError::Unsupported {
            path: path.to_path_buf(),
            detail,
        }
    }

    fn from_hound(path: &Path, error: hound::Error) -> IrError {
        match error {
            hound::Error::IoError(source) => IrError::Io {
                path: path.to_path_buf(),
                source,
            },
            hound::Error::Unsupported => IrError::unsupported(path, error.to_string()),
            other => IrError::Decode {
                path: path.to_path_buf(),
                detail: other.to_string(),
            },
        }
    }
}

impl fmt::Display for IrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IrError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            IrError::Unsupported { path, detail } => {
                write!(f, "{}: unsupported wav format: {detail}", path.display())
            }
            IrError::Decode { path, detail } => write!(f, "{}: {detail}", path.display()),
            IrError::NonFinite { name } => write!(f, "ir {name}: non-finite sample"),
        }
    }
}

impl std::error::Error for IrError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IrError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
