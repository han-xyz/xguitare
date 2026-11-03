use realfft::RealFftPlanner;
use xguitare_dsp::synth_ir::default_cabinet;

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
