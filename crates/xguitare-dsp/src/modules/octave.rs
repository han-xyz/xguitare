use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2};

use super::one_pole_coeff;
use crate::smooth::Smoother;
use crate::svf::{Svf, SvfCoefficients};
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const DRY: usize = 0;
const OCT1: usize = 1;
const OCT2: usize = 2;
const UP: usize = 3;
const TONE: usize = 4;
const RANGE: usize = 5;
const INPUT_HIGH_PASS_HZ: f32 = 20.0;
const UP_LOW_PASS_HZ: f32 = 5000.0;
/// A rectified sine with its mean removed peaks at 2/π of the sine's peak.
const UP_GAIN: f32 = FRAC_PI_2;
/// −70 dBFS: below it the tracker forgets the note and the sub voices fade out.
const GATE_LEVEL: f32 = 3.162e-4;
/// After an event, events sooner than this fraction of the last period are a harmonic's lobe, not the next cycle; under one half, so that one missed event (a doubled period) cannot lock the tracker onto every other cycle.
const HOLD_OFF_FRACTION: f32 = 0.45;
/// The −1 voice switches the note through a 4-pole low-pass at this multiple of the tracked note, so that a strong second harmonic does not land at 3f/2.
const FOLLOW_RATIO: f32 = 1.3;

/// The tracker's constants for one `range`.
struct Tracking {
    pre_filter_hz: f32,
    lowest_hz: f32,
    highest_hz: f32,
    peak_decay_seconds: f32,
    envelope_seconds: f32,
}

const TRACKING: [Tracking; 2] = [
    Tracking {
        pre_filter_hz: 180.0,
        lowest_hz: 28.0,
        highest_hz: 450.0,
        peak_decay_seconds: 0.08,
        envelope_seconds: 0.015,
    },
    Tracking {
        pre_filter_hz: 400.0,
        lowest_hz: 70.0,
        highest_hz: 1100.0,
        peak_decay_seconds: 0.04,
        envelope_seconds: 0.008,
    },
];

pub static SPEC: ModuleSpec = ModuleSpec {
    type_name: "octave",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "dry",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "oct1",
            min: 0.0,
            max: 100.0,
            default: 70.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "oct2",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "up",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "tone",
            min: 100.0,
            max: 2000.0,
            default: 500.0,
            unit: Unit::Hz,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "range",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: Unit::Linear,
            integer: true,
            labels: &["bass", "guitar"],
        },
    ],
};

/// A tracking octaver: a divide-by-two and a divide-by-four of the played note under the dry signal, and a rectified octave up.
pub struct Octave {
    values: [f32; 6],
    sample_rate: f32,
    dry: Smoother,
    oct1: Smoother,
    oct2: Smoother,
    up: Smoother,
    tone: Smoother,
    gate: Smoother,
    input_high_pass_coefficients: SvfCoefficients,
    pre_filter_coefficients: SvfCoefficients,
    up_low_pass_coefficients: SvfCoefficients,
    input_high_pass: Svf,
    pre_filter: Svf,
    up_high_pass: Svf,
    up_low_pass: Svf,
    oct1_tone: Svf,
    oct2_tone: Svf,
    follow: [Svf; 2],
    tracker: Tracker,
}

