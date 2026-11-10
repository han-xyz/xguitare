use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::{mem, slice};

use rtrb::{Consumer, Producer, RingBuffer};
use xguitare_dsp::Rack;
use xguitare_dsp::crossfade::Crossfader;

use crate::arrangement::{Arrangement, ArrangementCommand};

#[derive(Debug, Default)]
pub struct EngineShared {
    pub xruns: AtomicU64,
    pub oversize_blocks: AtomicU64,
    pub callback_ns_max: AtomicU64,
    pub callback_ns_last: AtomicU64,
    pub sample_rate: AtomicU32,
    pub buffer_size: AtomicU32,
    /// `max_frames` of the Rack the audio thread is running.
    pub running_frames: AtomicUsize,
    /// f32 bits of the largest input |sample| since the last `take_peak`.
    pub input_peak_bits: AtomicU32,
    /// f32 bits of the largest |sample| over both outputs since the last `take_peak`.
    pub output_peak_bits: AtomicU32,
    /// While true the switcher ramps both outputs to silence; the tap still carries the input.
    pub muted: AtomicBool,
    /// Pushes into a Take ring that dropped frames because the disk thread fell behind.
    pub disk_overflows: AtomicU64,
}

pub enum Command {
    SwapRack(Box<Rack>),
    /// Applies to the next crossfade and to the Mute ramp from the next block; sent before the `SwapRack` of a Rack rebuilt at a new sample rate.
    SetFadeLengths(FadeLengths),
    SetParam {
        module: usize,
        param: usize,
        value: f32,
    },
    SetBypass {
        module: usize,
        on: bool,
    },
    /// Runs this Arrangement instead of the Rack: cut in when none runs, else crossfaded in over the crossfade length, carrying on the transport and open Takes of the Arrangement it replaces.
    SwapArrangement(Box<Arrangement>),
    /// Goes back to the Rack.
    CloseArrangement,
    /// Applies to every Arrangement the switcher holds; ignored while none runs.
    Arrangement(ArrangementCommand),
}

pub enum FromAudio {
    Retired(Box<Rack>),
    RetiredArrangement(Box<Arrangement>),
    ProgramChange(u8),
}

/// The control side of the queues between the control thread and the audio thread.
pub struct EngineQueues {
    pub commands: Producer<Command>,
    pub from_audio: Consumer<FromAudio>,
    program_changes: Vec<u8>,
}

/// The audio side: the audio thread pops `commands` and hands every Rack it lets go of back through `from_audio`, so no Rack is freed on the audio thread.
pub struct AudioQueues {
    pub commands: Consumer<Command>,
    pub from_audio: Producer<FromAudio>,
    /// The dry input, for the Tuner on the control thread.
    pub tap: Producer<f32>,
}

const PROGRAM_CHANGE_SLOTS: usize = 16;
const TAP_CAPACITY: usize = 16384;
const MUTE_RAMP_MS: f32 = 10.0;

/// PipeWire's default `clock.quantum-limit`: the largest block the switcher's own scratch is prepared for, so a swap to a Rack re-prepared for any quantum up to it never allocates on the audio thread.
#[allow(clippy::doc_markdown, reason = "PipeWire is a product name, not code")]
pub const SWITCHER_MAX_FRAMES: usize = 8192;

/// The crossfade and Mute ramp lengths at one sample rate, computed on the control side so the audio thread does no conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FadeLengths {
    crossfade_frames: usize,
    mute_step: f32,
}

impl FadeLengths {
    pub fn at(sample_rate: f32, crossfade_ms: f32) -> FadeLengths {
        let mute_ramp_frames = (MUTE_RAMP_MS * sample_rate / 1000.0).round().max(1.0);
        FadeLengths {
            crossfade_frames: (crossfade_ms * sample_rate / 1000.0) as usize,
            mute_step: 1.0 / mute_ramp_frames,
        }
    }
}

/// The largest |sample| of `samples`, 0 for an empty slice.
pub fn block_peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0, |peak: f32, sample| peak.max(sample.abs()))
}

pub fn raise_peak(peak: &AtomicU32, value: f32) {
    // Non-negative f32 bit patterns order like the floats, so an integer max on the bits is a float max.
    peak.fetch_max(value.to_bits(), Ordering::Relaxed);
}

/// Reads the peak stored in `peak` and resets it to 0.
pub fn take_peak(peak: &AtomicU32) -> f32 {
    f32::from_bits(peak.swap(0, Ordering::Relaxed))
}

