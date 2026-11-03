use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use xguitare_dsp::Rack;
use xguitare_dsp::crossfade::Crossfader;

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
}

pub enum Command {
    SwapRack(Box<Rack>),
    SetParam {
        module: usize,
        param: usize,
        value: f32,
    },
}

pub enum FromAudio {
    Retired(Box<Rack>),
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
}

const PROGRAM_CHANGE_SLOTS: usize = 16;

/// PipeWire's default `clock.quantum-limit`: the largest block the switcher's own scratch is prepared for, so a swap to a Rack re-prepared for any quantum up to it never allocates on the audio thread.
pub const SWITCHER_MAX_FRAMES: usize = 8192;

pub fn engine_queues(capacity: usize) -> (EngineQueues, AudioQueues) {
    let (commands_tx, commands_rx) = RingBuffer::new(capacity);
    // At most every queued SwapRack plus the current and the fading-in Rack, minus the one left current, can be retired between two drains, and `control::handle` drains before each SwapRack push; program changes may only use the slots beyond that, so the audio thread never finds the queue full when it retires a Rack.
    let (from_audio_tx, from_audio_rx) = RingBuffer::new(capacity + 2 + PROGRAM_CHANGE_SLOTS);
    (
        EngineQueues {
            commands: commands_tx,
            from_audio: from_audio_rx,
            program_changes: Vec::new(),
        },
        AudioQueues {
            commands: commands_rx,
            from_audio: from_audio_tx,
        },
    )
}

impl EngineQueues {
    /// Empties `from_audio`: drops every retired Rack and keeps program changes for `take_program_changes`.
    pub fn drain_from_audio(&mut self) {
        while let Ok(message) = self.from_audio.pop() {
            match message {
                FromAudio::Retired(rack) => drop(rack),
                FromAudio::ProgramChange(program) => self.program_changes.push(program),
            }
        }
    }

    pub fn take_program_changes(&mut self) -> Vec<u8> {
        self.drain_from_audio();
        mem::take(&mut self.program_changes)
    }
}

/// The audio thread's Rack state: the current Rack, the one fading in, and the Commands that change them. Allocates and frees nothing after `new`; every Rack it lets go of leaves through `AudioQueues::from_audio`.
pub struct RackSwitcher {
    current: Box<Rack>,
    incoming: Option<Box<Rack>>,
    crossfader: Crossfader,
    max_frames: usize,
    crossfade_frames: usize,
    queues: AudioQueues,
    reserved_slots: usize,
    shared: Arc<EngineShared>,
}

impl RackSwitcher {
    pub fn new(
        rack: Box<Rack>,
        max_frames: usize,
        crossfade_frames: usize,
        queues: AudioQueues,
        shared: Arc<EngineShared>,
    ) -> RackSwitcher {
        let mut crossfader = Crossfader::new();
        crossfader.prepare(max_frames);
        let reserved_slots = queues.from_audio.buffer().capacity() - PROGRAM_CHANGE_SLOTS;
        shared
            .running_frames
            .store(rack.max_frames(), Ordering::Relaxed);
        RackSwitcher {
            current: rack,
            incoming: None,
            crossfader,
            max_frames,
            crossfade_frames,
            queues,
            reserved_slots,
            shared,
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
                    let newest = self.incoming.as_mut().unwrap_or(&mut self.current);
                    newest.module_mut(module).set_param(param, value);
                }
                Command::SwapRack(new) => match self.incoming.take() {
                    // A swap during a fade cuts straight to the newest Rack rather than queueing fades, so at most two Racks are ever live.
                    Some(fading_in) => {
                        let old = self.make_current(new);
                        self.retire(old);
                        self.retire(fading_in);
                    }
                    None => {
                        self.incoming = Some(new);
                        self.crossfader.start(self.crossfade_frames);
                    }
                },
            }
        }
    }

    /// Runs one block through the current Rack, or through both while a fade runs; writes silence and returns false when the block is longer than the switcher's `max_frames` or than a live Rack was prepared for.
    pub fn process(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32]) -> bool {
        let frames = input.len();
        if frames > self.current.max_frames()
            && let Some(new) = self.incoming.take_if(|new| frames <= new.max_frames())
        {
            // The current Rack has been silent at this block size, so there is nothing to fade from.
            let old = self.make_current(new);
            self.retire(old);
        }
        let fits = |rack: &Rack| frames <= rack.max_frames();
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
            let old = self.make_current(new);
            self.retire(old);
        }
        true
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

    fn retire(&mut self, rack: Box<Rack>) {
        let pushed = self.queues.from_audio.push(FromAudio::Retired(rack));
        debug_assert!(pushed.is_ok(), "from_audio queue full");
        if let Err(unretired) = pushed {
            // Leaking beats freeing on the audio thread; the queue sizing in engine_queues and the reserve in push_program_change make this unreachable.
            mem::forget(unretired);
        }
    }
}
