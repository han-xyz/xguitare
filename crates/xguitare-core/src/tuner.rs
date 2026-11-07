use rtrb::Consumer;
use xguitare_dsp::tuner::{Detector, note_name};

/// The Tuner on the control thread: drains the input tap into the Detector on each `tick` and keeps the last reading.
pub struct Tuner {
    tap: Consumer<f32>,
    detector: Detector,
    reference_hz: f32,
    last: Option<Reading>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub hz: f32,
    /// Note name and octave, e.g. `E1`.
    pub note: String,
    pub cents: i32,
    pub confident: bool,
}

impl Tuner {
    pub fn new(tap: Consumer<f32>, sample_rate: f32, reference_hz: f32) -> Tuner {
        Tuner {
            tap,
            detector: Detector::new(sample_rate),
            reference_hz,
            last: None,
        }
    }

    /// Restarts the Detector at `sample_rate`, dropping the samples it and the tap hold and the last reading.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if let Ok(chunk) = self.tap.read_chunk(self.tap.slots()) {
            chunk.commit_all();
        }
        self.detector = Detector::new(sample_rate);
        self.last = None;
    }

    /// Feeds every sample waiting in the tap to the Detector and replaces the reading; `None` when no pitch is detected.
    pub fn tick(&mut self) {
        if let Ok(chunk) = self.tap.read_chunk(self.tap.slots()) {
            let (first, second) = chunk.as_slices();
            self.detector.push(first);
            self.detector.push(second);
            chunk.commit_all();
        }
        self.last = self.detector.analyse().map(|pitch| {
            let note = note_name(pitch.hz, self.reference_hz);
            Reading {
                hz: pitch.hz,
                note: note.to_string(),
                cents: note.cents,
                confident: pitch.confident,
            }
        });
    }

    pub fn reading(&self) -> Option<&Reading> {
        self.last.as_ref()
    }
}
