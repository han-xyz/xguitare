use std::fs;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};
use xguitare_core::{IrError, load_ir};

const RAMP_LEN: usize = 100;

struct TempDir(PathBuf);

impl TempDir {
    fn join(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-ir-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn ramp() -> Vec<f32> {
    (0..RAMP_LEN)
        .map(|n| n as f32 / RAMP_LEN as f32 - 0.5)
        .collect()
}

fn spec(channels: u16, sample_rate: u32, bits: u16, format: SampleFormat) -> WavSpec {
    WavSpec {
        channels,
        sample_rate,
        bits_per_sample: bits,
        sample_format: format,
    }
}

fn write_int(path: &Path, spec: WavSpec, samples: &[f32]) {
    let full_scale = (1_i64 << (spec.bits_per_sample - 1)) as f64;
    let mut writer = WavWriter::create(path, spec).unwrap();
    for sample in samples {
        let value = (f64::from(*sample) * full_scale).round() as i32;
        match spec.bits_per_sample {
            8 => writer.write_sample(value as i8).unwrap(),
            16 => writer.write_sample(value as i16).unwrap(),
            _ => writer.write_sample(value).unwrap(),
        }
    }
    writer.finalize().unwrap();
}

fn write_float(path: &Path, spec: WavSpec, samples: &[f32]) {
    let mut writer = WavWriter::create(path, spec).unwrap();
    for sample in samples {
        writer.write_sample(*sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn assert_ramp(samples: &[f32]) {
    assert_eq!(samples.len(), RAMP_LEN);
    for (n, (actual, expected)) in samples.iter().zip(ramp()).enumerate() {
        assert!(
            (actual - expected).abs() <= 1e-4,
            "sample {n}: {actual} vs {expected}"
        );
    }
}

fn loads_int(test: &str, bits: u16) {
    let dir = temp_dir(test);
    let path = dir.join("ir.wav");
    write_int(&path, spec(1, 48_000, bits, SampleFormat::Int), &ramp());
    let ir = load_ir(&path, 48_000, 1000.0).unwrap();
    assert_ramp(&ir.left);
    assert_eq!(ir.right, None);
}

#[test]
fn loads_pcm16() {
    loads_int("pcm16", 16);
}

#[test]
fn loads_pcm24() {
    loads_int("pcm24", 24);
}

#[test]
fn loads_pcm32() {
    loads_int("pcm32", 32);
}

#[test]
fn loads_float32() {
    let dir = temp_dir("float32");
    let path = dir.join("ir.wav");
    write_float(&path, spec(1, 48_000, 32, SampleFormat::Float), &ramp());
    let ir = load_ir(&path, 48_000, 1000.0).unwrap();
    assert_ramp(&ir.left);
}

#[test]
fn loads_stereo() {
    let dir = temp_dir("stereo");
    let path = dir.join("ir.wav");
    let interleaved: Vec<f32> = ramp().iter().flat_map(|s| [*s, -*s]).collect();
    write_float(
        &path,
        spec(2, 48_000, 32, SampleFormat::Float),
        &interleaved,
    );
    let ir = load_ir(&path, 48_000, 1000.0).unwrap();
    assert_ramp(&ir.left);
    let right = ir.right.expect("stereo file has a right channel");
    let negated: Vec<f32> = ramp().iter().map(|s| -s).collect();
    assert_eq!(right, negated);
}

#[test]
fn resamples_to_engine_rate() {
    let dir = temp_dir("resample");
    let path = dir.join("ir.wav");
    write_float(&path, spec(1, 44_100, 32, SampleFormat::Float), &ramp());
    let ir = load_ir(&path, 48_000, 1000.0).unwrap();
    assert_eq!(ir.left.len(), 109);
}

#[test]
fn truncates_to_max_ms() {
    let dir = temp_dir("truncate");
    let path = dir.join("ir.wav");
    write_float(
        &path,
        spec(1, 48_000, 32, SampleFormat::Float),
        &[0.1; 48_000],
    );
    let ir = load_ir(&path, 48_000, 100.0).unwrap();
    assert_eq!(ir.left.len(), 4800);
}

#[test]
fn unsupported_8_bit() {
    let dir = temp_dir("pcm8");
    let path = dir.join("ir.wav");
    write_int(&path, spec(1, 48_000, 8, SampleFormat::Int), &ramp());
    let error = load_ir(&path, 48_000, 1000.0).unwrap_err();
    assert!(matches!(error, IrError::Unsupported { .. }), "{error}");
    assert_eq!(
        error.to_string(),
        format!("{}: unsupported wav format: 8-bit int", path.display())
    );
}

#[test]
fn missing_file_is_io() {
    let dir = temp_dir("missing");
    let path = dir.join("absent.wav");
    let error = load_ir(&path, 48_000, 1000.0).unwrap_err();
    assert!(matches!(error, IrError::Io { .. }), "{error}");
    assert!(
        error
            .to_string()
            .starts_with(&format!("{}: ", path.display())),
        "{error}"
    );
}

#[test]
fn three_channels_unsupported() {
    let dir = temp_dir("three");
    let path = dir.join("ir.wav");
    write_float(&path, spec(3, 48_000, 32, SampleFormat::Float), &[0.1; 30]);
    let error = load_ir(&path, 48_000, 1000.0).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("{}: unsupported wav format: 3 channels", path.display())
    );
}

#[test]
fn empty_file_is_error() {
    let dir = temp_dir("empty");
    let path = dir.join("ir.wav");
    write_float(&path, spec(1, 48_000, 32, SampleFormat::Float), &[]);
    let error = load_ir(&path, 48_000, 1000.0).unwrap_err();
    assert!(matches!(error, IrError::Decode { .. }), "{error}");
    assert_eq!(
        error.to_string(),
        format!("{}: empty impulse response", path.display())
    );
}
