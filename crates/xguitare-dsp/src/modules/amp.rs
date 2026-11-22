use std::f32::consts::TAU;

use super::db_to_linear;
use crate::amp_model::{AmpModel, MAX_STAGES, MODEL_COUNT, MODEL_LABELS, MODELS, Stack, Stage};
use crate::fmv;
use crate::oversample::Oversampler4x;
use crate::smooth::{SMOOTHING_SECONDS, Smoother};
use crate::svf::{Svf, SvfCoefficients};
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const MODEL: usize = 0;
const GAIN: usize = 1;
const BASS: usize = 2;
const MIDDLE: usize = 3;
const TREBLE: usize = 4;
const PRESENCE: usize = 5;
const MASTER: usize = 6;
const LEVEL: usize = 7;
const PARAM_COUNT: usize = 8;
const OVERSAMPLING: f32 = 4.0;
const POT_RANGE_DB: f32 = 40.0;
const MASTER_MIN_DB: f32 = -30.0;
const MASTER_RANGE_DB: f32 = 42.0;
const BRIGHT_MAX_BOOST: f32 = 3.0;
const PRESENCE_MAX_DB: f32 = 12.0;
const PRESENCE_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
const RESONANCE_MAX_DB: f32 = 4.0;
const RESONANCE_Q: f32 = 1.0;
const SAG_MAX_DB: f32 = 6.0;
const SAG_ATTACK_SECONDS: f32 = 0.010;
const SAG_RELEASE_SECONDS: f32 = 0.150;
// Below 1 because the downsampler overshoots a hard-clipped bright signal by about half.
const OUTPUT_GAIN: f32 = 0.9;

const fn percent(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        min: 0.0,
        max: 100.0,
        default: 50.0,
        unit: Unit::Percent,
        integer: false,
        labels: &[],
    }
}

pub static SPEC: ModuleSpec = ModuleSpec {
    type_name: "amp",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "model",
            min: 0.0,
            max: (MODEL_COUNT - 1) as f32,
            default: 0.0,
            unit: Unit::Linear,
            integer: true,
            labels: &MODEL_LABELS,
        },
        percent("gain"),
        percent("bass"),
        percent("middle"),
        percent("treble"),
        percent("presence"),
        percent("master"),
        ParamSpec {
            name: "level",
            min: -60.0,
            max: 12.0,
            default: -12.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
    ],
};

pub struct Amp {
    values: [f32; PARAM_COUNT],
    model: &'static AmpModel,
    oversampled_rate: f32,
    stages: [StageFilters; MAX_STAGES],
    bright: OnePole,
    pot: Smoother,
    bright_boost: Smoother,
    tone: [Smoother; 3],
    stack_state: [f64; 3],
    master: Smoother,
    presence: Smoother,
    presence_filter: Svf,
    resonance: Option<SvfCoefficients>,
    resonance_filter: Svf,
    sag_depth: f32,
    sag_attack: f32,
    sag_release: f32,
    sag_envelope: f32,
    level: Smoother,
    fade: f32,
    fade_step: f32,
    oversampler: Oversampler4x,
}

impl Amp {
    pub fn new() -> Amp {
        let values: [f32; PARAM_COUNT] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let model = &MODELS[values[MODEL] as usize];
        let mut amp = Amp {
            values,
            model,
            oversampled_rate: OVERSAMPLING * 48_000.0,
            stages: [StageFilters::default(); MAX_STAGES],
            bright: OnePole::default(),
            pot: Smoother::new(pot_gain(model, values[GAIN])),
            bright_boost: Smoother::new(bright_boost(model, values[GAIN])),
            tone: [BASS, MIDDLE, TREBLE].map(|idx| Smoother::new(values[idx] / 100.0)),
            stack_state: [0.0; 3],
            master: Smoother::new(master_gain(model, values[MASTER])),
            presence: Smoother::new(values[PRESENCE] / 100.0),
            presence_filter: Svf::new(),
            resonance: None,
            resonance_filter: Svf::new(),
            sag_depth: 0.0,
            sag_attack: 1.0,
            sag_release: 1.0,
            sag_envelope: 0.0,
            level: Smoother::new(db_to_linear(values[LEVEL])),
            fade: 1.0,
            fade_step: 1.0,
            oversampler: Oversampler4x::new(),
        };
        amp.load_model_filters();
        amp
    }

