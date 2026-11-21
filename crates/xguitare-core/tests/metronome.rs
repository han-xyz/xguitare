use std::sync::Arc;
use std::sync::atomic::Ordering;

use xguitare_core::arrangement::Arrangement;
use xguitare_core::engine::{
    Command, EngineQueues, EngineShared, FadeLengths, RackSwitcher, engine_queues,
};
use xguitare_core::metronome::{MetronomeSettings, TapTempo};
use xguitare_core::project::{Master, Project};
use xguitare_dsp::{Rack, StereoRack};
use xguitare_protocol::LoopRegion;

const RATE: f32 = 48_000.0;
const BLOCK: usize = 128;

struct Rig {
    switcher: RackSwitcher,
    control: EngineQueues,
    shared: Arc<EngineShared>,
}

fn rig() -> Rig {
    let mut rack = Rack::new(Vec::new()).expect("valid layout");
    rack.prepare(RATE, BLOCK);
    let (control, audio, _) = engine_queues(16);
    let shared = Arc::new(EngineShared::default());
    let switcher = RackSwitcher::new(
        Box::new(rack),
        BLOCK,
        FadeLengths::at(RATE, 20.0),
        audio,
        Arc::clone(&shared),
    );
    Rig {
        switcher,
        control,
        shared,
    }
}

fn settings(bpm: f64, meter: (u32, u32), volume_db: f32) -> MetronomeSettings {
    MetronomeSettings {
        on: true,
        bpm,
        meter,
        volume_db,
        sample_rate: RATE,
    }
}

impl Rig {
    fn send(&mut self, settings: MetronomeSettings) {
        assert!(
            self.control
                .commands
                .push(Command::Metronome(settings))
                .is_ok()
        );
    }

    fn run(&mut self, frames: usize) -> Vec<f32> {
        let input = [0.0; BLOCK];
        let mut out = Vec::with_capacity(frames);
        while out.len() < frames {
            let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
            self.switcher.apply_commands();
            self.switcher.process(&input, &mut left, &mut right);
            assert_eq!(left, right);
            out.extend_from_slice(&left);
        }
        out.truncate(frames);
        out
    }

    fn beat(&self) -> u32 {
        self.shared.metronome_beat.load(Ordering::Relaxed)
    }
}

/// The frames where a blip starts: sin(0) after silence, then sound.
fn onsets(samples: &[f32]) -> Vec<usize> {
    (0..samples.len() - 1)
        .filter(|&idx| {
            samples[idx] == 0.0 && samples[idx + 1] != 0.0 && (idx == 0 || samples[idx - 1] == 0.0)
        })
        .collect()
}

fn blip_peak(samples: &[f32], onset: usize) -> f32 {
    samples[onset..samples.len().min(onset + 240)]
        .iter()
        .fold(0.0, |peak: f32, s| peak.max(s.abs()))
}

fn db(level_db: f32) -> f32 {
    10f32.powf(level_db / 20.0)
}

/// Whether `louder` is 6 dB over `quieter`, within 1 dB: the accent's higher pitch peaks earlier in the decay.
fn six_db_over(louder: f32, quieter: f32) -> bool {
    (20.0 * (louder / quieter).log10() - 6.0).abs() < 1.0
}

#[test]
fn onsets_are_sixty_times_rate_over_bpm_apart() {
    for bpm in [120.0, 97.5, 300.0] {
        let mut rig = rig();
        rig.send(settings(bpm, (4, 4), -12.0));
        let spacing = 60.0 * f64::from(RATE) / bpm;
        let found = onsets(&rig.run((spacing * 9.5) as usize));
        assert_eq!(found.len(), 10, "{bpm} bpm: {found:?}");
        assert_eq!(found[0], 0, "{bpm} bpm starts at once");
        for (beat, &onset) in found.iter().enumerate() {
            let expected = beat as f64 * spacing;
            assert!(
                (onset as f64 - expected).abs() <= 1.0,
                "{bpm} bpm beat {beat} at {onset}, expected {expected}"
            );
        }
    }
}

#[test]
fn beat_one_is_accented_in_three_four_and_six_eight() {
    for (meter, beats) in [((3, 4), 3), ((6, 8), 6)] {
        let mut rig = rig();
        rig.send(settings(120.0, meter, 0.0));
        let out = rig.run(24_000 * (beats * 2 + 1));
        let found = onsets(&out);
        let normal = blip_peak(&out, found[1]);
        for (beat, &onset) in found.iter().enumerate() {
            let peak = blip_peak(&out, onset);
            let as_expected = if beat % beats == 0 {
                six_db_over(peak, normal)
            } else {
                (peak / normal - 1.0).abs() < 1e-3
            };
            assert!(
                as_expected,
                "{meter:?} beat {beat}: peak {peak}, normal {normal}"
            );
        }
    }
}

#[test]
fn volume_scales_the_peak() {
    let peak_at = |volume_db| {
        let mut rig = rig();
        rig.send(settings(120.0, (4, 4), volume_db));
        let out = rig.run(30_000);
        blip_peak(&out, 24_000)
    };
    let ratio = peak_at(-12.0) / peak_at(0.0);
    assert!((ratio - db(-12.0)).abs() < 1e-3, "ratio {ratio}");
}

