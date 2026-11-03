pub mod cabinet;
pub mod drive;
pub mod gain;
pub mod gate;
pub mod preamp;
pub mod tonestack;

fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub(crate) fn one_pole_coeff(seconds: f32, sample_rate: f32) -> f32 {
    (-1.0 / (seconds * sample_rate)).exp()
}
