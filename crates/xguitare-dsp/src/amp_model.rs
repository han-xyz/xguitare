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
];

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
