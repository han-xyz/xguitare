use std::collections::HashMap;
use std::sync::Arc;

use xguitare_core::Preset;
use xguitare_core::arrangement::{Arrangement, ArrangementCommand};
use xguitare_core::build::build_stereo_rack;
use xguitare_core::engine::{
    Command, EngineShared, FadeLengths, MeterPoint, RackSwitcher, engine_queues,
};
use xguitare_core::project::{Bus, LoopRegion, Master, Project, Send, Track};
use xguitare_dsp::{Gain, Module, Rack, StereoRack};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;
const SETTLE_BLOCKS: usize = 20;
const TOLERANCE: f32 = 1e-4;
const OFF: f32 = f32::NEG_INFINITY;

fn armed(name: &str) -> Track {
    Track {
        name: name.to_string(),
        input: None,
        volume_db: 0.0,
        pan: 0.0,
        mute: false,
        solo: false,
        arm: true,
        to_master: true,
        sends: Vec::new(),
        modules: Vec::new(),
        items: Vec::new(),
    }
}

fn bus(id: u64) -> Bus {
    Bus {
        id,
        name: format!("Bus {id}"),
        volume_db: 0.0,
        pan: 0.0,
        mute: false,
        solo: false,
        modules: Vec::new(),
    }
}

fn send(bus: u64, level_db: f32, pre: bool) -> Send {
    Send { bus, level_db, pre }
}

fn project(tracks: Vec<Track>, buses: Vec<Bus>) -> Project {
    Project {
        name: "mix".to_string(),
        sample_rate: RATE as u32,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 0.0,
        click: false,
        count_in: 0,
        tracks,
        buses,
        master: Master::default(),
    }
}

fn stereo_rack(modules: &str) -> StereoRack {
    let preset = Preset::parse(&format!("name = \"m\"\n{modules}")).expect("valid Modules");
    build_stereo_rack(RATE, BLOCK, &preset.modules).expect("no cabinet")
}

fn arrangement(project: &Project) -> Arrangement {
    arrangement_with_master(project, stereo_rack(""))
}

fn arrangement_with_master(project: &Project, master: StereoRack) -> Arrangement {
    let racks = project
        .tracks
        .iter()
        .map(|_| {
            let mut rack = Rack::new(Vec::new()).expect("valid layout");
            rack.prepare(RATE, BLOCK);
            rack
        })
        .collect();
    let buses = project.buses.iter().map(|_| stereo_rack("")).collect();
    Arrangement::new(project, racks, buses, master, &HashMap::new(), RATE, BLOCK).expect("no Takes")
}

/// The last frame of the output after the smoothers settle, with each Track's input held at its constant.
fn settled(arrangement: &mut Arrangement, levels: &[f32]) -> (f32, f32) {
    let inputs: Vec<Vec<f32>> = levels.iter().map(|&level| vec![level; BLOCK]).collect();
    let inputs: Vec<&[f32]> = inputs.iter().map(Vec::as_slice).collect();
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    for _ in 0..SETTLE_BLOCKS {
        arrangement.process(&inputs, &mut left, &mut right);
    }
    (left[BLOCK - 1], right[BLOCK - 1])
}

fn db(level_db: f32) -> f32 {
    10f32.powf(level_db / 20.0)
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < TOLERANCE,
        "{actual} is not {expected}"
    );
}

fn bus_only(pre: bool) -> Project {
    let mut t = armed("t");
    t.to_master = false;
    t.sends = vec![send(1, 0.0, pre)];
    project(vec![t], vec![bus(1)])
}

#[test]
fn post_send_follows_track_volume_and_pre_send_does_not() {
    let mut post = arrangement(&bus_only(false));
    let mut pre = arrangement(&bus_only(true));
    for mix in [&mut post, &mut pre] {
        assert_close(settled(mix, &[0.5]).0, 0.5);
        mix.apply(ArrangementCommand::Volume {
            track: 0,
            db: -12.0,
        });
    }
    assert_close(settled(&mut post, &[0.5]).0, 0.5 * db(-12.0));
    assert_close(settled(&mut pre, &[0.5]).0, 0.5);
}

