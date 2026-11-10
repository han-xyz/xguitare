use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use std::collections::HashMap;

use assert_no_alloc::*;
use xguitare_core::arrangement::{Arrangement, ArrangementCommand};
use xguitare_core::engine::{
    Command, EngineShared, FadeLengths, FromAudio, RackSwitcher, SWITCHER_MAX_FRAMES, block_peak,
    engine_queues, raise_peak, tap_block,
};
use xguitare_core::project::{Item, LoopRegion, Project, Take, Track};
use xguitare_dsp::{Gain, Module, Rack};

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const BLOCK: usize = 128;

fn fades() -> FadeLengths {
    FadeLengths::at(48_000.0, 20.0)
}

fn gain_rack() -> Box<Rack> {
    sized_gain_rack(BLOCK)
}

fn sized_gain_rack(max_frames: usize) -> Box<Rack> {
    let mut rack = Rack::new(vec![(
        "gain".to_string(),
        Box::new(Gain::new()) as Box<dyn Module>,
    )])
    .expect("valid layout");
    rack.prepare(48_000.0, max_frames);
    Box::new(rack)
}

#[test]
fn rack_switcher_does_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, fades(), audio, Arc::default());
    for _ in 0..2 {
        assert!(
            control
                .commands
                .push(Command::SwapRack(gain_rack()))
                .is_ok()
        );
    }
    let set = Command::SetParam {
        module: 0,
        param: 0,
        value: -6.0,
    };
    assert!(control.commands.push(set).is_ok());
    for on in [true, false] {
        let bypass = Command::SetBypass { module: 0, on };
        assert!(control.commands.push(bypass).is_ok());
    }
    assert!(
        control
            .commands
            .push(Command::SwapRack(gain_rack()))
            .is_ok()
    );
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for _ in 0..20 {
            switcher.push_program_change(3);
            switcher.apply_commands();
            switcher.process(&input, &mut left, &mut right);
        }
    });
    drop(switcher);
    control.drain_from_audio();
}

#[test]
fn rack_switcher_at_quantum_limit_does_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let mut switcher = RackSwitcher::new(
        sized_gain_rack(SWITCHER_MAX_FRAMES),
        SWITCHER_MAX_FRAMES,
        fades(),
        audio,
        Arc::default(),
    );
    assert!(
        control
            .commands
            .push(Command::SwapRack(sized_gain_rack(SWITCHER_MAX_FRAMES)))
            .is_ok()
    );
    let input = vec![0.25; SWITCHER_MAX_FRAMES];
    let mut left = vec![0.0; SWITCHER_MAX_FRAMES];
    let mut right = vec![0.0; SWITCHER_MAX_FRAMES];
    assert_no_alloc(|| {
        for _ in 0..20 {
            switcher.apply_commands();
            assert!(switcher.process(&input, &mut left, &mut right));
        }
    });
    drop(switcher);
    control.drain_from_audio();
}

#[test]
fn oversize_then_swap_and_cut_does_not_allocate() {
    const LARGER: usize = 2 * BLOCK;
    let (mut control, audio, _tap) = engine_queues(64);
    let mut switcher = RackSwitcher::new(
        sized_gain_rack(BLOCK),
        SWITCHER_MAX_FRAMES,
        fades(),
        audio,
        Arc::default(),
    );
    let larger = sized_gain_rack(LARGER);
    let input = [0.25; LARGER];
    let mut left = [0.0; LARGER];
    let mut right = [0.0; LARGER];
    let processed = assert_no_alloc(|| {
        switcher.apply_commands();
        let silent = !switcher.process(&input, &mut left, &mut right);
        assert!(control.commands.push(Command::SwapRack(larger)).is_ok());
        switcher.apply_commands();
        silent && switcher.process(&input, &mut left, &mut right)
    });
    assert!(processed);
    assert!(matches!(
        control.from_audio.pop(),
        Ok(FromAudio::Retired(_))
    ));
    drop(switcher);
    control.drain_from_audio();
}