/// The control side, the audio side, and the control thread's end of the input tap.
pub fn engine_queues(capacity: usize) -> (EngineQueues, AudioQueues, Consumer<f32>) {
    let (commands_tx, commands_rx) = RingBuffer::new(capacity);
    // At most every queued SwapRack, SwapArrangement or CloseArrangement plus the current, the fading-in and the pending Rack (minus the one left current) and the current, fading-in and pending Arrangement can be retired between two drains, and the control side drains before each such push; program changes may only use the slots beyond that, so the audio thread never finds the queue full when it retires a Rack.
    let (from_audio_tx, from_audio_rx) = RingBuffer::new(capacity + 5 + PROGRAM_CHANGE_SLOTS);
    let (tap_tx, tap_rx) = RingBuffer::new(TAP_CAPACITY);
    (
        EngineQueues {
            commands: commands_tx,
            from_audio: from_audio_rx,
            program_changes: Vec::new(),
        },
        AudioQueues {
            commands: commands_rx,
            from_audio: from_audio_tx,
            tap: tap_tx,
        },
        tap_rx,
    )
}

/// Pushes `input` into the tap; when the ring is full the rest is dropped, since a slow control thread is never a reason to block.
pub fn tap_block(tap: &mut Producer<f32>, input: &[f32]) {
    for &sample in input {
        if tap.push(sample).is_err() {
            return;
        }
    }
}

impl EngineQueues {
    /// Empties `from_audio`: drops every retired Rack and Arrangement and keeps program changes for `take_program_changes`.
    pub fn drain_from_audio(&mut self) {
        while let Ok(message) = self.from_audio.pop() {
            match message {
                FromAudio::Retired(rack) => drop(rack),
                FromAudio::RetiredArrangement(arrangement) => drop(arrangement),
                FromAudio::ProgramChange(program) => self.program_changes.push(program),
            }
        }
    }

    pub fn take_program_changes(&mut self) -> Vec<u8> {
        self.drain_from_audio();
        mem::take(&mut self.program_changes)
    }
}

/// The audio thread's Rack state: the current Rack, the one fading in, the one waiting for that fade to end and a block it fits, and the Commands that change them. Allocates and frees nothing after `new`; every Rack it lets go of leaves through `AudioQueues::from_audio`.
pub struct RackSwitcher {
    current: Box<Rack>,
    incoming: Option<Box<Rack>>,
    pending: Option<Box<Rack>>,
    arrangement: Option<Box<Arrangement>>,
    incoming_arrangement: Option<Box<Arrangement>>,
    pending_arrangement: Option<Box<Arrangement>>,
    crossfader: Crossfader,
    arrangement_fader: Crossfader,
    max_frames: usize,
    fades: FadeLengths,
    queues: AudioQueues,
    reserved_slots: usize,
    shared: Arc<EngineShared>,
    mute_gain: f32,
}

impl RackSwitcher {
    pub fn new(
        rack: Box<Rack>,
        max_frames: usize,
        fades: FadeLengths,
        queues: AudioQueues,
        shared: Arc<EngineShared>,
    ) -> RackSwitcher {
        let mut crossfader = Crossfader::new();
        crossfader.prepare(max_frames);
        let mut arrangement_fader = Crossfader::new();
        arrangement_fader.prepare(max_frames);
        let reserved_slots = queues.from_audio.buffer().capacity() - PROGRAM_CHANGE_SLOTS;
        shared
            .running_frames
            .store(rack.max_frames(), Ordering::Relaxed);
        RackSwitcher {
            current: rack,
            incoming: None,
            pending: None,
            arrangement: None,
            incoming_arrangement: None,
            pending_arrangement: None,
            crossfader,
            arrangement_fader,
            max_frames,
            fades,
            queues,
            reserved_slots,
            shared,
            mute_gain: 1.0,
        }
    }