#[test]
fn pre_send_keeps_the_track_pan() {
    let mut p = bus_only(true);
    p.tracks[0].pan = -1.0;
    let (left, right) = settled(&mut arrangement(&p), &[0.5]);
    assert_close(left, 0.5 * std::f32::consts::SQRT_2);
    assert_close(right, 0.0);
}

#[test]
fn to_master_off_is_silent_directly_but_heard_through_its_bus() {
    let mut p = bus_only(false);
    p.tracks[0].sends[0].level_db = OFF;
    let mut mix = arrangement(&p);
    assert_eq!(settled(&mut mix, &[0.5]), (0.0, 0.0));
    mix.apply(ArrangementCommand::SendLevel {
        track: 0,
        bus: 0,
        db: -6.0,
    });
    assert_close(settled(&mut mix, &[0.5]).0, 0.5 * db(-6.0));
    mix.apply(ArrangementCommand::ToMaster { track: 0, on: true });
    assert_close(settled(&mut mix, &[0.5]).0, 0.5 + 0.5 * db(-6.0));
}

#[test]
fn soloed_track_brings_in_the_bus_it_sends_to() {
    let mut soloed = armed("soloed");
    soloed.to_master = false;
    soloed.sends = vec![send(1, 0.0, false)];
    soloed.solo = true;
    let direct = armed("direct");
    let mut other = armed("other");
    other.sends = vec![send(1, 0.0, false)];
    let p = project(vec![soloed, direct, other], vec![bus(1)]);
    assert_close(settled(&mut arrangement(&p), &[0.5, 0.25, 0.125]).0, 0.5);
}

#[test]
fn soloed_bus_brings_in_the_tracks_that_send_to_it() {
    let mut sender = armed("sender");
    sender.sends = vec![send(1, 0.0, false)];
    let mut other = armed("other");
    other.sends = vec![send(2, 0.0, false)];
    let mut soloed = bus(1);
    soloed.solo = true;
    let p = project(vec![sender, other], vec![soloed, bus(2)]);
    assert_close(settled(&mut arrangement(&p), &[0.5, 0.25]).0, 1.0);
}

#[test]
fn nothing_soloed_lets_every_strip_sound() {
    let mut a = armed("a");
    a.sends = vec![send(1, 0.0, false)];
    let p = project(vec![a, armed("b")], vec![bus(1)]);
    let mut mix = arrangement(&p);
    assert_close(settled(&mut mix, &[0.5, 0.25]).0, 1.25);
    mix.apply(ArrangementCommand::BusSolo { bus: 0, on: true });
    assert_close(settled(&mut mix, &[0.5, 0.25]).0, 1.0);
    mix.apply(ArrangementCommand::BusSolo { bus: 0, on: false });
    assert_close(settled(&mut mix, &[0.5, 0.25]).0, 1.25);
}

#[test]
fn mute_wins_over_solo() {
    let mut muted = armed("muted");
    muted.solo = true;
    muted.mute = true;
    let mut sender = armed("sender");
    sender.sends = vec![send(1, 0.0, false)];
    let mut muted_bus = bus(1);
    muted_bus.solo = true;
    muted_bus.mute = true;
    let p = project(vec![muted, sender], vec![muted_bus]);
    assert_close(settled(&mut arrangement(&p), &[0.5, 0.25]).0, 0.25);
}

#[test]
fn bus_volume_pan_and_mute_shape_its_output() {
    let mut mix = arrangement(&bus_only(false));
    mix.apply(ArrangementCommand::BusVolume { bus: 0, db: -6.0 });
    mix.apply(ArrangementCommand::BusPan { bus: 0, pan: 1.0 });
    let (left, right) = settled(&mut mix, &[0.5]);
    assert_close(left, 0.0);
    assert_close(right, 0.5 * db(-6.0) * std::f32::consts::SQRT_2);
    mix.apply(ArrangementCommand::BusMute { bus: 0, on: true });
    let (left, right) = settled(&mut mix, &[0.5]);
    assert_close(left, 0.0);
    assert_close(right, 0.0);
}

