use super::{db_to_linear, one_pole_coeff};
use crate::smooth::Smoother;
use crate::window_peak::WindowPeak;
use crate::{AudioBuffers, Channels, Module, ModuleSpec, ParamSpec, Unit};

const ATTACK: usize = 0;
const SUSTAIN: usize = 1;
const OUTPUT: usize = 2;
const MIX: usize = 3;
const FAST_SECONDS: f32 = 0.001;
const SLOW_SECONDS: f32 = 0.040;
const RELEASE_SECONDS: f32 = 0.150;
/// Half a period at 20 Hz: every window of a steady note holds a crest, so the envelopes see a steady level, not each half cycle.
const HOLD_SECONDS: f32 = 0.025;
/// An envelope difference of this many dB applies the full `attack` or `sustain` gain; a smaller one a proportional part.
const FULL_DIFFERENCE_DB: f32 = 6.0;
const LEVEL_FLOOR: f32 = 1e-9;
const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

pub static SPEC: ModuleSpec = ModuleSpec {
    type_name: "shaper",
    channels: Channels::Mono,
    params: &[
        ParamSpec {
            name: "attack",
            min: -15.0,
            max: 15.0,
            default: 0.0,
            unit: Unit::Db,
            integer: false,
            labels: &[],
        },
        ParamSpec {
            name: "sustain",
            min: -24.0,
            max: 24.0,
            default: 0.0,
            unit: Unit::Db,
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
        ParamSpec {
            name: "mix",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            unit: Unit::Percent,
            integer: false,
            labels: &[],
        },
    ],
};

/// A one-pole follower with separate rise and fall coefficients.
#[derive(Clone, Copy)]
struct Envelope {
    rise: f32,
    fall: f32,
    value: f32,
}

impl Envelope {
    fn next(&mut self, level: f32) -> f32 {
        let coeff = if level > self.value {
            self.rise
        } else {
            self.fall
        };
        self.value = level + coeff * (self.value - level);
        self.value
    }
}

fn level_db(level: f32) -> f32 {
    20.0 * level.max(LEVEL_FLOOR).log10()
}

/// A transient shaper after the SPL method, detecting on the input's peak over the last 25 ms: `attack` follows the gap between a fast envelope and a slow-rising one, `sustain` the gap between that fast envelope, which holds peaks for the release time, and a follower that falls with the signal; gaps in dB do not depend on the input level.
pub struct Shaper {
    values: [f32; 4],
    attack: Smoother,
    sustain: Smoother,
    output: Smoother,
    mix: Smoother,
    peak: Envelope,
    slow: Envelope,
    follower: Envelope,
    hold: WindowPeak,
    hold_frames: usize,
    gain_coeff: f32,
    gain_db: f32,
}

impl Shaper {
    pub fn new() -> Shaper {
        let values: [f32; 4] = std::array::from_fn(|idx| SPEC.params[idx].default);
        let idle = Envelope {
            rise: 0.0,
            fall: 0.0,
            value: 0.0,
        };
        let mut shaper = Shaper {
            values,
            attack: Smoother::new(values[ATTACK]),
            sustain: Smoother::new(values[SUSTAIN]),
            output: Smoother::new(values[OUTPUT]),
            mix: Smoother::new(values[MIX] / 100.0),
            peak: idle,
            slow: idle,
            follower: idle,
            hold: WindowPeak::new(0),
            hold_frames: 0,
            gain_coeff: 0.0,
            gain_db: 0.0,
        };
        shaper.prepare(DEFAULT_SAMPLE_RATE, 0);
        shaper
    }

    fn clear(&mut self) {
        for envelope in [&mut self.peak, &mut self.slow, &mut self.follower] {
            envelope.value = 0.0;
        }
        self.hold.clear();
        self.gain_db = 0.0;
    }

    /// Shapes one sample of each side in `frame`, detecting on the louder one.
    fn shape<const N: usize>(&mut self, frame: [&mut f32; N]) {
        let attack = self.attack.next_value();
        let sustain = self.sustain.next_value();
        let output = db_to_linear(self.output.next_value());
        let mix = self.mix.next_value();
        let rectified = frame.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let level = self.hold.push(rectified, self.hold_frames);
        let peak_level = self.peak.next(level);
        let peak = level_db(peak_level);
        let onset = peak - level_db(self.slow.next(peak_level));
        let tail = peak - level_db(self.follower.next(level));
        let target = attack * share(onset) + sustain * share(tail);
        self.gain_db = target + self.gain_coeff * (self.gain_db - target);
        let gain = db_to_linear(self.gain_db);
        for sample in frame {
            let dry = *sample;
            *sample = (dry + mix * (dry * gain - dry)) * output;
        }
    }
}

fn share(difference_db: f32) -> f32 {
    (difference_db / FULL_DIFFERENCE_DB).clamp(0.0, 1.0)
}

impl Default for Shaper {
    fn default() -> Shaper {
        Shaper::new()
    }
}

impl Module for Shaper {
    fn spec(&self) -> &'static ModuleSpec {
        &SPEC
    }

    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        for smoother in [
            &mut self.attack,
            &mut self.sustain,
            &mut self.output,
            &mut self.mix,
        ] {
            smoother.prepare(sample_rate);
        }
        let fast = one_pole_coeff(FAST_SECONDS, sample_rate);
        let slow = one_pole_coeff(SLOW_SECONDS, sample_rate);
        let release = one_pole_coeff(RELEASE_SECONDS, sample_rate);
        self.peak = Envelope {
            rise: fast,
            fall: release,
            value: 0.0,
        };
        self.slow = Envelope {
            rise: slow,
            fall: release,
            value: 0.0,
        };
        self.follower = Envelope {
            rise: fast,
            fall: slow,
            value: 0.0,
        };
        self.hold_frames = (HOLD_SECONDS * sample_rate).ceil() as usize;
        self.hold = WindowPeak::new(self.hold_frames + 1);
        self.gain_coeff = fast;
        self.clear();
    }

    fn reset(&mut self) {
        for smoother in [
            &mut self.attack,
            &mut self.sustain,
            &mut self.output,
            &mut self.mix,
        ] {
            smoother.snap();
        }
        self.clear();
    }

    fn set_param(&mut self, idx: usize, value: f32) {
        let spec = &SPEC.params[idx];
        let value = value.clamp(spec.min, spec.max);
        self.values[idx] = value;
        match idx {
            ATTACK => self.attack.set_target(value),
            SUSTAIN => self.sustain.set_target(value),
            OUTPUT => self.output.set_target(value),
            _ => self.mix.set_target(value / 100.0),
        }
    }

    fn param(&self, idx: usize) -> f32 {
        self.values[idx]
    }

    fn process(&mut self, io: &mut AudioBuffers) {
        for sample in &mut io.mono[..io.frames] {
            self.shape([sample]);
        }
    }

    fn process_linked(&mut self, left: &mut [f32], right: &mut [f32]) -> bool {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.shape([l, r]);
        }
        true
    }
}