    pub fn apply_commands(&mut self) {
        while let Ok(command) = self.queues.commands.pop() {
            match command {
                Command::SetParam {
                    module,
                    param,
                    value,
                } => {
                    let newest = self
                        .pending
                        .as_mut()
                        .or(self.incoming.as_mut())
                        .unwrap_or(&mut self.current);
                    newest.module_mut(module).set_param(param, value);
                }
                Command::SetBypass { module, on } => {
                    let newest = self
                        .pending
                        .as_mut()
                        .or(self.incoming.as_mut())
                        .unwrap_or(&mut self.current);
                    newest.set_bypass(module, on);
                }
                Command::SwapRack(new) => {
                    if self.incoming.is_none() && self.pending.is_none() {
                        self.start_fade(new);
                    } else if let Some(replaced) = self.pending.replace(new) {
                        self.retire(replaced);
                    }
                }
                Command::SetFadeLengths(fades) => self.fades = fades,
                Command::SwapArrangement(new) => {
                    if self.arrangement.is_none() {
                        self.arrangement = Some(new);
                    } else if self.incoming_arrangement.is_none() {
                        self.start_arrangement_fade(new);
                    } else if let Some(replaced) = self.pending_arrangement.replace(new) {
                        self.retire_arrangement(replaced);
                    }
                }
                Command::CloseArrangement => {
                    let held = [
                        self.arrangement.take(),
                        self.incoming_arrangement.take(),
                        self.pending_arrangement.take(),
                    ];
                    for old in held.into_iter().flatten() {
                        self.retire_arrangement(old);
                    }
                }
                Command::Arrangement(command) => {
                    for arrangement in [
                        &mut self.arrangement,
                        &mut self.incoming_arrangement,
                        &mut self.pending_arrangement,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        arrangement.apply(command);
                    }
                }
            }
        }
    }

    fn start_fade(&mut self, new: Box<Rack>) {
        self.incoming = Some(new);
        self.crossfader.start(self.fades.crossfade_frames);
    }

