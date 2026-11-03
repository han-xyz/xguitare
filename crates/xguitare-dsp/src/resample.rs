use std::f64::consts::PI;

const TAPS_PER_SIDE: f64 = 32.0;
const CUTOFF: f64 = 0.45;

/// Resamples `input` from `from_hz` to `to_hz` with a Blackman-windowed sinc evaluated per output sample; samples outside `input` count as silence. Offline use only: it allocates.
pub fn resample_offline(input: &[f32], from_hz: u32, to_hz: u32) -> Vec<f32> {
    if from_hz == to_hz {
        return input.to_vec();
    }
    let ratio = f64::from(from_hz) / f64::from(to_hz);
    let output_len = (input.len() as f64 / ratio).round() as usize;
    let cutoff = CUTOFF * f64::from(from_hz.min(to_hz)) / f64::from(from_hz);
    let half_width = TAPS_PER_SIDE * ratio.max(1.0);
    (0..output_len)
        .map(|n| {
            let center = n as f64 * ratio;
            let first = (center - half_width).ceil() as i64;
            let last = (center + half_width).floor() as i64;
            let mut weight_sum = 0.0;
            let mut sum = 0.0;
            for k in first..=last {
                let weight = sinc(2.0 * cutoff * (k as f64 - center))
                    * blackman((k as f64 - center) / half_width);
                weight_sum += weight;
                if let Some(sample) = usize::try_from(k).ok().and_then(|k| input.get(k)) {
                    sum += weight * f64::from(*sample);
                }
            }
            (sum / weight_sum) as f32
        })
        .collect()
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

fn blackman(x: f64) -> f64 {
    0.42 + 0.5 * (PI * x).cos() + 0.08 * (2.0 * PI * x).cos()
}
