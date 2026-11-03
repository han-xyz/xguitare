use crate::smooth::Smoother;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const BASS: usize = 0;
const MIDDLE: usize = 1;
const TREBLE: usize = 2;

// Fender '59 Bassman values and transfer function from D. T. Yeh, J. O. Smith, "Discretization of the '59 Fender Bassman Tone Stack", DAFx 2006.
const R1: f64 = 250e3;
const R2: f64 = 1e6;
const R3: f64 = 25e3;
const R4: f64 = 56e3;
const C1: f64 = 250e-12;
const C2: f64 = 20e-9;
const C3: f64 = 20e-9;

static SPEC: ModuleSpec = ModuleSpec {
    type_name: "tonestack",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "bass",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "middle",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
        },
        ParamSpec {
            name: "treble",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: Unit::Percent,
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
        let analog = analog_coefficients(position[TREBLE], position[MIDDLE], position[BASS]);
        let (b, a) = bilinear(analog, 2.0 * self.sample_rate);
        let s = &mut self.state;
        for sample in &mut io.mono[..io.frames] {
            let x = f64::from(*sample);
            let y = b[0] * x + s[0];
            s[0] = b[1] * x - a[1] * y + s[1];
            s[1] = b[2] * x - a[2] * y + s[2];
            s[2] = b[3] * x - a[3] * y;
            *sample = y as f32;
        }
    }
}

type Coefficients = ([f64; 4], [f64; 4]);

fn analog_coefficients(t: f64, m: f64, l: f64) -> Coefficients {
    let b1 = t * C1 * R1 + m * C3 * R3 + l * (C1 * R2 + C2 * R2) + (C1 * R3 + C2 * R3);
    let b2 = t * (C1 * C2 * R1 * R4 + C1 * C3 * R1 * R4)
        - m * m * (C1 * C3 * R3 * R3 + C2 * C3 * R3 * R3)
        + m * (C1 * C3 * R1 * R3 + C1 * C3 * R3 * R3 + C2 * C3 * R3 * R3)
        + l * (C1 * C2 * R1 * R2 + C1 * C2 * R2 * R4 + C1 * C3 * R2 * R4)
        + l * m * (C1 * C3 * R2 * R3 + C2 * C3 * R2 * R3)
        + (C1 * C2 * R1 * R3 + C1 * C2 * R3 * R4 + C1 * C3 * R3 * R4);
    let b3 = l * m * (C1 * C2 * C3 * R1 * R2 * R3 + C1 * C2 * C3 * R2 * R3 * R4)
        - m * m * (C1 * C2 * C3 * R1 * R3 * R3 + C1 * C2 * C3 * R3 * R3 * R4)
        + m * (C1 * C2 * C3 * R1 * R3 * R3 + C1 * C2 * C3 * R3 * R3 * R4)
        + t * C1 * C2 * C3 * R1 * R3 * R4
        - t * m * C1 * C2 * C3 * R1 * R3 * R4
        + t * l * C1 * C2 * C3 * R1 * R2 * R4;
    let a1 =
        (C1 * R1 + C1 * R3 + C2 * R3 + C2 * R4 + C3 * R4) + m * C3 * R3 + l * (C1 * R2 + C2 * R2);
    let a2 = m * (C1 * C3 * R1 * R3 - C2 * C3 * R3 * R4 + C1 * C3 * R3 * R3 + C2 * C3 * R3 * R3)
        + l * m * (C1 * C3 * R2 * R3 + C2 * C3 * R2 * R3)
        - m * m * (C1 * C3 * R3 * R3 + C2 * C3 * R3 * R3)
        + l * (C1 * C2 * R2 * R4 + C1 * C2 * R1 * R2 + C1 * C3 * R2 * R4 + C2 * C3 * R2 * R4)
        + (C1 * C2 * R1 * R4
            + C1 * C3 * R1 * R4
            + C1 * C2 * R3 * R4
            + C1 * C2 * R1 * R3
            + C1 * C3 * R3 * R4
            + C2 * C3 * R3 * R4);
    let a3 = l * m * (C1 * C2 * C3 * R1 * R2 * R3 + C1 * C2 * C3 * R2 * R3 * R4)
        - m * m * (C1 * C2 * C3 * R1 * R3 * R3 + C1 * C2 * C3 * R3 * R3 * R4)
        + m * (C1 * C2 * C3 * R3 * R3 * R4 + C1 * C2 * C3 * R1 * R3 * R3
            - C1 * C2 * C3 * R1 * R3 * R4)
        + l * C1 * C2 * C3 * R1 * R2 * R4
        + C1 * C2 * C3 * R1 * R3 * R4;
    ([0.0, b1, b2, b3], [1.0, a1, a2, a3])
}

fn bilinear((b, a): Coefficients, c: f64) -> Coefficients {
    let discretise = |k: [f64; 4]| {
        let [k0, k1, k2, k3] = [k[0], k[1] * c, k[2] * c * c, k[3] * c * c * c];
        [
            k0 + k1 + k2 + k3,
            3.0 * k0 + k1 - k2 - 3.0 * k3,
            3.0 * k0 - k1 - k2 + 3.0 * k3,
            k0 - k1 + k2 - k3,
        ]
    };
    let (b, a) = (discretise(b), discretise(a));
    (b.map(|k| k / a[0]), a.map(|k| k / a[0]))
}