    /// Taps the input, runs one block through the current Rack, or through both while a fade runs, then applies the Mute gain; writes silence and returns false when the block is longer than the switcher's `max_frames` or than a live Rack was prepared for. While an Arrangement runs, `input` is its first Track's input.
    pub fn process(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32]) -> bool {
        self.process_tracks(input, slice::from_ref(&input), out_l, out_r)
    }

    /// [`RackSwitcher::process`], with `track_inputs` (one per Track) feeding the Arrangement while one runs; the Rack and the tap take `input`.
    pub fn process_tracks(
        &mut self,
        input: &[f32],
        track_inputs: &[&[f32]],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool {
        tap_block(&mut self.queues.tap, input);
        let fits = if self.arrangement.is_some() {
            self.run_arrangements(track_inputs, out_l, out_r)
        } else {
            self.run_racks(input, out_l, out_r)
        };
        self.apply_mute(out_l, out_r);
        fits
    }

    fn start_arrangement_fade(&mut self, mut new: Box<Arrangement>) {
        if let Some(current) = self.arrangement.as_mut() {
            new.continue_from(current);
        }
        self.incoming_arrangement = Some(new);
        self.arrangement_fader.start(self.fades.crossfade_frames);
    }

    fn finish_arrangement_fade(&mut self, new: Box<Arrangement>) {
        if let Some(old) = self.arrangement.replace(new) {
            self.retire_arrangement(old);
        }
        if let Some(next) = self.pending_arrangement.take() {
            self.start_arrangement_fade(next);
        }
    }

    /// Runs the current Arrangement, or crossfades to the incoming one; an Arrangement that cannot run this block ends the fade with a cut, and silence comes out while the one left cannot run it either.
    fn run_arrangements(
        &mut self,
        track_inputs: &[&[f32]],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool {
        let frames = out_l.len();
        let fits = |arrangement: &Option<Box<Arrangement>>| {
            arrangement
                .as_deref()
                .is_none_or(|arrangement| frames <= arrangement.max_frames())
        };
        while !(fits(&self.arrangement) && fits(&self.incoming_arrangement)) {
            let Some(incoming) = self.incoming_arrangement.take() else {
                break;
            };
            self.finish_arrangement_fade(incoming);
        }
        if frames > self.max_frames || !fits(&self.arrangement) || !fits(&self.incoming_arrangement)
        {
            out_l.fill(0.0);
            out_r.fill(0.0);
            return false;
        }
        let Some(current) = self.arrangement.as_deref_mut() else {
            return false;
        };
        let mut overflows = 0;
        let faded = if let Some(incoming) = self.incoming_arrangement.as_deref_mut() {
            let faded = self.arrangement_fader.process_with(
                out_l,
                out_r,
                |left, right| current.process(track_inputs, left, right),
                |left, right| incoming.process(track_inputs, left, right),
            );
            overflows += incoming.take_disk_overflows();
            faded
        } else {
            current.process(track_inputs, out_l, out_r);
            false
        };
        overflows += current.take_disk_overflows();
        if overflows > 0 {
            self.shared
                .disk_overflows
                .fetch_add(overflows, Ordering::Relaxed);
        }
        if faded && let Some(new) = self.incoming_arrangement.take() {
            self.finish_arrangement_fade(new);
        }
        true
    }

    fn run_racks(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32]) -> bool {
        let frames = input.len();
        let fits = |rack: &Rack| frames <= rack.max_frames();
        if self.incoming.is_none()
            && let Some(next) = self.pending.take_if(|next| fits(next))
        {
            self.start_fade(next);
        }
        if !fits(&self.current) || self.incoming.as_deref().is_some_and(|new| !fits(new)) {
            self.cut_to_fitting(frames);
        }
        if frames > self.max_frames
            || !fits(&self.current)
            || self.incoming.as_deref().is_some_and(|new| !fits(new))
        {
            out_l.fill(0.0);
            out_r.fill(0.0);
            return false;
        }
        let Some(incoming) = self.incoming.as_mut() else {
            self.current.process(input, out_l, out_r);
            return true;
        };
        let faded = self
            .crossfader
            .process(&mut self.current, incoming, input, out_l, out_r);
        if let Some(new) = self.incoming.take_if(|_| faded) {
            self.finish_fade(new, frames);
        }
        true
    }

    /// A Rack that cannot run this block makes the output silent and stops the fade, so cut without a fade: to the pending Rack if it fits, else to the fading-in one when the current Rack is the one that does not fit, keeping the pending Rack until a block fits it.
    fn cut_to_fitting(&mut self, frames: usize) {
        if let Some(newest) = self.pending.take_if(|new| frames <= new.max_frames()) {
            if let Some(fading_in) = self.incoming.take() {
                self.retire(fading_in);
            }
            let old = self.make_current(newest);
            self.retire(old);
            return;
        }
        if frames > self.current.max_frames()
            && let Some(new) = self.incoming.take_if(|new| frames <= new.max_frames())
        {
            self.finish_fade(new, frames);
        }
    }

    fn finish_fade(&mut self, new: Box<Rack>, frames: usize) {
        let old = self.make_current(new);
        self.retire(old);
        if let Some(next) = self.pending.take_if(|next| frames <= next.max_frames()) {
            self.start_fade(next);
        }
    }

    #[allow(
        clippy::float_cmp,
        reason = "the ramp clamps to the target, so it lands on 0.0 or 1.0 exactly"
    )]
    fn apply_mute(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let target = if self.shared.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            1.0
        };
        if self.mute_gain == target {
            if target == 0.0 {
                out_l.fill(0.0);
                out_r.fill(0.0);
            }
            return;
        }
        let step = self.fades.mute_step;
        for (left, right) in out_l.iter_mut().zip(out_r.iter_mut()) {
            self.mute_gain = if target > self.mute_gain {
                (self.mute_gain + step).min(target)
            } else {
                (self.mute_gain - step).max(target)
            };
            *left *= self.mute_gain;
            *right *= self.mute_gain;
        }
    }

    /// Hands a MIDI program change to the control thread, or drops it when only the slots reserved for retired Racks are free.
    pub fn push_program_change(&mut self, program: u8) {
        if self.queues.from_audio.slots() > self.reserved_slots {
            let pushed = self
                .queues
                .from_audio
                .push(FromAudio::ProgramChange(program));
            debug_assert!(pushed.is_ok(), "from_audio queue full");
        }
    }

    fn make_current(&mut self, rack: Box<Rack>) -> Box<Rack> {
        self.shared
            .running_frames
            .store(rack.max_frames(), Ordering::Relaxed);
        mem::replace(&mut self.current, rack)
    }

    fn retire_arrangement(&mut self, arrangement: Box<Arrangement>) {
        let pushed = self
            .queues
            .from_audio
            .push(FromAudio::RetiredArrangement(arrangement));
        debug_assert!(pushed.is_ok(), "from_audio queue full");
        if let Err(unretired) = pushed {
            mem::forget(unretired);
        }
    }

    fn retire(&mut self, rack: Box<Rack>) {
        let pushed = self.queues.from_audio.push(FromAudio::Retired(rack));
        debug_assert!(pushed.is_ok(), "from_audio queue full");
        if let Err(unretired) = pushed {
            // Leaking beats freeing on the audio thread; the queue sizing in engine_queues and the reserve in push_program_change make this unreachable.
            mem::forget(unretired);
        }
    }
}
