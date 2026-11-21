use serde_json::{Value, json};

use crate::control::EngineState;
use crate::project_control::loaded_peaks;
use xguitare_protocol::FRAMES_PER_PEAK;

/// The most pairs one `take_peaks` reply carries.
pub const MAX_PEAKS_PER_REPLY: usize = 16_384;
const FULL_SCALE: f32 = 127.0;

/// The min and max of each [`FRAMES_PER_PEAK`] frames of `samples`, the last partial run included, as `[min, max, …]` scaled to ±127.
pub(crate) fn peak_table(samples: &[f32]) -> Vec<i8> {
    samples
        .chunks(FRAMES_PER_PEAK)
        .flat_map(|frames| {
            let (min, max) = frames
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), &sample| {
                    (min.min(sample), max.max(sample))
                });
            [quantise(min), quantise(max)]
        })
        .collect()
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "clamped to ±127 before the cast"
)]
fn quantise(sample: f32) -> i8 {
    (sample.clamp(-1.0, 1.0) * FULL_SCALE).round() as i8
}

/// Up to `count` pairs, at most [`MAX_PEAKS_PER_REPLY`], of the loaded Take `file`'s peak table from pair `first`, with the Take's frames.
pub(crate) fn peak_page(
    state: &EngineState,
    file: &str,
    first: Option<usize>,
    count: Option<usize>,
) -> Result<Value, String> {
    let (frames, table) = loaded_peaks(state, file)?;
    let total = table.len() / 2;
    let first = first.unwrap_or(0);
    let count = count
        .unwrap_or(MAX_PEAKS_PER_REPLY)
        .min(MAX_PEAKS_PER_REPLY);
    let start = first.min(total);
    let end = start.saturating_add(count).min(total);
    Ok(json!({
        "frames_per_peak": FRAMES_PER_PEAK,
        "frames": frames,
        "total": total,
        "first": first,
        "peaks": table[start * 2..end * 2],
    }))
}