#[test]
fn pending_kept_through_grown_block_then_promoted_does_not_allocate() {
    const LARGER: usize = 2 * BLOCK;
    let (mut control, audio, _tap) = engine_queues(64);
    let mut switcher = RackSwitcher::new(
        sized_gain_rack(BLOCK),
        SWITCHER_MAX_FRAMES,
        fades(),
        audio,
        Arc::default(),
    );
    let larger = sized_gain_rack(LARGER);
    let pending = sized_gain_rack(BLOCK);
    let input = [0.25; LARGER];
    let mut left = [0.0; LARGER];
    let mut right = [0.0; LARGER];
    let processed = assert_no_alloc(|| {
        assert!(control.commands.push(Command::SwapRack(larger)).is_ok());
        switcher.apply_commands();
        let mut processed =
            switcher.process(&input[..BLOCK], &mut left[..BLOCK], &mut right[..BLOCK]);
        assert!(control.commands.push(Command::SwapRack(pending)).is_ok());
        switcher.apply_commands();
        processed &= switcher.process(&input, &mut left, &mut right);
        for _ in 0..BLOCK {
            switcher.apply_commands();
            processed &= switcher.process(&input[..BLOCK], &mut left[..BLOCK], &mut right[..BLOCK]);
        }
        processed
    });
    assert!(processed);
    drop(switcher);
    control.drain_from_audio();
}

#[test]
fn peak_update_does_not_allocate() {
    let peak = AtomicU32::new(0);
    let input = [0.25; BLOCK];
    assert_no_alloc(|| {
        for _ in 0..20 {
            raise_peak(&peak, block_peak(&input));
        }
    });
}

#[test]
fn tap_block_does_not_allocate() {
    let (_control, mut audio, mut tap) = engine_queues(4);
    let input = [0.25; BLOCK];
    assert_no_alloc(|| {
        for _ in 0..200 {
            tap_block(&mut audio.tap, &input);
        }
    });
    assert!(tap.pop().is_ok());
}

#[test]
fn mute_process_does_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, fades(), audio, Arc::clone(&shared));
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for round in 0..40 {
            shared.muted.store(round % 6 < 3, Ordering::Relaxed);
            switcher.process(&input, &mut left, &mut right);
        }
    });
    drop(switcher);
    control.drain_from_audio();
}

#[test]
fn pending_swap_and_new_fade_lengths_do_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, fades(), audio, Arc::default());
    let racks = [gain_rack(), gain_rack(), gain_rack()];
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    let doubled = FadeLengths::at(96_000.0, 20.0);
    assert_no_alloc(|| {
        assert!(
            control
                .commands
                .push(Command::SetFadeLengths(doubled))
                .is_ok()
        );
        for rack in racks {
            assert!(control.commands.push(Command::SwapRack(rack)).is_ok());
            switcher.apply_commands();
            switcher.process(&input, &mut left, &mut right);
        }
        for _ in 0..40 {
            switcher.process(&input, &mut left, &mut right);
        }
    });
    let mut retired = 0;
    while let Ok(FromAudio::Retired(_)) = control.from_audio.pop() {
        retired += 1;
    }
    assert_eq!(retired, 3);
    drop(switcher);
}

fn playing_project(tracks: usize) -> (Project, HashMap<String, Arc<[f32]>>) {
    let mut takes = HashMap::new();
    let tracks = (0..tracks)
        .map(|idx| {
            let file = format!("takes/t{idx}-1.wav");
            takes.insert(file.clone(), Arc::from(vec![0.1; 1000]));
            Track {
                name: format!("t{idx}"),
                input: None,
                volume_db: -3.0,
                pan: 0.25,
                mute: false,
                solo: idx == 1,
                arm: idx % 4 == 0,
                modules: Vec::new(),
                items: vec![Item {
                    position: 50 * idx as u64,
                    take: 0,
                    takes: vec![Take { file, frames: 1000 }],
                }],
            }
        })
        .collect();
    let project = Project {
        name: "rt".to_string(),
        sample_rate: 48_000,
        tempo_bpm: 240.0,
        meter: (3, 4),
        loop_region: LoopRegion {
            start: 100,
            end: 900,
            on: true,
        },
        latency_offset_ms: 0.0,
        tracks,
    };
    (project, takes)
}

fn sixteen_track_arrangement() -> Box<Arrangement> {
    let (project, takes) = playing_project(16);
    let racks = (0..16).map(|_| *gain_rack()).collect();
    Box::new(Arrangement::new(&project, racks, &takes, 48_000.0, BLOCK).expect("takes loaded"))
}

fn every_arrangement_command() -> [ArrangementCommand; 16] {
    [
        ArrangementCommand::Volume { track: 3, db: -9.0 },
        ArrangementCommand::Pan {
            track: 3,
            pan: -0.5,
        },
        ArrangementCommand::Mute { track: 2, on: true },
        ArrangementCommand::Solo { track: 5, on: true },
        ArrangementCommand::Arm { track: 6, on: true },
        ArrangementCommand::Param {
            track: 7,
            module: 0,
            param: 0,
            value: -6.0,
        },
        ArrangementCommand::Bypass {
            track: 7,
            module: 0,
            on: true,
        },
        ArrangementCommand::Tempo {
            bpm: 90.0,
            meter: (4, 4),
        },
        ArrangementCommand::Click(true),
        ArrangementCommand::CountIn(1),
        ArrangementCommand::Loop(LoopRegion {
            start: 0,
            end: 300,
            on: true,
        }),
        ArrangementCommand::Locate(250),
        ArrangementCommand::Play,
        ArrangementCommand::Stop,
        ArrangementCommand::Record,
        ArrangementCommand::Volume { track: 99, db: 0.0 },
    ]
}

