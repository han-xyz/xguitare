use crate::fmv::FmvStack;

pub const MAX_STAGES: usize = 4;

/// The circuit facts of one amplifier family, one entry of [`MODELS`]. Frequencies in Hz, levels in dB.
#[derive(Debug)]
pub struct AmpModel {
    pub name: &'static str,
    pub stages: &'static [Stage],
    /// Loss of the circuit around the `gain` pot, subtracted from its −40..0 dB.
    pub pot_loss_db: f32,
    /// Corner of the treble bypass across the `gain` pot; `None` when the amp has none.
    pub bright_hz: Option<f32>,
    /// Index of the stage the tone stack follows.
    pub stack_after: usize,
    pub stack: Stack,
    pub power: PowerAmp,
}

/// One gain stage: high-pass, optional shelf, gain, `tanh(g·x + bias) − tanh(bias)`, low-pass.
#[derive(Debug)]
pub struct Stage {
    pub low_cut_hz: f32,
    pub shelf: Option<LowShelf>,
    pub gain_db: f32,
    pub bias: f32,
    pub high_cut_hz: f32,
}

/// A partly bypassed cathode: lows below `hz` come out `cut_db` quieter.
#[derive(Debug)]
pub struct LowShelf {
    pub hz: f32,
    pub cut_db: f32,
}

#[derive(Debug)]
pub enum Stack {
    /// The passive treble/middle/bass circuit; `middle: Some(position)` fixes the mid pot for an amp without one.
    Fmv {
        values: FmvStack,
        middle: Option<f32>,
    },
    /// An active three-band circuit: low shelf, mid peak, high shelf; each pot at 50 leaves its band flat.
    Bands { low: Band, mid: Band, high: Band },
}

/// One band of an active tone circuit: its pot at 100 gives `boost_db`, at 0 `cut_db` less (a positive number).
/// A shelf's `hz` is where it has reached its gain, as an amp manual quotes it, so the filter's midpoint
/// sits one octave inward; a peak's `hz` is its centre.
#[derive(Debug)]
pub struct Band {
    pub hz: f32,
    pub boost_db: f32,
    pub cut_db: f32,
    pub q: f32,
}

#[derive(Debug)]
pub struct PowerAmp {
    /// Level of the tone stack's output, in dBFS, that `master` at 0 dB drives to the power amp's limit.
    pub headroom_db: f32,
    pub feedback: f32,
    pub presence_hz: f32,
    pub resonance_hz: Option<f32>,
    pub sag: f32,
}

