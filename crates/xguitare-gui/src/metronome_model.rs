use serde_json::Value;

use crate::model::{Command, KeyEvent};
use crate::project_model::project_name;

/// The meters the Metronome section offers.
pub const METER_CHOICES: [(u32, u32); 7] =
    [(2, 4), (3, 4), (4, 4), (5, 4), (6, 8), (7, 8), (12, 8)];
pub const BPM_RANGE: (f64, f64) = (20.0, 300.0);
pub const VOLUME_DB_RANGE: (f64, f64) = (-40.0, 0.0);
/// The Daemon's Metronome volume before any is chosen.
pub const DEFAULT_VOLUME_DB: f64 = -12.0;

/// The `status.metronome` object; with a Project open it mirrors the Project's tempo, meter and click.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetronomeView {
    pub on: bool,
    pub bpm: f64,
    pub meter: (u32, u32),
    pub volume_db: f64,
    /// The 1-based beat in the bar the audio thread last played; 0 while off.
    pub beat: u32,
    /// The beats played so far, wrapping; a change is a new beat even when `beat` stays the same.
    pub beats_played: u64,
}

pub fn metronome_view(status: &Value) -> Option<MetronomeView> {
    let metronome = &status["metronome"];
    let meter = &metronome["meter"];
    Some(MetronomeView {
        on: metronome["on"].as_bool()?,
        bpm: metronome["bpm"].as_f64()?,
        meter: (
            u32::try_from(meter[0].as_u64()?).ok()?,
            u32::try_from(meter[1].as_u64()?).ok()?,
        ),
        volume_db: metronome["volume_db"].as_f64()?,
        beat: u32::try_from(metronome["beat"].as_u64()?).ok()?,
        beats_played: metronome["beats_played"].as_u64()?,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BeatLight {
    #[default]
    Dark,
    /// `flash` while the beat is new since the previous status.
    Beat {
        beat: u32,
        accent: bool,
        flash: bool,
    },
}

/// The beat light for `current`, given the status received before it.
pub fn beat_light(previous: Option<&Value>, current: Option<&Value>) -> BeatLight {
    let playing = |status: Option<&Value>| {
        status
            .and_then(metronome_view)
            .filter(|view| view.on && view.beat > 0)
    };
    let Some(view) = playing(current) else {
        return BeatLight::Dark;
    };
    BeatLight::Beat {
        beat: view.beat,
        accent: view.beat == 1,
        flash: playing(previous).map(|previous| previous.beats_played) != Some(view.beats_played),
    }
}

/// M toggles the Metronome and T taps, with no Project open only: in a Project the transport's Click is the toggle.
pub fn metronome_key_to_command(key: KeyEvent, status: &Value) -> Option<Command> {
    if project_name(Some(status)).is_some() {
        return None;
    }
    match key {
        KeyEvent::M => Some(Command::Metronome {
            on: Some(!metronome_view(status).is_some_and(|view| view.on)),
            bpm: None,
            meter: None,
            volume: None,
        }),
        KeyEvent::T => Some(Command::MetronomeTap),
        _ => None,
    }
}
