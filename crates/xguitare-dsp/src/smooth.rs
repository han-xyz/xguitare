use crate::modules::one_pole_coeff;

pub const SMOOTHING_SECONDS: f32 = 0.005;
/// How near its target, relative to the target or to 1 when the target is smaller, a settling smoother lands on it.
pub const SETTLE_TOLERANCE: f32 = 1e-4;

/// One-pole 5 ms Parameter smoother; `prepare` with the rate at which `next_value` is called.
pub struct Smoother {
    target: f32,
    current: f32,
    coeff: f32,
}

impl Smoother {
    pub fn new(value: f32) -> Smoother {
        Smoother {
            target: value,
            current: value,
            coeff: 0.0,
        }
    }

    pub fn prepare(&mut self, sample_rate: f32) {
        self.coeff = one_pole_coeff(SMOOTHING_SECONDS, sample_rate);
    }

    pub fn set_target(&mut self, value: f32) {
        self.target = value;
    }

    /// Replaces the current value with `map(current)`; the ramp toward the target carries on from there.
    pub fn map_current(&mut self, map: impl FnOnce(f32) -> f32) {
        self.current = map(self.current);
    }

    pub fn snap(&mut self) {
        self.current = self.target;
    }

    pub fn next_value(&mut self) -> f32 {
        self.current = self.target + self.coeff * (self.current - self.target);
        self.current
    }

    /// `next_value`, landing exactly on the target once within `SETTLE_TOLERANCE` of it, so that a settled value stops changing.
    pub fn next_value_settling(&mut self) -> f32 {
        let value = self.next_value();
        if (value - self.target).abs() <= SETTLE_TOLERANCE * self.target.abs().max(1.0) {
            self.current = self.target;
        }
        self.current
    }

    pub fn advance(&mut self, frames: usize) -> f32 {
        self.current = self.target
            + self.coeff.powi(i32::try_from(frames).unwrap_or(i32::MAX))
                * (self.current - self.target);
        self.current
    }
}
