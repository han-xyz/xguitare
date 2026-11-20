use std::fmt;

use serde::{Deserialize, Serialize};

const BAD_BITS: &str = "bits must be 16, 24 or \"32f\"";
const FLOAT_BITS: &str = "32f";
const BAD_POSITION: &str = "position must be <bar>.<beat> or <seconds>s";
const SECONDS_SUFFIX: char = 's';

/// The frames one min/max pair of a peak table covers.
pub const FRAMES_PER_PEAK: usize = 256;
/// The answer to `project_new` and `project_open` while a closed Project still gets its last Takes.
pub const STILL_CLOSING: &str = "a project is still closing; try again";

/// Which starter Bank the first run seeds into a missing `bank_dir`; without a `bank_dir` in the config file, `[banks]` in `state.toml` can name each kind's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BankKind {
    #[default]
    Guitar,
    Bass,
}

impl fmt::Display for BankKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BankKind::Guitar => "guitar",
            BankKind::Bass => "bass",
        })
    }
}

/// What a Render plays: the Project from frame 0 to the end of its last Item plus the tail, or the loop region exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderRange {
    Project,
    Loop,
}

/// The sample format of the WAV a Render writes; on the wire `16`, `24` or `"32f"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BitsOnWire", into = "BitsOnWire")]
pub enum RenderBits {
    /// 16-bit integer with TPDF dither.
    Pcm16,
    Pcm24,
    Float32,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum BitsOnWire {
    Depth(u32),
    Word(String),
}

impl TryFrom<BitsOnWire> for RenderBits {
    type Error = &'static str;

    fn try_from(bits: BitsOnWire) -> Result<Self, Self::Error> {
        match bits {
            BitsOnWire::Depth(16) => Ok(RenderBits::Pcm16),
            BitsOnWire::Depth(24) => Ok(RenderBits::Pcm24),
            BitsOnWire::Word(word) if word == FLOAT_BITS => Ok(RenderBits::Float32),
            BitsOnWire::Depth(_) | BitsOnWire::Word(_) => Err(BAD_BITS),
        }
    }
}

impl From<RenderBits> for BitsOnWire {
    fn from(bits: RenderBits) -> Self {
        match bits {
            RenderBits::Pcm16 => BitsOnWire::Depth(16),
            RenderBits::Pcm24 => BitsOnWire::Depth(24),
            RenderBits::Float32 => BitsOnWire::Word(FLOAT_BITS.to_string()),
        }
    }
}

/// Start and end in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopRegion {
    pub start: u64,
    pub end: u64,
    pub on: bool,
}

/// `name` with each control character and each Unicode bidi override or isolate escaped as `escape_debug` writes it, so a file name shown to people reads as the name it is.
pub fn shown_file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if is_hidden_char(c) {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// A control character, or a bidi override or isolate (U+202A–U+202E, U+2066–U+2069), which can make shown text read as other text.
pub(crate) fn is_hidden_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// The frame of `<bar>.<beat>` or `<bar>` (both from 1, the beat within `beats_per_bar`) or `<seconds>s`, at `sample_rate` and `tempo_bpm`, rounded; not bounded.
///
/// # Errors
///
/// `position must be <bar>.<beat> or <seconds>s` for any other text, a negative time, or a bar or beat of 0 or a beat past the bar.
pub fn position_frame(
    text: &str,
    sample_rate: u32,
    tempo_bpm: f64,
    beats_per_bar: u32,
) -> Result<f64, String> {
    if let Some(seconds) = text.strip_suffix(SECONDS_SUFFIX) {
        let seconds: f64 = seconds.parse().map_err(|_| BAD_POSITION)?;
        if !seconds.is_finite() || seconds.is_sign_negative() {
            return Err(BAD_POSITION.to_string());
        }
        return Ok((seconds * f64::from(sample_rate)).round());
    }
    let (bar, beat) = text.split_once('.').unwrap_or((text, "1"));
    let (Ok(bar), Ok(beat)) = (bar.parse::<u64>(), beat.parse::<u64>()) else {
        return Err(BAD_POSITION.to_string());
    };
    let beats_per_bar = u64::from(beats_per_bar);
    if bar == 0 || beat == 0 || beat > beats_per_bar {
        return Err(BAD_POSITION.to_string());
    }
    let beat_index = (bar - 1) as f64 * beats_per_bar as f64 + (beat - 1) as f64;
    Ok((beat_index * (f64::from(sample_rate) * 60.0 / tempo_bpm)).round())
}
