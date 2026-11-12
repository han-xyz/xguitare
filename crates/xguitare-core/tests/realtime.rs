use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use std::collections::HashMap;

use assert_no_alloc::*;
use xguitare_core::arrangement::{Arrangement, ArrangementCommand};
use xguitare_core::build::build_stereo_rack;
use xguitare_core::engine::{
    Command, EngineShared, FadeLengths, FromAudio, MeterPoint, RackSwitcher, SWITCHER_MAX_FRAMES,
    block_peak, engine_queues, raise_peak, tap_block,
};
use xguitare_core::metronome::MetronomeSettings;
use xguitare_core::project::{Bus, Item, LoopRegion, Master, Project, Send, Take, Track};
use xguitare_core::{ModuleRegistry, PresetModule};
use xguitare_dsp::{Gain, Module, Rack, StereoRack};

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
                to_master: true,
                sends: Vec::new(),
                modules: Vec::new(),
                items: vec![
                    Item {
                        id: 3 * idx as u64,
                        position: 50 * idx as u64,
                        offset: 0,
                        length: None,
                        take: 0,
                        takes: vec![Take {
                            file: file.clone(),
                            frames: 1000,
                        }],
                    },
                    Item {
                        id: 3 * idx as u64 + 1,
                        position: 50 * idx as u64 + 300,
                        offset: 20,
                        length: None,
                        take: 0,
                        takes: vec![Take {
                            file: file.clone(),
                            frames: 1000,
                        }],
                    },
                    Item {
                        id: 3 * idx as u64 + 2,
                        position: 50 * idx as u64 + 400,
                        offset: 0,
                        length: Some(200),
                        take: 0,
                        takes: vec![Take { file, frames: 1000 }],
                    },
                ],
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
        click: false,
        count_in: 0,
        next_item_id: 48,
        tracks,
        buses: Vec::new(),
        master: Master::default(),
    };
    (project, takes)
}

fn sixteen_track_arrangement() -> Box<Arrangement> {
    let (project, takes) = playing_project(16);
    let racks = (0..16).map(|_| *gain_rack()).collect();
    Box::new(
        Arrangement::new(
            &project,
            racks,
            Vec::new(),
            empty_master(),
            &takes,
            48_000.0,
            BLOCK,
        )
        .expect("takes loaded"),
    )
}

fn every_arrangement_command() -> [ArrangementCommand; 17] {
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
        ArrangementCommand::ClickVolume(-6.0),
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
fn a_set_on_a_module_added_during_a_crossfade_does_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, fades(), audio, Arc::default());
    let (project, takes) = playing_project(16);
    let two_gains = (0..16)
        .map(|_| {
            let gains = (0..2)
                .map(|idx| {
                    (
                        format!("gain{idx}"),
                        Box::new(Gain::new()) as Box<dyn Module>,
                    )
                })
                .collect();
            let mut rack = Rack::new(gains).expect("valid layout");
            rack.prepare(48_000.0, BLOCK);
            rack
        })
        .collect();
    let added = Box::new(
        Arrangement::new(
            &project,
            two_gains,
            Vec::new(),
            empty_master(),
            &takes,
            48_000.0,
            BLOCK,
        )
        .expect("loaded"),
    );
    let arrangements = [sixteen_track_arrangement(), added];
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
        }
        for on in [true, false] {
            let set = ArrangementCommand::Param {
                track: 3,
                module: 1,
                param: 0,
                value: -6.0,
            };
            let bypass = ArrangementCommand::Bypass {
                track: 3,
                module: 1,
                on,
            };
            for command in [set, bypass] {
                assert!(control.commands.push(Command::Arrangement(command)).is_ok());
            }
        }
        switcher.apply_commands();
        for _ in 0..20 {
            switcher.process_tracks(&input, &inputs, &mut left, &mut right);
        }
    });
    drop(switcher);
}