    fn load_model_filters(&mut self) {
        for (filters, stage) in self.stages.iter_mut().zip(self.model.stages) {
            *filters = StageFilters::new(stage, self.oversampled_rate);
        }
        self.bright = OnePole::new(self.model.bright_hz.unwrap_or(0.0), self.oversampled_rate);
        self.stack_state = [0.0; 3];
        let power = &self.model.power;
        self.presence_filter.reset();
        self.resonance = power.resonance_hz.map(|hz| {
            let gain_db = RESONANCE_MAX_DB * (1.0 - power.feedback);
            SvfCoefficients::bell(hz, RESONANCE_Q, gain_db, self.oversampled_rate)
        });
        self.resonance_filter.reset();
        self.sag_depth = 1.0 - db_to_linear(-SAG_MAX_DB * power.sag);
        self.sag_attack = one_pole_coeff(SAG_ATTACK_SECONDS, self.oversampled_rate);
        self.sag_release = one_pole_coeff(SAG_RELEASE_SECONDS, self.oversampled_rate);
        self.sag_envelope = 0.0;
    }

    fn change_model(&mut self, number: usize) {
        self.model = &MODELS[number];
        self.load_model_filters();
        self.pot.set_target(pot_gain(self.model, self.values[GAIN]));
        self.bright_boost
            .set_target(bright_boost(self.model, self.values[GAIN]));
        self.master
            .set_target(master_gain(self.model, self.values[MASTER]));
        self.oversampler.reset();
        self.fade = 0.0;
    }
}

impl Default for Amp {
    fn default() -> Amp {
        Amp::new()
    }
}

impl Module for Amp {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.oversampled_rate = OVERSAMPLING * sample_rate;
        self.pot.prepare(self.oversampled_rate);
        self.bright_boost.prepare(self.oversampled_rate);
        self.master.prepare(self.oversampled_rate);
        self.presence.prepare(sample_rate);
        for position in &mut self.tone {
            position.prepare(sample_rate);
        }
        self.level.prepare(sample_rate);
        self.fade_step = 1.0 / (SMOOTHING_SECONDS * sample_rate);
        self.oversampler.prepare(max_frames);
        self.load_model_filters();
    }

    fn reset(&mut self) {
        self.pot.snap();
        self.bright_boost.snap();
        self.master.snap();
        self.presence.snap();
        for position in &mut self.tone {
            position.snap();
        }
        self.level.snap();
        self.fade = 1.0;
        self.oversampler.reset();
        self.load_model_filters();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let mut value = value.clamp(spec.min, spec.max);
        match idx {
            MODEL => {
                value = value.round();
                let number = value as usize;
                if number != self.values[MODEL] as usize {
                    self.change_model(number);
                }
            }
            GAIN => {
                self.pot.set_target(pot_gain(self.model, value));
                self.bright_boost
                    .set_target(bright_boost(self.model, value));
            }
            BASS => self.tone[0].set_target(value / 100.0),
            MIDDLE => self.tone[1].set_target(value / 100.0),
            TREBLE => self.tone[2].set_target(value / 100.0),
            PRESENCE => self.presence.set_target(value / 100.0),
            MASTER => self.master.set_target(master_gain(self.model, value)),
            LEVEL => self.level.set_target(db_to_linear(value)),
            _ => unreachable!("amp has no parameter {idx}"),
        }
        self.values[idx] = value;
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        let frames = io.frames;
        let [bass, middle, treble] = self
            .tone
            .each_mut()
            .map(|position| f64::from(position.advance(frames)));
        let Stack::Fmv {
            values,
            middle: fixed_middle,
        } = &self.model.stack;
        let middle = fixed_middle.map_or(middle, f64::from);
        let coefficients = fmv::bilinear(
            fmv::analog_coefficients(values, treble, middle, bass),
            2.0 * f64::from(self.oversampled_rate),
        );
        let model = self.model;
        let presence = SvfCoefficients::high_shelf(
            model.power.presence_hz,
            PRESENCE_Q,
            PRESENCE_MAX_DB * self.presence.advance(frames),
            self.oversampled_rate,
        );
        let feedback = model.power.feedback;
        let mono = &mut io.mono[..frames];
        for sample in mono.iter_mut() {
            *sample *= self.fade;
            self.fade = (self.fade + self.fade_step).min(1.0);
        }
        for sample in self.oversampler.upsample(mono) {
            let pot = self.pot.next_value();
            let bright_boost = self.bright_boost.next_value();
            let master = self.master.next_value();
            let mut x = *sample;
            for (idx, filters) in self.stages[..model.stages.len()].iter_mut().enumerate() {
                x = filters.process(x);
                if idx == 0 {
                    x = pot * (x + bright_boost * self.bright.high_pass(x));
                }
                if idx == model.stack_after {
                    x = fmv::step(&coefficients, &mut self.stack_state, f64::from(x)) as f32;
                }
            }
            x = self.presence_filter.process(&presence, x);
            if let Some(resonance) = &self.resonance {
                x = self.resonance_filter.process(resonance, x);
            }
            let sag = 1.0 - self.sag_depth * self.sag_envelope.min(1.0);
            let y = push_pull(sag * master * x, feedback);
            let rate = if y.abs() > self.sag_envelope {
                self.sag_attack
            } else {
                self.sag_release
            };
            self.sag_envelope += rate * (y.abs() - self.sag_envelope);
            *sample = OUTPUT_GAIN * y;
        }
        self.oversampler.downsample(mono);

        for sample in mono.iter_mut() {
            *sample *= self.level.next_value();
        }
    }
}

