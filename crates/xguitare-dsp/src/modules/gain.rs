use super::db_to_linear;
use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const LEVEL: usize = 0;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "gain",
    channels: Channels::Mono,
    params: &[ParamSpec {
        name: "level",
        min: -60.0,
        max: 20.0,
        default: 0.0,
        unit: Unit::Db,
        integer: false,
    }],
};

pub struct Gain {
    level_db: f32,
    level: Smoother,
}

impl Gain {
    pub fn new() -> Gain {
        let level_db = SPEC.params[LEVEL].default;
        Gain {
            level_db,
            level: Smoother::new(db_to_linear(level_db)),
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
        self.level.prepare(sample_rate);
    }

    fn reset(&mut self) {
        self.level.snap();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        debug_assert!(idx == LEVEL, "gain has no parameter {idx}");
        let spec = &SPEC.params[LEVEL];
        self.level_db = value.clamp(spec.min, spec.max);
        self.level.set_target(db_to_linear(self.level_db));
    }

    fn param(&self, idx: usize) -> f32 {
        debug_assert!(idx == LEVEL, "gain has no parameter {idx}");
        self.level_db
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            *sample *= self.level.next_value();
        }
    }
}