#[test]
fn take_capture_and_loop_wraps_do_not_allocate() {
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

fn empty_master() -> StereoRack {
    let mut master = StereoRack::new(Vec::new()).expect("no Modules");
    master.prepare(48_000.0, BLOCK);
    master
}

const BUS_MODULES: [&str; 4] = ["compressor", "gate", "chorus", "reverb"];
const MASTER_MODULES: [&str; 2] = ["compressor", "gain"];

fn modules(types: &[&str]) -> Vec<PresetModule> {
    let registry = ModuleRegistry::default();
    types
        .iter()
        .map(|&type_name| PresetModule {
            id: type_name.to_string(),
            type_name: type_name.to_string(),
            ir: None,
            bypass: false,
            params: registry
                .spec(type_name)
                .expect("registered")
                .params
                .iter()
                .map(|param| (param.name.to_string(), param.default))
                .collect(),
        })
        .collect()
}

fn full_mixer_arrangement() -> Box<Arrangement> {
    let (mut project, takes) = playing_project(16);
    project.buses = (0..8)
        .map(|id| Bus {
            id,
            name: format!("Bus {id}"),
            volume_db: -3.0,
            pan: 0.25,
            mute: false,
            solo: id == 2,
            modules: modules(&BUS_MODULES),
        })
        .collect();
    project.master.modules = modules(&MASTER_MODULES);
    for (idx, track) in project.tracks.iter_mut().enumerate() {
        track.to_master = idx % 3 != 0;
        track.sends = (0..8)
            .filter(|bus| (idx as u64 + bus).is_multiple_of(2))
            .map(|bus| Send {
                bus,
                level_db: -6.0,
                pre: bus % 4 == 0,
            })
            .collect();
    }
    let racks = (0..16).map(|_| *gain_rack()).collect();
    let bus_racks = project
        .buses
        .iter()
        .map(|bus| build_stereo_rack(48_000.0, BLOCK, &bus.modules).expect("no cabinet"))
        .collect();
    let master = build_stereo_rack(48_000.0, BLOCK, &project.master.modules).expect("no cabinet");
    Box::new(
        Arrangement::new(&project, racks, bus_racks, master, &takes, 48_000.0, BLOCK)
            .expect("takes loaded"),
    )
}

fn every_mixer_command() -> [ArrangementCommand; 14] {
    [
        ArrangementCommand::ToMaster { track: 0, on: true },
        ArrangementCommand::SendLevel {
            track: 1,
            bus: 1,
            db: 0.0,
        },
        ArrangementCommand::SendLevel {
            track: 0,
            bus: 0,
            db: f32::NEG_INFINITY,
        },
        ArrangementCommand::SendPre {
            track: 2,
            bus: 0,
            on: true,
        },
        ArrangementCommand::BusVolume { bus: 3, db: -9.0 },
        ArrangementCommand::BusPan { bus: 3, pan: -0.5 },
        ArrangementCommand::BusMute { bus: 4, on: true },
        ArrangementCommand::BusSolo { bus: 2, on: false },
        ArrangementCommand::BusParam {
            bus: 5,
            module: 0,
            param: 0,
            value: -30.0,
        },
        ArrangementCommand::BusBypass {
            bus: 5,
            module: 3,
            on: true,
        },
        ArrangementCommand::MasterVolume { db: -3.0 },
        ArrangementCommand::MasterParam {
            module: 1,
            param: 0,
            value: -6.0,
        },
        ArrangementCommand::MasterBypass {
            module: 0,
            on: true,
        },
        ArrangementCommand::BusVolume { bus: 99, db: 0.0 },
    ]
}

#[test]
fn sixteen_tracks_eight_buses_and_master_do_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, fades(), audio, Arc::clone(&shared));
    let arrangements = [full_mixer_arrangement(), full_mixer_arrangement()];
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
            let play = Command::Arrangement(ArrangementCommand::Play);
            assert!(control.commands.push(play).is_ok());
            switcher.apply_commands();
            switcher.process_tracks(&input, &inputs, &mut left, &mut right);
        }
        for command in every_mixer_command() {
            assert!(control.commands.push(Command::Arrangement(command)).is_ok());
            switcher.apply_commands();
            for _ in 0..10 {
                switcher.process_tracks(&input, &inputs, &mut left, &mut right);
            }
        }
        for _ in 0..100 {
            switcher.process_tracks(&input, &inputs, &mut left, &mut right);
        }
        let _ = shared.mix_meters.take_peak(MeterPoint::Master);
        shared.mix_meters.clip_reset(None);
    });
    assert!(left.iter().any(|&sample| sample != 0.0));
    drop(switcher);
}

#[test]
fn metronome_with_no_project_does_not_allocate() {
    let (mut control, audio, _tap) = engine_queues(64);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(gain_rack(), BLOCK, fades(), audio, Arc::clone(&shared));
    let on = |bpm, meter, volume_db| MetronomeSettings {
        on: true,
        bpm,
        meter,
        volume_db,
        sample_rate: 48_000.0,
    };
    let changes = [
        on(120.0, (4, 4), -12.0),
        on(97.5, (3, 4), 0.0),
        on(300.0, (6, 8), -40.0),
        MetronomeSettings {
            on: false,
            ..on(300.0, (6, 8), -40.0)
        },
        on(60.0, (4, 4), -12.0),
    ];
    let input = [0.25; BLOCK];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    assert_no_alloc(|| {
        for settings in changes {
            assert!(control.commands.push(Command::Metronome(settings)).is_ok());
            switcher.apply_commands();
            for block in 0..100 {
                shared.muted.store(block % 50 > 40, Ordering::Relaxed);
                switcher.process(&input, &mut left, &mut right);
            }
        }
    });
    assert_ne!(shared.metronome_beat.load(Ordering::Relaxed), 0);
}
