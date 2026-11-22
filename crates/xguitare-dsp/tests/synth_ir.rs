use realfft::RealFftPlanner;
use xguitare_dsp::synth_ir::{
    bass_cabinet, bass_cabinet_8x10, cabinet_2x12_alnico, cabinet_2x12_open, cabinet_4x10,
    cabinet_4x12, default_cabinet,
};

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
fn existing_cabinets_keep_their_samples() {
    assert_eq!(fnv1a(&default_cabinet(RATE)), 0x4a99_5182_abae_f009);
    assert_eq!(fnv1a(&bass_cabinet(RATE)), 0xe6af_857d_5620_e6cd);
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

type Synthesize = fn(u32) -> Vec<f32>;

const NEW_SHAPES: [(&str, Synthesize, usize); 5] = [
    ("cabinet_4x10", cabinet_4x10, 4_800),
    ("cabinet_2x12_open", cabinet_2x12_open, 4_800),
    ("cabinet_2x12_alnico", cabinet_2x12_alnico, 4_800),
    ("cabinet_4x12", cabinet_4x12, 4_800),
    ("bass_cabinet_8x10", bass_cabinet_8x10, 5_760),
];

#[test]
fn new_shapes_are_deterministic() {
    for (name, shape, _) in NEW_SHAPES {
        assert_eq!(shape(RATE), shape(RATE), "{name}");
    }
}

#[test]
fn new_shapes_peak_at_minus_3dbfs() {
    for (name, shape, _) in NEW_SHAPES {
        let peak = shape(RATE).iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let peak_db = 20.0 * peak.log10();
        assert!((peak_db + 3.0).abs() <= 0.1, "{name}: peak {peak_db} dBFS");
    }
}

#[test]
fn new_shapes_have_their_length() {
    for (name, shape, frames) in NEW_SHAPES {
        assert_eq!(shape(RATE).len(), frames, "{name}");
    }
}

#[test]
fn every_shape_differs_from_the_others() {
    let shapes: Vec<(&str, Vec<f32>)> = [
        ("default_cabinet", default_cabinet(RATE)),
        ("bass_cabinet", bass_cabinet(RATE)),
    ]
    .into_iter()
    .chain(NEW_SHAPES.map(|(name, shape, _)| (name, shape(RATE))))
    .collect();
    for (idx, (name, ir)) in shapes.iter().enumerate() {
        for (other_name, other) in &shapes[idx + 1..] {
            assert_ne!(ir, other, "{name} equals {other_name}");
        }
    }
}

fn fnv1a(samples: &[f32]) -> u64 {
    samples
        .iter()
        .flat_map(|sample| sample.to_bits().to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
}