#[test]
fn mute_silences_it_and_it_keeps_time() {
    let mut rig = rig();
    rig.shared.muted.store(true, Ordering::Relaxed);
    rig.run(4 * BLOCK);
    rig.send(settings(120.0, (4, 4), 0.0));
    let muted = rig.run(383 * BLOCK);
    assert!(muted.iter().all(|&sample| sample == 0.0));
    assert_eq!(rig.beat(), 3, "counting on under Mute");
    rig.shared.muted.store(false, Ordering::Relaxed);
    let after = rig.run(24_000 * 2);
    let found = onsets(&after);
    let done = muted.len();
    let expected = [3 * 24_000 - done, 4 * 24_000 - done];
    assert_eq!(found.len(), 2, "{found:?}");
    for (onset, expected) in found.iter().zip(expected) {
        assert!(onset.abs_diff(expected) <= 1, "{onset} vs {expected}");
    }
}

#[test]
fn it_starts_on_beat_one_and_a_tempo_change_keeps_the_count() {
    let mut rig = rig();
    assert_eq!(rig.beat(), 0);
    rig.run(1_000);
    assert_eq!(rig.beat(), 0, "off");
    rig.send(settings(120.0, (4, 4), 0.0));
    let first = rig.run(BLOCK);
    assert_eq!(onsets(&first), [0]);
    assert_eq!(rig.beat(), 1);
    let changed_at = 288 * BLOCK;
    rig.run(changed_at - BLOCK);
    assert_eq!(rig.beat(), 2);
    rig.send(settings(60.0, (4, 4), 0.0));
    let out = rig.run(50_000);
    let found = onsets(&out);
    let rest_of_beat = 2.0 - changed_at as f64 / 24_000.0;
    let expected = (rest_of_beat * 48_000.0).round() as usize;
    assert!(
        found[0].abs_diff(expected) <= 1,
        "the rest of the beat at the new tempo: {found:?}, expected {expected}"
    );
    assert_eq!(rig.beat(), 3);
    let normal = blip_peak(&out, found[0]);
    let off_and_on = |rig: &mut Rig| {
        let mut off = settings(60.0, (4, 4), 0.0);
        off.on = false;
        rig.send(off);
        rig.run(BLOCK);
        assert_eq!(rig.beat(), 0);
        rig.send(settings(60.0, (4, 4), 0.0));
        rig.run(BLOCK)
    };
    let restarted = off_and_on(&mut rig);
    assert_eq!(onsets(&restarted), [0]);
    assert_eq!(rig.beat(), 1);
    assert!(
        six_db_over(blip_peak(&restarted, 0), normal),
        "beat 1 again"
    );
}

#[test]
fn four_taps_half_a_second_apart_give_120() {
    let mut tap = TapTempo::default();
    let start = std::time::Instant::now();
    let at = |ms: u64| start + std::time::Duration::from_millis(ms);
    assert_eq!(tap.tap(at(0)), None);
    assert_eq!(tap.tap(at(500)), Some(120.0));
    assert_eq!(tap.tap(at(1000)), Some(120.0));
    assert_eq!(tap.tap(at(1500)), Some(120.0));
}

#[test]
fn tap_averages_the_last_four_taps() {
    let mut tap = TapTempo::default();
    let start = std::time::Instant::now();
    let at = |ms: u64| start + std::time::Duration::from_millis(ms);
    for ms in [0, 1000, 1500, 2000] {
        tap.tap(at(ms));
    }
    assert_eq!(
        tap.tap(at(2500)),
        Some(120.0),
        "the 1 s interval dropped out"
    );
}

#[test]
fn a_gap_over_two_seconds_starts_a_new_series() {
    let mut tap = TapTempo::default();
    let start = std::time::Instant::now();
    let at = |ms: u64| start + std::time::Duration::from_millis(ms);
    tap.tap(at(0));
    tap.tap(at(1000));
    assert_eq!(tap.tap(at(3001)), None);
    assert_eq!(tap.tap(at(3401)), Some(150.0));
}

#[test]
fn a_tapped_tempo_is_clamped() {
    let mut tap = TapTempo::default();
    let start = std::time::Instant::now();
    let at = |ms: u64| start + std::time::Duration::from_millis(ms);
    tap.tap(at(0));
    assert_eq!(tap.tap(at(100)), Some(300.0));
}

#[test]
fn an_arrangement_switches_the_practice_metronome_off() {
    let mut rig = rig();
    rig.send(settings(120.0, (4, 4), 0.0));
    rig.run(BLOCK);
    let project = Project {
        name: "Song".to_string(),
        sample_rate: RATE as u32,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 0.0,
        click: false,
        count_in: 0,
        next_item_id: 1,
        tracks: Vec::new(),
        buses: Vec::new(),
        master: Master::default(),
    };
    let mut master = StereoRack::new(Vec::new()).expect("no Modules");
    master.prepare(RATE, BLOCK);
    let arrangement = Arrangement::new(
        &project,
        Vec::new(),
        Vec::new(),
        master,
        &std::collections::HashMap::new(),
        RATE,
        BLOCK,
    )
    .expect("no Takes");
    for command in [
        Command::SwapArrangement(Box::new(arrangement)),
        Command::CloseArrangement,
    ] {
        assert!(rig.control.commands.push(command).is_ok());
    }
    let out = rig.run(30_000);
    assert!(out.iter().all(|&sample| sample == 0.0));
    assert_eq!(rig.beat(), 0);
}

#[test]
fn beats_played_counts_every_beat_of_a_one_beat_meter() {
    let mut rig = rig();
    rig.send(settings(120.0, (1, 4), -12.0));
    let played = |rig: &Rig| rig.shared.metronome_beats_played.load(Ordering::Relaxed);
    rig.run(BLOCK);
    assert_eq!((rig.beat(), played(&rig)), (1, 1));
    rig.run(24_000);
    assert_eq!((rig.beat(), played(&rig)), (1, 2));
}