#[test]
fn master_volume_scales_the_mix() {
    let mut mix = arrangement(&project(vec![armed("t")], Vec::new()));
    mix.apply(ArrangementCommand::MasterVolume { db: -6.0 });
    assert_close(settled(&mut mix, &[0.5]).0, 0.5 * db(-6.0));
    mix.apply(ArrangementCommand::MasterVolume { db: OFF });
    assert_close(settled(&mut mix, &[0.5]).0, 0.0);
}

#[test]
fn click_level_is_unaffected_by_master_volume() {
    let clicked = |tracks: Vec<Track>, master_db: f32| {
        let mut p = project(tracks, Vec::new());
        p.master.volume_db = master_db;
        let mut mix = arrangement(&p);
        mix.apply(ArrangementCommand::Click(true));
        mix.apply(ArrangementCommand::Play);
        let levels = vec![0.5; p.tracks.len()];
        let inputs: Vec<&[f32]> = levels.iter().map(|_| &[0.5; BLOCK][..]).collect();
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        mix.process(&inputs, &mut left, &mut right);
        left
    };
    let silent_master = clicked(vec![armed("t")], OFF);
    let click_alone = clicked(Vec::new(), 0.0);
    assert!(click_alone.iter().any(|&sample| sample.abs() > 0.1));
    assert_eq!(silent_master, click_alone);
}

#[test]
fn master_rack_runs_a_mono_module_on_both_sides() {
    let master = stereo_rack("[[module]]\ntype = \"gain\"\nparams = { level = -6.0 }\n");
    let mut mix = arrangement_with_master(&project(vec![armed("t")], Vec::new()), master);
    let (left, right) = settled(&mut mix, &[0.5]);
    assert_close(left, 0.5 * db(-6.0));
    assert_close(right, 0.5 * db(-6.0));
}

#[test]
fn bus_and_master_params_and_bypass_reach_their_racks() {
    let gain = "[[module]]\ntype = \"gain\"\n";
    let p = bus_only(false);
    let racks = vec![{
        let mut rack = Rack::new(Vec::new()).expect("valid layout");
        rack.prepare(RATE, BLOCK);
        rack
    }];
    let mut mix = Arrangement::new(
        &p,
        racks,
        vec![stereo_rack(gain)],
        stereo_rack(gain),
        &HashMap::new(),
        RATE,
        BLOCK,
    )
    .expect("no Takes");
    let bus_param = ArrangementCommand::BusParam {
        bus: 0,
        module: 0,
        param: 0,
        value: -6.0,
    };
    let master_param = ArrangementCommand::MasterParam {
        module: 0,
        param: 0,
        value: -6.0,
    };
    for command in [bus_param, master_param] {
        mix.apply(command);
    }
    assert_close(settled(&mut mix, &[0.5]).0, 0.5 * db(-12.0));
    mix.apply(ArrangementCommand::BusBypass {
        bus: 0,
        module: 0,
        on: true,
    });
    mix.apply(ArrangementCommand::MasterBypass {
        module: 0,
        on: true,
    });
    assert_close(settled(&mut mix, &[0.5]).0, 0.5);
}

#[test]
fn bus_send_and_master_indices_out_of_range_are_ignored() {
    let mut mix = arrangement(&bus_only(false));
    for command in [
        ArrangementCommand::BusVolume { bus: 5, db: -6.0 },
        ArrangementCommand::BusPan { bus: 5, pan: 1.0 },
        ArrangementCommand::BusMute { bus: 5, on: true },
        ArrangementCommand::BusSolo { bus: 5, on: true },
        ArrangementCommand::SendLevel {
            track: 0,
            bus: 5,
            db: 0.0,
        },
        ArrangementCommand::SendLevel {
            track: 5,
            bus: 0,
            db: OFF,
        },
        ArrangementCommand::SendPre {
            track: 5,
            bus: 5,
            on: true,
        },
        ArrangementCommand::ToMaster { track: 5, on: true },
        ArrangementCommand::BusParam {
            bus: 0,
            module: 3,
            param: 0,
            value: 1.0,
        },
        ArrangementCommand::BusBypass {
            bus: 5,
            module: 0,
            on: true,
        },
        ArrangementCommand::MasterParam {
            module: 3,
            param: 9,
            value: 1.0,
        },
        ArrangementCommand::MasterBypass {
            module: 3,
            on: true,
        },
        ArrangementCommand::SendLevel {
            track: 0,
            bus: 0,
            db: f32::NAN,
        },
        ArrangementCommand::MasterVolume { db: f32::NAN },
    ] {
        mix.apply(command);
    }
    assert_close(settled(&mut mix, &[0.5]).0, 0.5);
}