fn pot_gain(model: &AmpModel, gain: f32) -> f32 {
    db_to_linear(POT_RANGE_DB * (gain / 100.0 - 1.0) - model.pot_loss_db)
}

/// The treble bypass across the `gain` pot: full at 0, gone at 100.
fn bright_boost(model: &AmpModel, gain: f32) -> f32 {
    match model.bright_hz {
        Some(_) => BRIGHT_MAX_BOOST * (1.0 - gain / 100.0),
        None => 0.0,
    }
}

fn master_gain(model: &AmpModel, master: f32) -> f32 {
    db_to_linear(MASTER_MIN_DB + MASTER_RANGE_DB * master / 100.0 - model.power.headroom_db)
}

/// Odd, bounded to ±1: `tanh` at `feedback` 0, blended towards `x / (1 + x^16)^(1/16)`, near a hard clip, at 1.
fn push_pull(x: f32, feedback: f32) -> f32 {
    let x2 = x * x;
    let x4 = x2 * x2;
    let x8 = x4 * x4;
    let hard = x / (1.0 + x8 * x8).sqrt().sqrt().sqrt().sqrt();
    (1.0 - feedback) * x.tanh() + feedback * hard
}

fn one_pole_coeff(seconds: f32, sample_rate: f32) -> f32 {
    1.0 - (-1.0 / (seconds * sample_rate)).exp()
}

#[derive(Clone, Copy, Default)]
struct OnePole {
    coeff: f32,
    state: f32,
}

impl OnePole {
    fn new(hz: f32, sample_rate: f32) -> OnePole {
        OnePole {
            coeff: 1.0 - (-TAU * hz / sample_rate).exp(),
            state: 0.0,
        }
    }

    fn low_pass(&mut self, x: f32) -> f32 {
        self.state += self.coeff * (x - self.state);
        self.state
    }

    fn high_pass(&mut self, x: f32) -> f32 {
        x - self.low_pass(x)
    }
}

#[derive(Clone, Copy, Default)]
struct StageFilters {
    low_cut: OnePole,
    shelf: OnePole,
    shelf_depth: f32,
    gain: f32,
    bias: f32,
    rest: f32,
    high_cut: OnePole,
}

impl StageFilters {
    fn new(stage: &Stage, sample_rate: f32) -> StageFilters {
        let (shelf_hz, cut_db) = stage
            .shelf
            .as_ref()
            .map_or((0.0, 0.0), |s| (s.hz, s.cut_db));
        StageFilters {
            low_cut: OnePole::new(stage.low_cut_hz, sample_rate),
            shelf: OnePole::new(shelf_hz, sample_rate),
            shelf_depth: 1.0 - db_to_linear(-cut_db),
            gain: db_to_linear(stage.gain_db),
            bias: stage.bias,
            rest: stage.bias.tanh(),
            high_cut: OnePole::new(stage.high_cut_hz, sample_rate),
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let x = self.low_cut.high_pass(x);
        let x = x - self.shelf_depth * self.shelf.low_pass(x);
        let shaped = (self.gain * x + self.bias).tanh() - self.rest;
        self.high_cut.low_pass(shaped)
    }
}