#[test]
fn arrangement_with_sixteen_tracks_playing_does_not_allocate() {
    let mut arrangement = sixteen_track_arrangement();
    arrangement.apply(ArrangementCommand::Click(true));
    arrangement.apply(ArrangementCommand::Play);
    let input = [0.25; BLOCK];
    let inputs = [&input[..]; 16];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for _ in 0..200 {
            arrangement.process(&inputs, &mut left, &mut right);
        }
    });
}

#[test]
fn every_arrangement_command_does_not_allocate() {
    let mut arrangement = sixteen_track_arrangement();
    let commands = every_arrangement_command();
    let input = [0.25; BLOCK];
    let inputs = [&input[..]; 16];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for command in commands {
            arrangement.apply(command);
            for _ in 0..20 {
                arrangement.process(&inputs, &mut left, &mut right);
            }
        }
    });
}

#[test]
fn switcher_arrangement_swap_commands_and_close_do_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, fades(), audio, Arc::default());
    let arrangements = [sixteen_track_arrangement(), sixteen_track_arrangement()];
    let commands = every_arrangement_command();
    let input = [0.25; BLOCK];
    let inputs = [&input[..]; 16];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for arrangement in arrangements {
            assert!(
                control
                    .commands
                    .push(Command::SwapArrangement(arrangement))
                    .is_ok()
            );
            switcher.apply_commands();
            switcher.process_tracks(&input, &inputs, &mut left, &mut right);
        }
        for command in commands {
            assert!(control.commands.push(Command::Arrangement(command)).is_ok());
            switcher.apply_commands();
            switcher.process_tracks(&input, &inputs, &mut left, &mut right);
        }
        assert!(control.commands.push(Command::CloseArrangement).is_ok());
        switcher.apply_commands();
        switcher.process(&input, &mut left, &mut right);
    });
    let mut retired = 0;
    while let Ok(FromAudio::RetiredArrangement(_)) = control.from_audio.pop() {
        retired += 1;
    }
    assert_eq!(retired, 2);
    drop(switcher);
}

#[test]
fn recording_passes_and_loop_wraps_do_not_allocate() {
    let mut arrangement = sixteen_track_arrangement();
    let input = [0.25; BLOCK];
    let inputs = [&input[..]; 16];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        arrangement.apply(ArrangementCommand::Record);
        for _ in 0..100 {
            arrangement.process(&inputs, &mut left, &mut right);
        }
        arrangement.apply(ArrangementCommand::Arm {
            track: 0,
            on: false,
        });
        arrangement.apply(ArrangementCommand::Locate(500));
        for _ in 0..20 {
            arrangement.process(&inputs, &mut left, &mut right);
        }
        arrangement.apply(ArrangementCommand::Stop);
        arrangement.process(&inputs, &mut left, &mut right);
        let _ = arrangement.take_disk_overflows();
    });
}

#[test]
fn arrangement_crossfades_while_recording_do_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, fades(), audio, shared);
    let arrangements = [
        sixteen_track_arrangement(),
        sixteen_track_arrangement(),
        sixteen_track_arrangement(),
        sixteen_track_arrangement(),
    ];
    let input = [0.25; BLOCK];
    let inputs = [&input[..]; 16];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for (idx, arrangement) in arrangements.into_iter().enumerate() {
            assert!(
                control
                    .commands
                    .push(Command::SwapArrangement(arrangement))
                    .is_ok()
            );
            if idx == 0 {
                let record = Command::Arrangement(ArrangementCommand::Record);
                assert!(control.commands.push(record).is_ok());
            }
            switcher.apply_commands();
            switcher.process_tracks(&input, &inputs, &mut left, &mut right);
        }
        for _ in 0..40 {
            switcher.process_tracks(&input, &inputs, &mut left, &mut right);
        }
        assert!(control.commands.push(Command::CloseArrangement).is_ok());
        switcher.apply_commands();
        switcher.process(&input, &mut left, &mut right);
    });
    let mut retired = 0;
    while let Ok(FromAudio::RetiredArrangement(_)) = control.from_audio.pop() {
        retired += 1;
    }
    assert_eq!(retired, 4);
    drop(switcher);
}
