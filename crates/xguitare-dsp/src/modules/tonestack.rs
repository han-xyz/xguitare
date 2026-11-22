use crate::fmv::{self, FmvStack};
use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const BASS: usize = 0;
const MIDDLE: usize = 1;
const TREBLE: usize = 2;

// Fender '59 Bassman values from D. T. Yeh, J. O. Smith, "Discretization of the '59 Fender Bassman Tone Stack", DAFx 2006.
const STACK: FmvStack = FmvStack {
    r1: 250e3,
    r2: 1e6,
    r3: 25e3,
    r4: 56e3,
    c1: 250e-12,
    c2: 20e-9,
    c3: 20e-9,
};

pub static SPEC: ModuleSpec = ModuleSpec {
    type_name: "tonestack",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "bass",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "middle",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "treble",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
    ],
};

pub struct Tonestack {
    values: [f32; 3],
    positions: [Smoother; 3],
    sample_rate: f64,
    state: [f64; 3],
}

impl Tonestack {
    pub fn new() -> Tonestack {
        let values = std::array::from_fn(|idx| SPEC.params[idx].default);
        Tonestack {
            values,
            positions: values.map(|value| Smoother::new(value / 100.0)),
            sample_rate: 48_000.0,
            state: [0.0; 3],
        }
    }
}

impl Default for Tonestack {
    fn default() -> Tonestack {
        Tonestack::new()
    }
}

impl Module for Tonestack {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.sample_rate = f64::from(sample_rate);
        for position in &mut self.positions {
            position.prepare(sample_rate);
        }
    }

    fn reset(&mut self) {
        for position in &mut self.positions {
            position.snap();
        }
        self.state = [0.0; 3];
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        self.positions[idx].set_target(value / 100.0);
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let position = self
            .positions
            .each_mut()
            .map(|position| f64::from(position.advance(io.frames)));
        let analog =
            fmv::analog_coefficients(&STACK, position[TREBLE], position[MIDDLE], position[BASS]);
        let coefficients = fmv::bilinear(analog, 2.0 * self.sample_rate);
        for sample in &mut io.mono[..io.frames] {
            *sample = fmv::step(&coefficients, &mut self.state, f64::from(*sample)) as f32;
        }
    }
}
