use realfft::RealFftPlanner;
use xguitare_dsp::synth_ir::{bass_cabinet, default_cabinet};

const RATE: u32 = 48_000;

#[test]
fn length_is_100ms() {
    assert_eq!(default_cabinet(RATE).len(), 4_800);
    assert_eq!(default_cabinet(44_100).len(), 4_410);
}

#[test]
fn peak_is_minus_3dbfs() {
    let ir = default_cabinet(RATE);
    let peak = ir.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let peak_db = 20.0 * peak.log10();
    assert!((peak_db + 3.0).abs() <= 0.1, "peak {peak_db} dBFS");
}

#[test]
fn energy_in_band() {
    let mut ir = default_cabinet(RATE);
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(ir.len());
    let mut spectrum = fft.make_output_vec();
    fft.process(&mut ir, &mut spectrum).unwrap();
    let bin_hz = RATE as f32 / fft.len() as f32;
    let (in_band, total) =
        spectrum
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(in_band, total), (bin, value)| {
                let energy = value.norm_sqr();
                let freq = bin as f32 * bin_hz;
                let inside = (80.0..=5_000.0).contains(&freq);
                (in_band + if inside { energy } else { 0.0 }, total + energy)
            });
    let ratio = in_band / total;
    assert!(ratio >= 0.9, "in-band energy {ratio}");
}

#[test]
fn deterministic() {
    assert_eq!(default_cabinet(RATE), default_cabinet(RATE));
}

#[test]
fn guitar_ir_unchanged() {
    assert_eq!(fnv1a(&default_cabinet(RATE)), 0x4a99_5182_abae_f009);
    assert_eq!(fnv1a(&default_cabinet(44_100)), 0xd712_4fb2_fdf4_f188);
}

#[test]
fn bass_length_is_120ms() {
    assert_eq!(bass_cabinet(RATE).len(), 5_760);
    assert_eq!(bass_cabinet(44_100).len(), 5_292);
}

#[test]
fn bass_peak_is_minus_3dbfs() {
    let ir = bass_cabinet(RATE);
    let peak = ir.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let peak_db = 20.0 * peak.log10();
    assert!((peak_db + 3.0).abs() <= 0.1, "peak {peak_db} dBFS");
}

#[test]
fn bass_energy_below_3khz() {
    let mut ir = bass_cabinet(RATE);
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(ir.len());
    let mut spectrum = fft.make_output_vec();
    fft.process(&mut ir, &mut spectrum).unwrap();
    let bin_hz = RATE as f32 / fft.len() as f32;
    let (below, total) =
        spectrum
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(below, total), (bin, value)| {
                let energy = value.norm_sqr();
                let freq = bin as f32 * bin_hz;
                (
                    below + if freq < 3_000.0 { energy } else { 0.0 },
                    total + energy,
                )
            });
    let ratio = below / total;
    assert!(ratio >= 0.85, "energy below 3 kHz {ratio}");
}

#[test]
fn bass_deterministic() {
    assert_eq!(bass_cabinet(RATE), bass_cabinet(RATE));
}

fn fnv1a(samples: &[f32]) -> u64 {
    samples
        .iter()
        .flat_map(|sample| sample.to_bits().to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
}