impl Octave {
    pub fn new() -> Octave {
        let values: [f32; 6] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let sample_rate = 48_000.0;
        let mut octave = Octave {
            values,
            sample_rate,
            dry: Smoother::new(values[DRY] / 100.0),
            oct1: Smoother::new(values[OCT1] / 100.0),
            oct2: Smoother::new(values[OCT2] / 100.0),
            up: Smoother::new(values[UP] / 100.0),
            tone: Smoother::new(values[TONE]),
            gate: Smoother::new(0.0),
            input_high_pass_coefficients: SvfCoefficients::high_pass(
                INPUT_HIGH_PASS_HZ,
                FRAC_1_SQRT_2,
                sample_rate,
            ),
            pre_filter_coefficients: SvfCoefficients::low_pass(
                TRACKING[0].pre_filter_hz,
                FRAC_1_SQRT_2,
                sample_rate,
            ),
            up_low_pass_coefficients: SvfCoefficients::low_pass(
                UP_LOW_PASS_HZ,
                FRAC_1_SQRT_2,
                sample_rate,
            ),
            input_high_pass: Svf::new(),
            pre_filter: Svf::new(),
            up_high_pass: Svf::new(),
            up_low_pass: Svf::new(),
            oct1_tone: Svf::new(),
            oct2_tone: Svf::new(),
            follow: [Svf::new(), Svf::new()],
            tracker: Tracker::default(),
        };
        octave.update_tracking();
        octave.reset();
        octave
    }

    fn update_tracking(&mut self) {
        let tracking = &TRACKING[self.values[RANGE] as usize];
        self.pre_filter_coefficients =
            SvfCoefficients::low_pass(tracking.pre_filter_hz, FRAC_1_SQRT_2, self.sample_rate);
        self.tracker.prepare(tracking, self.sample_rate);
    }

    fn follow_target_hz(&self) -> f32 {
        FOLLOW_RATIO * self.sample_rate / self.tracker.period
    }

    /// The pre-filtered note through the low-pass that follows the tracked note.
    fn followed(&mut self, pre_filtered: f32) -> f32 {
        let coefficients =
            SvfCoefficients::low_pass(self.follow_target_hz(), FRAC_1_SQRT_2, self.sample_rate);
        self.follow.iter_mut().fold(pre_filtered, |signal, filter| {
            filter.process(&coefficients, signal)
        })
    }

    fn smoothers(&mut self) -> [&mut Smoother; 6] {
        [
            &mut self.dry,
            &mut self.oct1,
            &mut self.oct2,
            &mut self.up,
            &mut self.tone,
            &mut self.gate,
        ]
    }

    fn up_voice(&mut self, input: f32) -> f32 {
        let rectified = self
            .up_high_pass
            .process(&self.input_high_pass_coefficients, input.abs());
        UP_GAIN
            * self
                .up_low_pass
                .process(&self.up_low_pass_coefficients, rectified)
    }
}

impl Default for Octave {
    fn default() -> Octave {
        Octave::new()
    }
}

impl Module for Octave {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.sample_rate = sample_rate;
        for smoother in self.smoothers() {
            smoother.prepare(sample_rate);
        }
        self.input_high_pass_coefficients =
            SvfCoefficients::high_pass(INPUT_HIGH_PASS_HZ, FRAC_1_SQRT_2, sample_rate);
        self.up_low_pass_coefficients =
            SvfCoefficients::low_pass(UP_LOW_PASS_HZ, FRAC_1_SQRT_2, sample_rate);
        self.update_tracking();
        self.reset();
    }

    fn reset(&mut self) {
        self.gate.set_target(0.0);
        for smoother in self.smoothers() {
            smoother.snap();
        }
        for filter in [
            &mut self.input_high_pass,
            &mut self.pre_filter,
            &mut self.up_high_pass,
            &mut self.up_low_pass,
            &mut self.oct1_tone,
            &mut self.oct2_tone,
        ] {
            filter.reset();
        }
        for filter in &mut self.follow {
            filter.reset();
        }
        self.tracker.reset();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = if spec.integer { value.round() } else { value };
        match idx {
            DRY => self.dry.set_target(value / 100.0),
            OCT1 => self.oct1.set_target(value / 100.0),
            OCT2 => self.oct2.set_target(value / 100.0),
            UP => self.up.set_target(value / 100.0),
            TONE => self.tone.set_target(value),
            RANGE => self.update_tracking(),
            _ => unreachable!("octave has no parameter {idx}"),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            let input = *sample;
            let high_passed = self
                .input_high_pass
                .process(&self.input_high_pass_coefficients, input);
            let pre_filtered = self
                .pre_filter
                .process(&self.pre_filter_coefficients, high_passed);
            let followed = self.followed(pre_filtered);
            let (sub1, sub2) = self.tracker.process(pre_filtered, followed);
            self.gate
                .set_target(if self.tracker.is_open() { 1.0 } else { 0.0 });
            let gate = self.gate.next_value();
            let tone =
                SvfCoefficients::low_pass(self.tone.next_value(), FRAC_1_SQRT_2, self.sample_rate);
            let sub1 = self.oct1_tone.process(&tone, gate * sub1);
            let sub2 = self.oct2_tone.process(&tone, gate * sub2);
            let up = self.up_voice(input);
            *sample = self.dry.next_value_settling() * input
                + self.oct1.next_value_settling() * sub1
                + self.oct2.next_value_settling() * sub2
                + self.up.next_value_settling() * up;
        }
    }
}