fn switcher(shared: &Arc<EngineShared>) -> (xguitare_core::engine::EngineQueues, RackSwitcher) {
    let (control, audio, _tap) = engine_queues(16);
    let mut rack = Rack::new(Vec::new()).expect("valid layout");
    rack.prepare(RATE, BLOCK);
    let fades = FadeLengths::at(RATE, 20.0);
    let switcher = RackSwitcher::new(Box::new(rack), BLOCK, fades, audio, Arc::clone(shared));
    (control, switcher)
}

#[test]
fn clip_latches_until_reset() {
    let shared = Arc::new(EngineShared::default());
    let (mut control, mut switcher) = switcher(&shared);
    let mut loud = armed("loud");
    loud.sends = vec![send(1, 0.0, false)];
    let p = project(vec![loud, armed("quiet")], vec![bus(1)]);
    let swap = Command::SwapArrangement(Box::new(arrangement(&p)));
    assert!(control.commands.push(swap).is_ok());
    switcher.apply_commands();
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let (hot, soft) = ([1.5; BLOCK], [0.25; BLOCK]);
    switcher.process_tracks(&hot, &[&hot, &soft], &mut left, &mut right);
    let meters = &shared.mix_meters;
    for point in [MeterPoint::Track(0), MeterPoint::Bus(0), MeterPoint::Master] {
        assert!(meters.clip(point), "{point:?} did not clip");
    }
    assert!(!meters.clip(MeterPoint::Track(1)));
    let [peak_l, peak_r] = meters.take_peak(MeterPoint::Track(1));
    assert_close(peak_l, 0.25);
    assert_close(peak_r, 0.25);
    assert_eq!(meters.take_peak(MeterPoint::Track(1)), [0.0, 0.0]);

    let silence = [0.0; BLOCK];
    switcher.process_tracks(&silence, &[&silence, &silence], &mut left, &mut right);
    assert!(meters.clip(MeterPoint::Master), "the clip did not latch");
    meters.clip_reset(Some(MeterPoint::Master));
    assert!(!meters.clip(MeterPoint::Master));
    assert!(meters.clip(MeterPoint::Track(0)) && meters.clip(MeterPoint::Bus(0)));
    meters.clip_reset(None);
    assert!(!meters.clip(MeterPoint::Track(0)) && !meters.clip(MeterPoint::Bus(0)));
}