/// Every Amp model, numbered by position: entries are only ever appended, because a Preset stores the number.
pub static MODELS: &[AmpModel] = &[
    // Bassman 5F6-A: tone stack values from Yeh and Smith, DAFx 2006; 100 pF bright cap on the volume pot.
    AmpModel {
        name: "tweed",
        stages: &[
            Stage {
                low_cut_hz: 5.0,
                shelf: None,
                gain_db: 6.0,
                bias: 0.05,
                high_cut_hz: 10_000.0,
            },
            Stage {
                low_cut_hz: 10.0,
                shelf: None,
                gain_db: 36.0,
                bias: 0.1,
                high_cut_hz: 7_000.0,
            },
        ],
        pot_loss_db: 0.0,
        bright_hz: Some(2_000.0),
        stack_after: 1,
        stack: Stack::Fmv {
            values: FmvStack {
                r1: 250e3,
                r2: 1e6,
                r3: 25e3,
                r4: 56e3,
                c1: 250e-12,
                c2: 20e-9,
                c3: 20e-9,
            },
            middle: None,
        },
        // Presence: 5k with 0.1 uF in the feedback loop.
        power: PowerAmp {
            headroom_db: -9.0,
            feedback: 0.4,
            presence_hz: 320.0,
            resonance_hz: Some(100.0),
            sag: 0.5,
        },
    },
    // Blackface AB763: 25 uF cathode bypass and 47 nF coupling; the amp has no presence pot.
    AmpModel {
        name: "american",
        stages: &[
            Stage {
                low_cut_hz: 4.0,
                shelf: None,
                gain_db: 6.0,
                bias: 0.0,
                high_cut_hz: 12_000.0,
            },
            Stage {
                low_cut_hz: 15.0,
                shelf: None,
                gain_db: 45.0,
                bias: 0.0,
                high_cut_hz: 9_000.0,
            },
        ],
        pot_loss_db: 0.0,
        bright_hz: None,
        stack_after: 0,
        stack: Stack::Fmv {
            values: FmvStack {
                r1: 250e3,
                r2: 250e3,
                r3: 10e3,
                r4: 100e3,
                c1: 250e-12,
                c2: 100e-9,
                c3: 47e-9,
            },
            middle: None,
        },
        power: PowerAmp {
            headroom_db: -9.0,
            feedback: 0.8,
            presence_hz: 2_000.0,
            resonance_hz: Some(110.0),
            sag: 0.3,
        },
    },
    // Vox AC30 Top Boost: 25 uF cathode bypass, 47 nF coupling, 100 pF bright cap; no mid pot, no feedback.
    AmpModel {
        name: "chime",
        stages: &[
            Stage {
                low_cut_hz: 4.0,
                shelf: None,
                gain_db: 6.0,
                bias: 0.05,
                high_cut_hz: 12_000.0,
            },
            Stage {
                low_cut_hz: 15.0,
                shelf: None,
                gain_db: 44.0,
                bias: 0.1,
                high_cut_hz: 9_000.0,
            },
        ],
        pot_loss_db: 0.0,
        bright_hz: Some(2_500.0),
        stack_after: 1,
        stack: Stack::Fmv {
            values: FmvStack {
                r1: 1e6,
                r2: 1e6,
                r3: 10e3,
                r4: 100e3,
                c1: 50e-12,
                c2: 22e-9,
                c3: 22e-9,
            },
            middle: Some(1.0),
        },
        power: PowerAmp {
            headroom_db: -11.0,
            feedback: 0.0,
            presence_hz: 2_000.0,
            resonance_hz: Some(90.0),
            sag: 0.5,
        },
    },
    // Marshall 1959 Super Lead: 2.2 nF coupling, 2.7k with 0.68 uF cathode, 470 pF bright cap.
    // Tone stack values are the 2203's; the 1959's own were not found.
    AmpModel {
        name: "crunch",
        stages: &[
            Stage {
                low_cut_hz: 72.0,
                shelf: Some(LowShelf {
                    hz: 87.0,
                    cut_db: 6.0,
                }),
                gain_db: 6.0,
                bias: 0.1,
                high_cut_hz: 10_000.0,
            },
            Stage {
                low_cut_hz: 7.0,
                shelf: None,
                gain_db: 45.0,
                bias: 0.15,
                high_cut_hz: 7_000.0,
            },
        ],
        pot_loss_db: 0.0,
        bright_hz: Some(1_500.0),
        stack_after: 1,
        stack: Stack::Fmv {
            values: LEAD_STACK,
            middle: None,
        },
        // Presence: 22k in parallel with 4.7k, with 0.1 uF in the feedback loop.
        power: PowerAmp {
            headroom_db: -9.0,
            feedback: 0.5,
            presence_hz: 400.0,
            resonance_hz: Some(100.0),
            sag: 0.3,
        },
    },
    // Marshall JCM800 2203: 470k/500k attenuator before the gain pot; a cold clipper on a 10k cathode.
    AmpModel {
        name: "lead",
        stages: &[
            Stage {
                low_cut_hz: 7.0,
                shelf: Some(LowShelf {
                    hz: 87.0,
                    cut_db: 6.0,
                }),
                gain_db: 6.0,
                bias: 0.1,
                high_cut_hz: 10_000.0,
            },
            Stage {
                low_cut_hz: 10.0,
                shelf: None,
                gain_db: 24.0,
                bias: -0.6,
                high_cut_hz: 8_000.0,
            },
            Stage {
                low_cut_hz: 7.0,
                shelf: None,
                gain_db: 18.0,
                bias: 0.1,
                high_cut_hz: 6_000.0,
            },
        ],
        pot_loss_db: 6.0,
        bright_hz: Some(1_500.0),
        stack_after: 2,
        stack: Stack::Fmv {
            values: LEAD_STACK,
            middle: None,
        },
        power: PowerAmp {
            headroom_db: -9.0,
            feedback: 0.5,
            presence_hz: 400.0,
            resonance_hz: Some(100.0),
            sag: 0.1,
        },
    },
    // Soldano SLO-100 and Mesa Dual Rectifier: 1.8k with 1 uF on the first cathode, a cold clipper
    // on 39k, 2 nF bright cap. Tone stack values from CAPS, not checked against a schematic.
    AmpModel {
        name: "modern",
        stages: &[
            Stage {
                low_cut_hz: 7.0,
                shelf: Some(LowShelf {
                    hz: 88.0,
                    cut_db: 6.0,
                }),
                gain_db: 6.0,
                bias: 0.1,
                high_cut_hz: 10_000.0,
            },
            Stage {
                low_cut_hz: 20.0,
                shelf: None,
                gain_db: 14.0,
                bias: 0.1,
                high_cut_hz: 8_000.0,
            },
            Stage {
                low_cut_hz: 20.0,
                shelf: None,
                gain_db: 12.0,
                bias: -0.8,
                high_cut_hz: 6_000.0,
            },
            Stage {
                low_cut_hz: 20.0,
                shelf: None,
                gain_db: 20.0,
                bias: 0.1,
                high_cut_hz: 5_000.0,
            },
        ],
        pot_loss_db: 6.0,
        bright_hz: Some(1_300.0),
        stack_after: 3,
        stack: Stack::Fmv {
            values: FmvStack {
                r1: 250e3,
                r2: 1e6,
                r3: 25e3,
                r4: 47e3,
                c1: 600e-12,
                c2: 20e-9,
                c3: 20e-9,
            },
            middle: None,
        },
        power: PowerAmp {
            headroom_db: -16.0,
            feedback: 0.9,
            presence_hz: 600.0,
            resonance_hz: Some(80.0),
            sag: 0.0,
        },
    },
    // Ampeg SVT: 12DW7 input stage on an unbypassed 3.3k cathode, 0.1 uF couplers, 47k feedback.
    // The tone circuit's corner frequencies are not on the schematic; these are voiced.
    AmpModel {
        name: "bass-valve",
        stages: &[
            Stage {
                low_cut_hz: 2.0,
                shelf: None,
                gain_db: 6.0,
                bias: 0.0,
                high_cut_hz: 10_000.0,
            },
            Stage {
                low_cut_hz: 2.0,
                shelf: None,
                gain_db: 40.0,
                bias: 0.05,
                high_cut_hz: 8_000.0,
            },
        ],
        pot_loss_db: 0.0,
        bright_hz: None,
        stack_after: 1,
        stack: Stack::Bands {
            low: Band {
                hz: 80.0,
                boost_db: 12.0,
                cut_db: 12.0,
                q: SHELF_Q,
            },
            mid: Band {
                hz: 800.0,
                boost_db: 10.0,
                cut_db: 10.0,
                q: 0.7,
            },
            high: Band {
                hz: 3_000.0,
                boost_db: 12.0,
                cut_db: 12.0,
                q: SHELF_Q,
            },
        },
        power: PowerAmp {
            headroom_db: -9.0,
            feedback: 0.6,
            presence_hz: 2_000.0,
            resonance_hz: Some(60.0),
            sag: 0.1,
        },
    },
    // Gallien-Krueger: the three bands of the MB 108 manual (the 800RB's own were not found).
    AmpModel {
        name: "bass-solid",
        stages: &[Stage {
            low_cut_hz: 20.0,
            shelf: None,
            gain_db: 6.0,
            bias: 0.0,
            high_cut_hz: 10_000.0,
        }],
        pot_loss_db: 0.0,
        bright_hz: None,
        stack_after: 0,
        stack: Stack::Bands {
            low: Band {
                hz: 50.0,
                boost_db: 10.0,
                cut_db: 10.0,
                q: SHELF_Q,
            },
            mid: Band {
                hz: 500.0,
                boost_db: 6.0,
                cut_db: 10.0,
                q: 0.7,
            },
            high: Band {
                hz: 8_000.0,
                boost_db: 14.0,
                cut_db: 14.0,
                q: SHELF_Q,
            },
        },
        power: PowerAmp {
            headroom_db: -38.0,
            feedback: 1.0,
            presence_hz: 4_000.0,
            resonance_hz: None,
            sag: 0.0,
        },
    },
];

const SHELF_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

// Marshall JCM800 2203 tone stack, also used by `crunch`.
const LEAD_STACK: FmvStack = FmvStack {
    r1: 220e3,
    r2: 1e6,
    r3: 22e3,
    r4: 33e3,
    c1: 470e-12,
    c2: 22e-9,
    c3: 22e-9,
};

pub const MODEL_COUNT: usize = MODELS.len();

pub static MODEL_LABELS: [&str; MODEL_COUNT] = {
    let mut labels = [""; MODEL_COUNT];
    let mut idx = 0;
    while idx < MODEL_COUNT {
        labels[idx] = MODELS[idx].name;
        idx += 1;
    }
    labels
};