/// Divides the pre-filtered note by two and four: a decaying peak-hold detector fires one event per cycle, each event toggles flip-flop A, each rising edge of A toggles B.
#[derive(Default)]
struct Tracker {
    peak_decay_coeff: f32,
    envelope_coeff: f32,
    shortest_period: f32,
    longest_period: f32,
    peak: f32,
    armed: bool,
    since_event: f32,
    hold_off: f32,
    period: f32,
    flip_a: bool,
    flip_b: bool,
    sub1_sign: f32,
    sub2_sign: f32,
    previous: f32,
    envelope: f32,
}

impl Tracker {
    fn prepare(&mut self, tracking: &Tracking, sample_rate: f32) {
        self.peak_decay_coeff = one_pole_coeff(tracking.peak_decay_seconds, sample_rate);
        self.envelope_coeff = one_pole_coeff(tracking.envelope_seconds, sample_rate);
        self.shortest_period = sample_rate / tracking.highest_hz;
        self.longest_period = sample_rate / tracking.lowest_hz;
    }

    fn reset(&mut self) {
        self.peak = 0.0;
        self.envelope = 0.0;
        self.previous = 0.0;
        self.forget_note();
    }

    fn forget_note(&mut self) {
        self.armed = true;
        self.since_event = f32::INFINITY;
        self.hold_off = 0.0;
        self.period = self.shortest_period;
        self.flip_a = false;
        self.flip_b = false;
        self.sub1_sign = 1.0;
        self.sub2_sign = 1.0;
    }

    fn is_open(&self) -> bool {
        self.peak >= GATE_LEVEL
    }

    /// The −1 voice (`followed` with A's polarity) and the −2 voice (B as a square at the input's envelope), before `tone`.
    fn process(&mut self, input: f32, followed: f32) -> (f32, f32) {
        self.since_event += 1.0;
        if input > self.peak {
            if self.armed && self.since_event >= self.hold_off {
                self.fire();
            }
            self.armed = false;
            self.peak = input;
        } else {
            self.peak *= self.peak_decay_coeff;
        }
        if input < 0.0 {
            self.armed = true;
        }
        if !self.is_open() {
            self.forget_note();
        }
        // Flipping at a rising zero crossing keeps f/2 above 3f/2 when the note has a strong second harmonic; at a falling one it does not.
        if self.previous < 0.0 && followed >= 0.0 {
            self.sub1_sign = if self.flip_a { -1.0 } else { 1.0 };
            self.sub2_sign = if self.flip_b { -1.0 } else { 1.0 };
        }
        self.previous = followed;
        let level = input.abs();
        self.envelope = level + self.envelope_coeff * (self.envelope - level);
        (self.sub1_sign * followed, self.sub2_sign * self.envelope)
    }

    fn fire(&mut self) {
        if self.since_event <= self.longest_period {
            self.period = self.since_event.max(self.shortest_period);
        }
        self.hold_off = HOLD_OFF_FRACTION * self.period;
        self.since_event = 0.0;
        self.flip_a = !self.flip_a;
        if self.flip_a {
            self.flip_b = !self.flip_b;
        }
    }
}