#[test]
fn a_bus_param_during_a_crossfade_lands_on_the_new_arrangement_only() {
    let shared = Arc::new(EngineShared::default());
    let (mut control, mut switcher) = switcher(&shared);
    let p = bus_only(false);
    let with_bus_rack = |modules: &str| {
        let mut rack = Rack::new(Vec::new()).expect("valid layout");
        rack.prepare(RATE, BLOCK);
        Arrangement::new(
            &p,
            vec![rack],
            vec![stereo_rack(modules)],
            stereo_rack(""),
            &HashMap::new(),
            RATE,
            BLOCK,
        )
        .expect("no Takes")
    };
    let one_gain = "[[module]]\ntype = \"gain\"\nid = \"a\"\n";
    let two_gains = format!("{one_gain}[[module]]\ntype = \"gain\"\nid = \"b\"\n");
    let swap = Command::SwapArrangement(Box::new(with_bus_rack(one_gain)));
    assert!(control.commands.push(swap).is_ok());
    switcher.apply_commands();
    let input = [0.5; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    switcher.process_tracks(&input, &[&input], &mut left, &mut right);

    let swap = Command::SwapArrangement(Box::new(with_bus_rack(&two_gains)));
    let set = Command::Arrangement(ArrangementCommand::BusParam {
        bus: 0,
        module: 0,
        param: 0,
        value: -20.0,
    });
    let bypass = Command::Arrangement(ArrangementCommand::BusBypass {
        bus: 0,
        module: 1,
        on: true,
    });
    for command in [swap, set, bypass] {
        assert!(control.commands.push(command).is_ok());
    }
    switcher.apply_commands();
    for _ in 0..2 {
        switcher.process_tracks(&input, &[&input], &mut left, &mut right);
    }
    assert!(left[0] > 0.4, "the outgoing Bus Rack changed: {}", left[0]);
    for _ in 0..SETTLE_BLOCKS {
        switcher.process_tracks(&input, &[&input], &mut left, &mut right);
    }
    assert_close(left[BLOCK - 1], 0.5 * db(-20.0));
}

#[test]
#[should_panic(expected = "a Bus or Master Rack prepared for fewer than 256 frames")]
fn a_bus_rack_prepared_for_fewer_frames_is_refused() {
    let p = bus_only(false);
    let mut rack = Rack::new(Vec::new()).expect("valid layout");
    rack.prepare(RATE, BLOCK);
    let small = build_stereo_rack(RATE, BLOCK / 2, &[]).expect("no cabinet");
    let _ = Arrangement::new(
        &p,
        vec![rack],
        vec![small],
        stereo_rack(""),
        &HashMap::new(),
        RATE,
        BLOCK,
    );
}

#[test]
#[should_panic(expected = "a Bus or Master Rack prepared for fewer than 256 frames")]
fn a_master_rack_prepared_for_fewer_frames_is_refused() {
    let p = project(vec![armed("t")], Vec::new());
    let mut rack = Rack::new(Vec::new()).expect("valid layout");
    rack.prepare(RATE, BLOCK);
    let unprepared = StereoRack::new(Vec::new()).expect("no Modules");
    let _ = Arrangement::new(
        &p,
        vec![rack],
        Vec::new(),
        unprepared,
        &HashMap::new(),
        RATE,
        BLOCK,
    );
}

#[test]
fn pre_send_is_taken_after_the_track_rack() {
    let p = bus_only(true);
    let mut gain: Box<dyn Module> = Box::new(Gain::new());
    gain.set_param(0, -6.0);
    let mut rack = Rack::new(vec![("gain".to_string(), gain)]).expect("valid layout");
    rack.prepare(RATE, BLOCK);
    rack.module_mut(0).reset();
    let mut mix = Arrangement::new(
        &p,
        vec![rack],
        vec![stereo_rack("")],
        stereo_rack(""),
        &HashMap::new(),
        RATE,
        BLOCK,
    )
    .expect("no Takes");
    assert_close(settled(&mut mix, &[0.5]).0, 0.5 * db(-6.0));
}

#[test]
fn closing_the_arrangement_clears_the_meters() {
    let shared = Arc::new(EngineShared::default());
    let (mut control, mut switcher) = switcher(&shared);
    let swap = Command::SwapArrangement(Box::new(arrangement(&project(
        vec![armed("t")],
        Vec::new(),
    ))));
    assert!(control.commands.push(swap).is_ok());
    switcher.apply_commands();
    let hot = [1.5; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    switcher.process_tracks(&hot, &[&hot], &mut left, &mut right);
    assert!(shared.mix_meters.clip(MeterPoint::Master));
    assert!(control.commands.push(Command::CloseArrangement).is_ok());
    switcher.apply_commands();
    assert!(
        !shared.mix_meters.clip(MeterPoint::Master)
            && !shared.mix_meters.clip(MeterPoint::Track(0))
    );
    assert_eq!(
        shared.mix_meters.take_peak(MeterPoint::Track(0)),
        [0.0, 0.0]
    );
}
