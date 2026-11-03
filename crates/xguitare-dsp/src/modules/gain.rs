use super::db_to_linear;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const LEVEL: usize = 0;
const SMOOTHING_SECONDS: f32 = 0.005;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "gain",
    channels: Channels::Mono,
    params: &[ParamSpec {
        name: "level",
        min: -60.0,
        max: 20.0,
        default: 0.0,
        unit: Unit::Db,
    }],
};

pub struct Gain {
    level_db: f32,
    target: f32,
    current: f32,
    coeff: f32,
}

impl Gain {
    pub fn new() -> Gain {
        let level_db = SPEC.params[LEVEL].default;
        Gain {
            level_db,
            target: db_to_linear(level_db),
            current: db_to_linear(level_db),
            coeff: 0.0,
        }
    }
}

impl Default for Gain {
    fn default() -> Gain {
        Gain::new()
    }
}

impl Module for Gain {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.coeff = (-1.0 / (SMOOTHING_SECONDS * sample_rate)).exp();
    }

    fn reset(&mut self) {
        self.current = self.target;
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        debug_assert!(idx == LEVEL, "gain has no parameter {idx}");
        let spec = &SPEC.params[LEVEL];
        self.level_db = value.clamp(spec.min, spec.max);
        self.target = db_to_linear(self.level_db);
    }

    fn param(&self, idx: usize) -> f32 {
        debug_assert!(idx == LEVEL, "gain has no parameter {idx}");
        self.level_db
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            self.current = self.target + self.coeff * (self.current - self.target);
            *sample *= self.current;
        }
    }
}
