use std::f32::consts::FRAC_1_SQRT_2;

use super::db_to_linear;
use crate::smooth::Smoother;
use crate::svf::{Svf, SvfCoefficients};
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const LOW_CUT: usize = 0;
const LOW_FREQ: usize = 1;
const LOW_GAIN: usize = 2;
const MID1_FREQ: usize = 3;
const MID1_GAIN: usize = 4;
const MID1_Q: usize = 5;
const MID2_FREQ: usize = 6;
const MID2_GAIN: usize = 7;
const MID2_Q: usize = 8;
const HIGH_FREQ: usize = 9;
const HIGH_GAIN: usize = 10;
const HIGH_CUT: usize = 11;
const OUTPUT: usize = 12;
const PARAMS: usize = 13;
const FILTERS: usize = 6;
const CUT_Q: f32 = FRAC_1_SQRT_2;
const SHELF_Q: f32 = FRAC_1_SQRT_2;
/// Filter frequencies stop below Nyquist, so a 20 kHz cut stays stable at low sample rates.
pub const MAX_FREQ_PER_RATE: f32 = 0.45;

pub static SPEC: ModuleSpec = ModuleSpec {
    type_name: "eq",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "low_cut",
            min: 10.0,
            max: 300.0,
            default: 10.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "low_freq",
            min: 20.0,
            max: 500.0,
            default: 100.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "low_gain",
            min: -15.0,
            max: 15.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "mid1_freq",
            min: 40.0,
            max: 5000.0,
            default: 400.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "mid1_gain",
            min: -15.0,
            max: 15.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "mid1_q",
            min: 0.3,
            max: 10.0,
            default: 0.707,
            unit: Unit::Linear,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "mid2_freq",
            min: 200.0,
            max: 10_000.0,
            default: 2500.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "mid2_gain",
            min: -15.0,
            max: 15.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "mid2_q",
            min: 0.3,
            max: 10.0,
            default: 0.707,
            unit: Unit::Linear,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "high_freq",
            min: 1000.0,
            max: 20_000.0,
            default: 8000.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "high_gain",
            min: -15.0,
            max: 15.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "high_cut",
            min: 1000.0,
            max: 20_000.0,
            default: 20_000.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "output",
            min: -12.0,
            max: 12.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
    ],
};

/// The eq's gain in dB at `freq_hz` for Parameter values `params` in spec order (all thirteen), `output` included: the same filters the Module runs, read from their transfer functions.
pub fn eq_response_db(params: &[f32], freq_hz: f32, sample_rate: f32) -> f32 {
    let filters_db: f32 = filter_coefficients(params, sample_rate)
        .iter()
        .map(|coefficients| coefficients.gain_db(freq_hz, sample_rate))
        .sum();
    filters_db + params[OUTPUT]
}

fn filter_coefficients(values: &[f32], sample_rate: f32) -> [SvfCoefficients; FILTERS] {
    let freq = |idx: usize| values[idx].min(MAX_FREQ_PER_RATE * sample_rate);
    [
        SvfCoefficients::high_pass(freq(LOW_CUT), CUT_Q, sample_rate),
        SvfCoefficients::low_shelf(freq(LOW_FREQ), SHELF_Q, values[LOW_GAIN], sample_rate),
        SvfCoefficients::bell(
            freq(MID1_FREQ),
            values[MID1_Q],
            values[MID1_GAIN],
            sample_rate,
        ),
        SvfCoefficients::bell(
            freq(MID2_FREQ),
            values[MID2_Q],
            values[MID2_GAIN],
            sample_rate,
        ),
        SvfCoefficients::high_shelf(freq(HIGH_FREQ), SHELF_Q, values[HIGH_GAIN], sample_rate),
        SvfCoefficients::low_pass(freq(HIGH_CUT), CUT_Q, sample_rate),
    ]
}

/// A parametric equaliser: 12 dB/oct low and high cuts, low and high shelves, two bells and an output level.
pub struct Eq {
    values: [f32; PARAMS],
    sample_rate: f32,
    smoothers: [Smoother; PARAMS],
    smoothed: [f32; PARAMS],
    coefficients: [SvfCoefficients; FILTERS],
    filters: [Svf; FILTERS],
    output_gain: f32,
}

impl Eq {
    pub fn new() -> Eq {
        let values: [f32; PARAMS] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let sample_rate = 48_000.0;
        Eq {
            values,
            sample_rate,
            smoothers: values.map(Smoother::new),
            smoothed: values,
            coefficients: filter_coefficients(&values, sample_rate),
            filters: Default::default(),
            output_gain: db_to_linear(values[OUTPUT]),
        }
    }

    fn rebuild(&mut self) {
        self.coefficients = filter_coefficients(&self.smoothed, self.sample_rate);
        self.output_gain = db_to_linear(self.smoothed[OUTPUT]);
    }

    /// Advances every smoother one sample; true when any value moved.
    fn advance_smoothers(&mut self) -> bool {
        let mut moved = false;
        for (smoother, last) in self.smoothers.iter_mut().zip(&mut self.smoothed) {
            let value = smoother.next_value_settling();
            if value.to_bits() != last.to_bits() {
                *last = value;
                moved = true;
            }
        }
        moved
    }
}

impl Default for Eq {
    fn default() -> Eq {
        Eq::new()
    }
}

impl Module for Eq {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.sample_rate = sample_rate;
        for smoother in &mut self.smoothers {
            smoother.prepare(sample_rate);
        }
        self.rebuild();
        for filter in &mut self.filters {
            filter.reset();
        }
    }

    fn reset(&mut self) {
        for smoother in &mut self.smoothers {
            smoother.snap();
        }
        self.smoothed = self.values;
        self.rebuild();
        for filter in &mut self.filters {
            filter.reset();
        }
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        self.smoothers[idx].set_target(value);
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            if self.advance_smoothers() {
                self.rebuild();
            }
            let mut value = *sample;
            for (filter, coefficients) in self.filters.iter_mut().zip(&self.coefficients) {
                value = filter.process(coefficients, value);
            }
            *sample = value * self.output_gain;
        }
    }
}
