use std::fmt;

use serde::{Deserialize, Serialize};

const BAD_BITS: &str = "bits must be 16, 24 or \"32f\"";
const FLOAT_BITS: &str = "32f";

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
