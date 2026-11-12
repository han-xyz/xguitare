use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use xguitare_core::arrangement::{Arrangement, ArrangementCommand, TakeNotLoaded, Transport};
use xguitare_core::engine::{
    Command, EngineShared, FadeLengths, FromAudio, RackSwitcher, engine_queues,
};
use xguitare_core::flatten::{PlacedTake, flatten, item_fade_frames};
use xguitare_core::project::{Item, LoopRegion, Master, Project, Take, Track};
use xguitare_core::take_capture::{ReadStop, TakeReader, TakeSpan};
use xguitare_dsp::{Gain, Module, Rack, StereoRack};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;
const TOLERANCE: f32 = 1e-5;

fn track(name: &str) -> Track {
    Track {
        name: name.to_string(),
        input: None,
        volume_db: 0.0,
        pan: 0.0,
        mute: false,
        solo: false,
        arm: false,
        to_master: true,
        sends: Vec::new(),
        modules: Vec::new(),
        items: Vec::new(),
    }
}

fn project(tracks: Vec<Track>) -> Project {
    Project {
        name: "test".to_string(),
        sample_rate: RATE,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 0.0,
        click: false,
        count_in: 0,
        next_item_id: 1,
        tracks,
        buses: Vec::new(),
        master: Master::default(),
    }
}

fn item(position: u64, file: &str, frames: u64) -> Item {
    Item {
        id: 0,
        position,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![Take {
            file: file.to_string(),
            frames,
        }],
    }
}

fn arrangement(project: &Project, takes: &HashMap<String, Arc<[f32]>>) -> Arrangement {
    let racks = project
        .tracks
        .iter()
        .map(|_| {
            let mut rack = Rack::new(Vec::new()).expect("valid layout");
            rack.prepare(RATE as f32, BLOCK);
            rack
        })
        .collect();
    Arrangement::new(
        project,
        racks,
        Vec::new(),
        empty_master(),
        takes,
        RATE as f32,
        BLOCK,
    )
    .expect("takes loaded")
}

fn run(arrangement: &mut Arrangement, inputs: &[&[f32]], frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let mut done = 0;
    while done < frames {
        let len = BLOCK.min(frames - done);
        let block_inputs: Vec<&[f32]> = inputs.iter().map(|input| &input[..len]).collect();
        let (mut l, mut r) = ([0.0; BLOCK], [0.0; BLOCK]);
        arrangement.process(&block_inputs, &mut l[..len], &mut r[..len]);
        left.extend_from_slice(&l[..len]);
        right.extend_from_slice(&r[..len]);
        done += len;
    }
    (left, right)
}

fn db(level_db: f32) -> f32 {
    10f32.powf(level_db / 20.0)
}

fn close(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() < TOLERANCE
}

#[test]
fn two_tracks_sum_with_volume_and_pan() {
    let mut a = track("a");
    a.arm = true;
    a.volume_db = -6.0;
    a.pan = -1.0;
    let mut b = track("b");
    b.arm = true;
    b.pan = 0.5;
    let mut arrangement = arrangement(&project(vec![a, b]), &HashMap::new());
    let (left, right) = run(&mut arrangement, &[&[0.5; BLOCK], &[0.25; BLOCK]], BLOCK);
    let angle = 0.75 * std::f32::consts::FRAC_PI_2;
    let sqrt2 = std::f32::consts::SQRT_2;
    let expected_l = 0.5 * db(-6.0) * sqrt2 + 0.25 * angle.cos() * sqrt2;
    let expected_r = 0.25 * angle.sin() * sqrt2;
    assert!(close(left[BLOCK - 1], expected_l), "{}", left[BLOCK - 1]);
    assert!(close(right[BLOCK - 1], expected_r), "{}", right[BLOCK - 1]);
}

#[test]
fn centred_track_at_zero_db_is_unity() {
    let mut a = track("a");
    a.arm = true;
    let mut arrangement = arrangement(&project(vec![a]), &HashMap::new());
    let (left, right) = run(&mut arrangement, &[&[0.5; BLOCK]], BLOCK);
    assert!(close(left[0], 0.5) && close(right[0], 0.5));
}

#[test]
fn volume_command_ramps_to_the_new_level() {
    let mut a = track("a");
    a.arm = true;
    let mut arrangement = arrangement(&project(vec![a]), &HashMap::new());
    arrangement.apply(ArrangementCommand::Volume {
        track: 0,
        db: -12.0,
    });
    let input = vec![1.0; 4800];
    let (left, _) = run(&mut arrangement, &[&input], 4800);
    assert!(left[0] > db(-12.0) + 0.1, "no jump: {}", left[0]);
    assert!(close(left[4799], db(-12.0)), "{}", left[4799]);
}

#[test]
fn mute_silences_and_solo_leaves_only_soloed_tracks() {
    let tracks = (0..3)
        .map(|idx| {
            let mut t = track(&format!("t{idx}"));
            t.arm = true;
            t
        })
        .collect();
    let mut arrangement = arrangement(&project(tracks), &HashMap::new());
    let levels = [0.1, 0.2, 0.4].map(|level| vec![level; 4800]);
    let inputs = levels.each_ref().map(Vec::as_slice);
    arrangement.apply(ArrangementCommand::Mute { track: 2, on: true });
    let (left, _) = run(&mut arrangement, &inputs, 4800);
    assert!(close(left[4799], 0.3), "{}", left[4799]);
    arrangement.apply(ArrangementCommand::Solo { track: 1, on: true });
    let (left, _) = run(&mut arrangement, &inputs, 4800);
    assert!(close(left[4799], 0.2), "{}", left[4799]);
    arrangement.apply(ArrangementCommand::Solo { track: 2, on: true });
    let (left, _) = run(&mut arrangement, &inputs, 4800);
    assert!(
        close(left[4799], 0.2),
        "a muted soloed Track stays silent: {}",
        left[4799]
    );
}

#[test]
fn armed_track_monitors_its_input_while_stopped() {
    let mut armed = track("armed");
    armed.arm = true;
    armed.pan = -1.0;
    let mut unarmed = track("unarmed");
    unarmed.pan = 1.0;
    let mut arrangement = arrangement(&project(vec![armed, unarmed]), &HashMap::new());
    assert_eq!(arrangement.transport(), Transport::Stopped);
    let (left, right) = run(&mut arrangement, &[&[0.5; BLOCK], &[0.5; BLOCK]], BLOCK);
    assert!(close(left[10], 0.5 * std::f32::consts::SQRT_2));
    assert!(right.iter().all(|&sample| sample == 0.0));
}

#[test]
fn item_plays_at_its_position_and_stops_at_its_end() {
    let mut t = track("t");
    t.items.push(item(100, "takes/t-1.wav", 50));
    let takes = HashMap::from([("takes/t-1.wav".to_string(), Arc::from(vec![1.0; 50]))]);
    let mut arrangement = arrangement(&project(vec![t]), &takes);
    let (stopped, _) = run(&mut arrangement, &[&[0.0; BLOCK]], BLOCK);
    assert!(stopped.iter().all(|&sample| sample == 0.0));
    arrangement.apply(ArrangementCommand::Play);
    let (left, _) = run(&mut arrangement, &[&[0.0; BLOCK]], BLOCK);
    for (frame, &sample) in left.iter().enumerate() {
        if !(100..150).contains(&frame) {
            assert!(close(sample, 0.0), "frame {frame}: {sample}");
        }
    }
    assert!(close(left[125], 0.98), "{}", left[125]);
    assert_eq!(arrangement.playhead(), BLOCK as u64);
}

/// 5 ms at [`RATE`].
const FADE_FRAMES: usize = 240;

fn played(items: Vec<(Item, f32)>, frames: usize) -> Vec<f32> {
    let items = items
        .into_iter()
        .map(|(item, level)| {
            let take = vec![level; item.takes[0].frames as usize];
            (item, take)
        })
        .collect();
    played_takes(items, frames)
}

fn played_takes(items: Vec<(Item, Vec<f32>)>, frames: usize) -> Vec<f32> {
    let mut t = track("t");
    let mut takes = HashMap::new();
    for (mut item, take) in items {
        let file = format!("takes/t-{}.wav", t.items.len() + 1);
        item.takes[0].file.clone_from(&file);
        takes.insert(file, Arc::from(take));
        t.items.push(item);
    }
    let mut arrangement = arrangement(&project(vec![t]), &takes);
    arrangement.apply(ArrangementCommand::Play);
    run(&mut arrangement, &[&[0.0; BLOCK]], frames).0
}

/// A Take whose frame n holds n / 10 000, so a sample tells which Take frame was read.
fn frame_ramp(frames: u64) -> Vec<f32> {
    (0..frames).map(|frame| frame as f32 / 10_000.0).collect()
}

fn silent(item: Item) -> (Item, Vec<f32>) {
    let take = vec![0.0; item.takes[0].frames as usize];
    (item, take)
}

/// Asserts `left[frames]` is Take frame `frame + shift` of [`frame_ramp`] times `gain(frame)`.
fn assert_ramp_read(
    left: &[f32],
    frames: std::ops::Range<usize>,
    shift: usize,
    gain: impl Fn(usize) -> f32,
) {
    for frame in frames {
        let expected = (frame + shift) as f32 / 10_000.0 * gain(frame);
        assert!(
            close(left[frame], expected),
            "frame {frame}: {} vs {expected}",
            left[frame]
        );
    }
}

#[test]
fn a_lower_item_plays_its_own_take_frames_under_a_top_fade_in() {
    let mut lower = item(0, "", 6000);
    lower.offset = 500;
    let left = played_takes(
        vec![(lower, frame_ramp(6000)), silent(item(1000, "", 2000))],
        1300,
    );
    let fade = FADE_FRAMES as f32;
    assert_ramp_read(&left, 1000..1000 + FADE_FRAMES, 500, |frame| {
        ((1240 - frame) as f32 - 0.5) / fade
    });
}

#[test]
fn a_lower_item_leads_in_with_its_own_take_frames_after_a_top_ends() {
    let mut lower = item(0, "", 6000);
    lower.offset = 500;
    let left = played_takes(
        vec![(lower, frame_ramp(6000)), silent(item(1000, "", 2000))],
        3300,
    );
    let fade = FADE_FRAMES as f32;
    assert_ramp_read(&left, 3000 - FADE_FRAMES..3000, 500, |frame| {
        ((frame - 2760) as f32 + 0.5) / fade
    });
    assert_ramp_read(&left, 3000..3300, 500, |_| 1.0);
}

#[test]
fn an_item_plays_on_into_its_take_at_a_join_with_another_take() {
    let mut first = item(0, "", 4800);
    first.length = Some(2000);
    let left = played_takes(
        vec![(first, frame_ramp(4800)), silent(item(2000, "", 2000))],
        2300,
    );
    let fade = FADE_FRAMES as f32;
    assert_ramp_read(&left, 2000..2000 + FADE_FRAMES, 0, |frame| {
        ((2240 - frame) as f32 - 0.5) / fade
    });
    assert!(left[2240..].iter().all(|&sample| sample == 0.0));
}

#[test]
fn an_item_edge_ramps_from_silence_over_5_ms() {
    let left = played(vec![(item(0, "", 4800), 1.0)], 4801);
    let step = 1.0 / FADE_FRAMES as f32;
    assert!(close(left[0], step / 2.0), "{}", left[0]);
    assert!(
        close(left[FADE_FRAMES / 2], 0.5 + step / 2.0),
        "{}",
        left[FADE_FRAMES / 2]
    );
    assert!(close(left[FADE_FRAMES], 1.0), "{}", left[FADE_FRAMES]);
    assert!(close(left[4799], step / 2.0), "{}", left[4799]);
    assert_eq!(left[4800], 0.0);
}

#[test]
fn a_sliver_ramps_up_and_down_over_half_its_length_each() {
    let cases: [(u64, &[f32]); 4] = [
        (1, &[0.5]),
        (2, &[0.5, 0.5]),
        (3, &[0.25, 0.75, 0.25]),
        (9, &[0.1, 0.3, 0.5, 0.7, 0.9, 0.7, 0.5, 0.3, 0.1]),
    ];
    for (frames, gains) in cases {
        let left = played(vec![(item(10, "", frames), 1.0)], 30);
        let sliver = &left[10..10 + gains.len()];
        for (frame, (&sample, &gain)) in sliver.iter().zip(gains).enumerate() {
            assert!(
                close(sample, gain),
                "{frames} frames, frame {frame}: {sample}"
            );
        }
        assert!(
            left[..10]
                .iter()
                .chain(&left[10 + gains.len()..])
                .all(|&s| s == 0.0)
        );
    }
}

#[test]
fn a_split_item_plays_exactly_as_before() {
    let take: Vec<f32> = (0..4800).map(|n| ((n % 97) as f32 / 97.0) - 0.5).collect();
    let render = |items: Vec<Item>| {
        let mut t = track("t");
        t.items = items;
        let takes = HashMap::from([("takes/t-1.wav".to_string(), Arc::from(take.clone()))]);
        let mut arrangement = arrangement(&project(vec![t]), &takes);
        arrangement.apply(ArrangementCommand::Play);
        run(&mut arrangement, &[&[0.0; BLOCK]], 5000).0
    };
    let whole = render(vec![item(0, "takes/t-1.wav", 4800)]);
    for at in [1, 7, 239, 240, 241, 2400, 4790, 4799] {
        let mut first = item(0, "takes/t-1.wav", 4800);
        first.length = Some(at);
        let mut second = item(at, "takes/t-1.wav", 4800);
        second.offset = at;
        let split = render(vec![first, second]);
        for (frame, (a, b)) in whole.iter().zip(&split).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "split at {at}, frame {frame}: {a} vs {b}"
            );
        }
    }
}

#[test]
fn abutting_items_of_two_takes_cross_with_no_dip() {
    let mut first = item(0, "", 4800);
    first.length = Some(2000);
    let left = played(vec![(first, 0.5), (item(2000, "", 2000), 0.5)], 4000);
    let inside = FADE_FRAMES..4000 - FADE_FRAMES;
    for (&sample, frame) in left[inside.clone()].iter().zip(inside) {
        assert!(close(sample, 0.5), "frame {frame}: {sample}");
    }
}

fn assert_level_across(left: &[f32], level: f32) {
    let step = level / FADE_FRAMES as f32;
    let inside = FADE_FRAMES..left.len() - FADE_FRAMES;
    for (&sample, frame) in left[inside.clone()].iter().zip(inside) {
        assert!(
            (sample - level).abs() <= step + TOLERANCE,
            "frame {frame}: {sample}"
        );
    }
}

#[test]
fn a_split_with_the_right_half_one_frame_left_crosses_with_no_dip() {
    let mut first = item(0, "", 4800);
    first.length = Some(2000);
    let mut second = item(1999, "", 4800);
    second.offset = 2000;
    let left = played(vec![(first, 0.5), (second, 0.5)], 4799);
    assert_level_across(&left, 0.5);
}

#[test]
fn a_split_with_the_left_half_one_frame_right_crosses_with_no_dip() {
    let mut second = item(2000, "", 4800);
    second.offset = 2000;
    let mut first = item(1, "", 4800);
    first.length = Some(2000);
    let left = played(vec![(second, 0.5), (first, 0.5)], 4800);
    assert_level_across(&left[1..], 0.5);
}

#[test]
fn a_covered_stretch_carries_none_of_the_lower_item() {
    let left = played(
        vec![(item(0, "", 4800), 0.25), (item(1000, "", 2000), 0.5)],
        4800,
    );
    let covered = 1000 + FADE_FRAMES..3000 - FADE_FRAMES;
    for (&sample, frame) in left[covered.clone()].iter().zip(covered) {
        assert!(close(sample, 0.5), "frame {frame}: {sample}");
    }
    assert!(close(left[500], 0.25), "{}", left[500]);
}

#[test]
fn the_crossfade_at_a_cut_keeps_an_equal_level() {
    let left = played(
        vec![(item(0, "", 4800), 0.5), (item(1000, "", 2000), 0.5)],
        4800,
    );
    let inside = FADE_FRAMES..4800 - FADE_FRAMES;
    for (&sample, frame) in left[inside.clone()].iter().zip(inside) {
        assert!(close(sample, 0.5), "frame {frame}: {sample}");
    }
}

#[test]
fn an_item_with_a_length_stops_at_its_end() {
    let mut short = item(0, "", 2000);
    short.length = Some(500);
    let left = played(vec![(short, 1.0)], 1000);
    assert!(close(left[250], 1.0), "{}", left[250]);
    assert!(left[500..].iter().all(|&sample| sample == 0.0));
}

#[test]
fn locate_moves_the_playhead() {
    let mut t = track("t");
    t.items.push(item(0, "takes/t-1.wav", 1000));
    let ramp: Vec<f32> = (0..1000).map(|frame| frame as f32 / 1000.0).collect();
    let takes = HashMap::from([("takes/t-1.wav".to_string(), Arc::from(ramp))]);
    let mut arrangement = arrangement(&project(vec![t]), &takes);
    arrangement.apply(ArrangementCommand::Locate(500));
    arrangement.apply(ArrangementCommand::Play);
    let (left, _) = run(&mut arrangement, &[&[0.0; BLOCK]], 10);
    assert!(close(left[3], 0.503), "{}", left[3]);
}

#[test]
fn armed_track_does_not_play_its_items() {
    let mut t = track("t");
    t.arm = true;
    t.items.push(item(0, "takes/t-1.wav", 50));
    let takes = HashMap::from([("takes/t-1.wav".to_string(), Arc::from(vec![1.0; 50]))]);
    let mut arrangement = arrangement(&project(vec![t]), &takes);
    arrangement.apply(ArrangementCommand::Play);
    let (left, _) = run(&mut arrangement, &[&[0.0; BLOCK]], BLOCK);
    assert!(left.iter().all(|&sample| sample == 0.0));
}

#[test]
fn loop_wraps_inside_a_block_on_the_exact_frame() {
    let mut t = track("t");
    t.items.push(item(0, "takes/t-1.wav", 1000));
    let ramp: Vec<f32> = (0..1000).map(|frame| frame as f32 / 1000.0).collect();
    let takes = HashMap::from([("takes/t-1.wav".to_string(), Arc::from(ramp))]);
    let mut p = project(vec![t]);
    p.loop_region = LoopRegion {
        start: 300,
        end: 410,
        on: true,
    };
    let mut arrangement = arrangement(&p, &takes);
    arrangement.apply(ArrangementCommand::Locate(300));
    arrangement.apply(ArrangementCommand::Play);
    let (left, _) = run(&mut arrangement, &[&[0.0; BLOCK]], BLOCK);
    for (frame, &sample) in left.iter().enumerate() {
        let expected = (300 + frame % 110) as f32 / 1000.0;
        assert!(close(sample, expected), "frame {frame}: {sample}");
    }
    assert_eq!(arrangement.playhead(), 300 + BLOCK as u64 % 110);
}

#[test]
fn an_empty_or_reversed_loop_keeps_the_running_loop() {
    let mut t = track("t");
    t.items.push(item(0, "takes/t-1.wav", 1000));
    let ramp: Vec<f32> = (0..1000).map(|frame| frame as f32 / 1000.0).collect();
    let takes = HashMap::from([("takes/t-1.wav".to_string(), Arc::from(ramp))]);
    let mut p = project(vec![t]);
    p.loop_region = LoopRegion {
        start: 10,
        end: 110,
        on: true,
    };
    let mut arrangement = arrangement(&p, &takes);
    for (start, end) in [(200, 200), (300, 100)] {
        arrangement.apply(ArrangementCommand::Loop(LoopRegion {
            start,
            end,
            on: true,
        }));
    }
    arrangement.apply(ArrangementCommand::Play);
    run(&mut arrangement, &[&[0.0; BLOCK]], BLOCK);
    assert_eq!(arrangement.playhead(), 10 + (BLOCK as u64 - 110) % 100);
}

fn zero_crossings(samples: &[f32]) -> usize {
    samples
        .windows(2)
        .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0) && pair[1] != 0.0)
        .count()
}

#[test]
fn click_lands_on_beats_with_an_accent_on_one() {
    let mut arrangement = arrangement(&project(Vec::new()), &HashMap::new());
    arrangement.apply(ArrangementCommand::Click(true));
    arrangement.apply(ArrangementCommand::Play);
    let beat = 24_000;
    let (left, right) = run(&mut arrangement, &[], 6 * beat);
    assert_eq!(left, right);
    let blip = 240;
    for idx in 0..6 {
        let start = idx * beat;
        if idx > 0 {
            assert!(left[start - 1] == 0.0, "silent before beat {idx}");
        }
        assert!(
            left[start] == 0.0,
            "the blip starts with sin(0) on beat {idx}"
        );
        assert!(left[start + 1] != 0.0, "click on beat {idx}");
        let crossings = zero_crossings(&left[start..start + blip]);
        let expected = if idx % 4 == 0 { 15 } else { 10 };
        assert!(
            crossings.abs_diff(expected) <= 1,
            "beat {idx}: {crossings} crossings"
        );
        assert!(
            left[start + 2 * blip..start + beat]
                .iter()
                .all(|&s| s == 0.0)
        );
    }
}

#[test]
fn click_off_is_silent() {
    let mut arrangement = arrangement(&project(Vec::new()), &HashMap::new());
    arrangement.apply(ArrangementCommand::Play);
    let (left, _) = run(&mut arrangement, &[], 30_000);
    assert!(left.iter().all(|&sample| sample == 0.0));
}

#[test]
fn count_in_holds_the_playhead_then_records() {
    let mut arrangement = arrangement(&project(Vec::new()), &HashMap::new());
    arrangement.apply(ArrangementCommand::CountIn(1));
    arrangement.apply(ArrangementCommand::Record);
    assert_eq!(arrangement.transport(), Transport::Recording);
    let bar = 4 * 24_000;
    let (left, _) = run(&mut arrangement, &[], bar - 100);
    assert!(arrangement.is_counting_in());
    assert_eq!(arrangement.playhead(), 0);
    assert!(left[1] != 0.0, "count-in clicks even with the click off");
    run(&mut arrangement, &[], 300);
    assert!(!arrangement.is_counting_in());
    assert_eq!(arrangement.playhead(), 200);
}

#[test]
fn stop_holds_the_playhead_and_silences_items() {
    let mut t = track("t");
    t.items.push(item(0, "takes/t-1.wav", 10_000));
    let takes = HashMap::from([("takes/t-1.wav".to_string(), Arc::from(vec![1.0; 10_000]))]);
    let mut arrangement = arrangement(&project(vec![t]), &takes);
    arrangement.apply(ArrangementCommand::Play);
    run(&mut arrangement, &[&[0.0; BLOCK]], 1000);
    arrangement.apply(ArrangementCommand::Stop);
    let (left, _) = run(&mut arrangement, &[&[0.0; BLOCK]], BLOCK);
    assert_eq!(arrangement.transport(), Transport::Stopped);
    assert_eq!(arrangement.playhead(), 1000);
    assert!(left.iter().all(|&sample| sample == 0.0));
}

#[test]
fn swapped_arrangement_continues_the_transport() {
    let mut old = arrangement(&project(Vec::new()), &HashMap::new());
    old.apply(ArrangementCommand::Play);
    run(&mut old, &[], 1000);
    let mut new = arrangement(&project(vec![track("t")]), &HashMap::new());
    new.continue_from(&mut old);
    assert_eq!(new.transport(), Transport::Playing);
    assert_eq!(new.playhead(), 1000);
}

#[test]
fn an_active_take_that_is_not_loaded_is_an_error() {
    let mut t = track("t");
    t.items.push(item(0, "takes/t-1.wav", 10));
    let p = project(vec![t]);
    let mut rack = Rack::new(Vec::new()).expect("valid layout");
    rack.prepare(RATE as f32, BLOCK);
    let error = Arrangement::new(
        &p,
        vec![rack],
        Vec::new(),
        empty_master(),
        &HashMap::new(),
        RATE as f32,
        BLOCK,
    )
    .err()
    .expect("missing Take");
    assert_eq!(error, TakeNotLoaded("takes/t-1.wav".to_string()));
    assert_eq!(error.to_string(), "take 'takes/t-1.wav' is not loaded");
}

fn empty_rack() -> Box<Rack> {
    let mut rack = Rack::new(Vec::new()).expect("valid layout");
    rack.prepare(RATE as f32, BLOCK);
    Box::new(rack)
}

#[test]
fn switcher_runs_a_swapped_in_arrangement_until_it_is_closed() {
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(RATE as f32, 20.0);
    let mut switcher = RackSwitcher::new(empty_rack(), BLOCK, fades, audio, Arc::default());
    let input = [0.5; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let mut t = track("t");
    t.arm = true;
    t.pan = -1.0;
    let first = arrangement(&project(vec![t.clone()]), &HashMap::new());
    let swap = Command::SwapArrangement(Box::new(first));
    assert!(control.commands.push(swap).is_ok());
    let play = Command::Arrangement(ArrangementCommand::Play);
    assert!(control.commands.push(play).is_ok());
    switcher.apply_commands();
    assert!(switcher.process_tracks(&input, &[&[0.25; BLOCK]], &mut left, &mut right));
    assert!(close(left[0], 0.25 * std::f32::consts::SQRT_2));
    assert!(right.iter().all(|&sample| sample == 0.0));

    let second = arrangement(&project(vec![t]), &HashMap::new());
    let swap = Command::SwapArrangement(Box::new(second));
    assert!(control.commands.push(swap).is_ok());
    switcher.apply_commands();
    let fade_blocks = 960_usize.div_ceil(BLOCK);
    for _ in 0..fade_blocks {
        assert!(
            control.from_audio.pop().is_err(),
            "retired before the fade ended"
        );
        switcher.process_tracks(&input, &[], &mut left, &mut right);
    }
    let Ok(FromAudio::RetiredArrangement(retired)) = control.from_audio.pop() else {
        panic!("expected the replaced Arrangement");
    };
    let swapped_at = (1 + fade_blocks) * BLOCK;
    assert_eq!(retired.playhead(), swapped_at as u64);

    assert!(control.commands.push(Command::CloseArrangement).is_ok());
    switcher.apply_commands();
    switcher.process(&input, &mut left, &mut right);
    let Ok(FromAudio::RetiredArrangement(closed)) = control.from_audio.pop() else {
        panic!("expected the closed Arrangement");
    };
    assert_eq!(closed.transport(), Transport::Playing);
    assert_eq!(closed.playhead(), swapped_at as u64);
    assert!(close(left[0], 0.5) && close(right[0], 0.5));
}

#[test]
fn a_set_on_a_module_added_during_a_crossfade_lands_on_the_new_rack() {
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(RATE as f32, 20.0);
    let mut switcher = RackSwitcher::new(empty_rack(), BLOCK, fades, audio, Arc::default());
    let input = [1.0; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let p = project(vec![armed("t")]);
    let gain_rack = || {
        let mut rack = Rack::new(vec![(
            "gain".to_string(),
            Box::new(Gain::new()) as Box<dyn Module>,
        )])
        .expect("valid layout");
        rack.prepare(RATE as f32, BLOCK);
        rack
    };
    let old_arrangement = Arrangement::new(
        &p,
        vec![gain_rack()],
        Vec::new(),
        empty_master(),
        &HashMap::new(),
        RATE as f32,
        BLOCK,
    )
    .expect("loaded");
    assert!(
        control
            .commands
            .push(Command::SwapArrangement(Box::new(old_arrangement)))
            .is_ok()
    );
    switcher.apply_commands();
    switcher.process_tracks(&input, &[&input], &mut left, &mut right);

    let with_gain = Arrangement::new(
        &p,
        vec![gain_rack()],
        Vec::new(),
        empty_master(),
        &HashMap::new(),
        RATE as f32,
        BLOCK,
    )
    .expect("loaded");
    let rack_add = Command::SwapArrangement(Box::new(with_gain));
    let set = Command::Arrangement(ArrangementCommand::Param {
        track: 0,
        module: 0,
        param: 0,
        value: -20.0,
    });
    let bypass = Command::Arrangement(ArrangementCommand::Bypass {
        track: 0,
        module: 0,
        on: false,
    });
    for command in [rack_add, set, bypass] {
        assert!(control.commands.push(command).is_ok());
    }
    switcher.apply_commands();
    for _ in 0..2 {
        switcher.process_tracks(&input, &[&input], &mut left, &mut right);
    }
    assert!(
        left[0] > 0.7,
        "the outgoing Arrangement's Module changed: {}",
        left[0]
    );
    for _ in 0..16 {
        switcher.process_tracks(&input, &[&input], &mut left, &mut right);
    }
    assert!(
        (left[BLOCK - 1] - db(-20.0)).abs() < 1e-3,
        "left {} is not -20 dB",
        left[BLOCK - 1]
    );
}

#[test]
fn module_and_parameter_indices_out_of_range_are_ignored() {
    let mut arrangement = arrangement(&project(vec![track("t")]), &HashMap::new());
    arrangement.apply(ArrangementCommand::Param {
        track: 0,
        module: 3,
        param: 0,
        value: 1.0,
    });
    arrangement.apply(ArrangementCommand::Bypass {
        track: 0,
        module: 3,
        on: true,
    });
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(RATE as f32, 20.0);
    let mut switcher = RackSwitcher::new(empty_rack(), BLOCK, fades, audio, Arc::default());
    let set = Command::SetParam {
        module: 3,
        param: 0,
        value: 1.0,
    };
    let bypass = Command::SetBypass {
        module: 3,
        on: true,
    };
    for command in [set, bypass] {
        assert!(control.commands.push(command).is_ok());
    }
    switcher.apply_commands();
}

fn armed(name: &str) -> Track {
    let mut t = track(name);
    t.arm = true;
    t
}

fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| i as f32 / 1000.0).collect()
}

/// Runs `input` through Track 1 in blocks that move along it.
fn feed(arrangement: &mut Arrangement, input: &[f32]) {
    for chunk in input.chunks(BLOCK) {
        let (mut l, mut r) = ([0.0; BLOCK], [0.0; BLOCK]);
        arrangement.process(&[chunk], &mut l[..chunk.len()], &mut r[..chunk.len()]);
    }
}

/// Every Take in `reader` with its samples, and the open Take's samples last.
fn spans(reader: &mut TakeReader) -> Vec<(Vec<f32>, Option<TakeSpan>)> {
    let mut found = Vec::new();
    let mut samples = Vec::new();
    let mut out = Vec::new();
    loop {
        let stop = reader.read(samples.len() as u64, &mut out);
        samples.extend_from_slice(&out);
        match stop {
            ReadStop::TakeEnd(span) => found.push((std::mem::take(&mut samples), Some(span))),
            ReadStop::Empty | ReadStop::HandedOver | ReadStop::Abandoned => break,
        }
    }
    if !samples.is_empty() {
        found.push((samples, None));
    }
    found
}

#[test]
fn recording_pushes_the_armed_dry_input_from_the_record_start() {
    let mut a = arrangement(&project(vec![armed("a"), track("b")]), &HashMap::new());
    let mut readers = a.take_readers();
    a.apply(ArrangementCommand::Locate(1000));
    a.apply(ArrangementCommand::Record);
    let input = ramp(600);
    feed(&mut a, &input);
    a.apply(ArrangementCommand::Stop);
    feed(&mut a, &ramp(300));
    let span = TakeSpan {
        start: 1000,
        frames: 600,
        loop_take: false,
    };
    assert_eq!(spans(&mut readers[0]), [(input, Some(span))]);
    assert_eq!(spans(&mut readers[1]), []);
}

#[test]
fn count_in_is_not_recorded() {
    let mut a = arrangement(&project(vec![armed("a")]), &HashMap::new());
    let mut readers = a.take_readers();
    a.apply(ArrangementCommand::CountIn(1));
    a.apply(ArrangementCommand::Record);
    let bar = 4 * 24_000;
    feed(&mut a, &ramp(bar + 200));
    a.apply(ArrangementCommand::Stop);
    let found = spans(&mut readers[0]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, ramp(bar + 200)[bar..]);
    assert_eq!(found[0].1.map(|span| span.start), Some(0));
}

#[test]
fn loop_wrap_while_recording_starts_a_loop_take() {
    let mut p = project(vec![armed("a")]);
    p.loop_region = LoopRegion {
        start: 100,
        end: 400,
        on: true,
    };
    let mut a = arrangement(&p, &HashMap::new());
    let mut readers = a.take_readers();
    a.apply(ArrangementCommand::Locate(100));
    a.apply(ArrangementCommand::Record);
    let input = ramp(700);
    feed(&mut a, &input);
    a.apply(ArrangementCommand::Stop);
    let found = spans(&mut readers[0]);
    let span = |frames, loop_take| {
        Some(TakeSpan {
            start: 100,
            frames,
            loop_take,
        })
    };
    assert_eq!(
        found,
        [
            (input[..300].to_vec(), span(300, false)),
            (input[300..600].to_vec(), span(300, true)),
            (input[600..].to_vec(), span(100, true)),
        ]
    );
}

#[test]
fn a_swapped_in_arrangement_continues_the_open_take() {
    let p = project(vec![armed("a")]);
    let mut old = arrangement(&p, &HashMap::new());
    let mut old_readers = old.take_readers();
    old.apply(ArrangementCommand::Record);
    let input = ramp(500);
    feed(&mut old, &input[..300]);
    let mut new = arrangement(&p, &HashMap::new());
    let mut new_readers = new.take_readers();
    new.continue_from(&mut old);
    feed(&mut old, &input[300..]);
    feed(&mut new, &input[300..]);
    new.apply(ArrangementCommand::Stop);
    assert_eq!(spans(&mut old_readers[0]), [(input[..300].to_vec(), None)]);
    let mut out = Vec::new();
    assert_eq!(
        new_readers[0].read(300, &mut out),
        ReadStop::TakeEnd(TakeSpan {
            start: 0,
            frames: 500,
            loop_take: false,
        })
    );
    assert_eq!(out, input[300..]);
}

#[test]
fn swap_arrangement_crossfades_with_no_step_above_the_fade_slope() {
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(RATE as f32, 20.0);
    let mut switcher = RackSwitcher::new(empty_rack(), BLOCK, fades, audio, Arc::default());
    let loud = arrangement(&project(vec![armed("t")]), &HashMap::new());
    let mut muted_track = armed("t");
    muted_track.mute = true;
    let silent = arrangement(&project(vec![muted_track]), &HashMap::new());
    let input = [1.0; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let mut heard = Vec::new();
    for swap_in in [loud, silent] {
        assert!(
            control
                .commands
                .push(Command::SwapArrangement(Box::new(swap_in)))
                .is_ok()
        );
        switcher.apply_commands();
        for _ in 0..8 {
            switcher.process_tracks(&input, &[&input], &mut left, &mut right);
            heard.extend_from_slice(&left);
        }
    }
    assert!(close(heard[0], 1.0));
    assert!(close(*heard.last().unwrap(), 0.0));
    let slope = 1.0 / 960.0;
    let steepest = heard
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max);
    assert!(
        steepest <= slope + TOLERANCE,
        "step {steepest} above {slope}"
    );
}

#[test]
fn a_full_take_ring_counts_disk_overflows() {
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(RATE as f32, 20.0);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(empty_rack(), BLOCK, fades, audio, Arc::clone(&shared));
    let a = arrangement(&project(vec![armed("t")]), &HashMap::new());
    assert!(
        control
            .commands
            .push(Command::SwapArrangement(Box::new(a)))
            .is_ok()
    );
    let record = Command::Arrangement(ArrangementCommand::Record);
    assert!(control.commands.push(record).is_ok());
    switcher.apply_commands();
    let input = [0.5; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let ring_blocks = 2 * RATE as usize / BLOCK;
    for _ in 0..ring_blocks {
        switcher.process_tracks(&input, &[&input], &mut left, &mut right);
    }
    assert_eq!(shared.disk_overflows.load(Ordering::Relaxed), 0);
    for _ in 0..3 {
        switcher.process_tracks(&input, &[&input], &mut left, &mut right);
    }
    assert_eq!(shared.disk_overflows.load(Ordering::Relaxed), 3);
}

#[test]
fn tempo_outside_the_project_range_keeps_the_old_beats() {
    let mut arrangement = arrangement(&project(Vec::new()), &HashMap::new());
    arrangement.apply(ArrangementCommand::Click(true));
    arrangement.apply(ArrangementCommand::Play);
    run(&mut arrangement, &[], 1000);
    for bpm in [f64::NAN, 0.0, -120.0, f64::INFINITY, 19.0, 301.0] {
        arrangement.apply(ArrangementCommand::Tempo { bpm, meter: (4, 4) });
    }
    for meter in [(0, 4), (4, 0), (4, 3), (17, 4)] {
        arrangement.apply(ArrangementCommand::Tempo { bpm: 60.0, meter });
    }
    let (left, _) = run(&mut arrangement, &[], 48_000);
    let beat = 24_000 - 1000;
    assert!(left[beat - 1] == 0.0 && left[beat] == 0.0);
    assert!(left[beat + 1] != 0.0, "the click keeps its 120 bpm beat");
    let crossings = zero_crossings(&left[beat..beat + 240]);
    assert!(
        crossings.abs_diff(10) <= 1,
        "{crossings} crossings: still 4/4"
    );
}

#[test]
fn non_finite_volume_and_pan_are_ignored() {
    let mut t = track("t");
    t.arm = true;
    let mut arrangement = arrangement(&project(vec![t]), &HashMap::new());
    for db in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        arrangement.apply(ArrangementCommand::Volume { track: 0, db });
    }
    arrangement.apply(ArrangementCommand::Pan {
        track: 0,
        pan: f32::NAN,
    });
    let (left, right) = run(&mut arrangement, &[&[0.5; BLOCK]], BLOCK);
    assert!(close(left[BLOCK - 1], 0.5) && close(right[BLOCK - 1], 0.5));
    arrangement.apply(ArrangementCommand::Volume { track: 0, db: -6.0 });
    let input = vec![0.5; 4800];
    let (left, _) = run(&mut arrangement, &[&input], 4800);
    assert!(close(left[4799], 0.5 * db(-6.0)), "{}", left[4799]);
}

#[test]
fn count_in_above_two_bars_is_ignored() {
    let mut arrangement = arrangement(&project(Vec::new()), &HashMap::new());
    arrangement.apply(ArrangementCommand::CountIn(1));
    arrangement.apply(ArrangementCommand::CountIn(1000));
    arrangement.apply(ArrangementCommand::Record);
    run(&mut arrangement, &[], 4 * 24_000 + 200);
    assert_eq!(arrangement.playhead(), 200);
}

#[test]
fn tempo_change_during_count_in_keeps_it_one_bar() {
    let mut arrangement = arrangement(&project(Vec::new()), &HashMap::new());
    arrangement.apply(ArrangementCommand::CountIn(1));
    arrangement.apply(ArrangementCommand::Record);
    run(&mut arrangement, &[], 24_000);
    arrangement.apply(ArrangementCommand::Tempo {
        bpm: 60.0,
        meter: (4, 4),
    });
    run(&mut arrangement, &[], 3 * 48_000 + 200);
    assert!(!arrangement.is_counting_in());
    assert_eq!(arrangement.playhead(), 200);
}

#[test]
fn switcher_publishes_the_count_in_beats_left() {
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(RATE as f32, 20.0);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(empty_rack(), BLOCK, fades, audio, Arc::clone(&shared));
    let input = [0.0; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let first = arrangement(&project(vec![armed("t")]), &HashMap::new());
    for command in [
        Command::SwapArrangement(Box::new(first)),
        Command::Arrangement(ArrangementCommand::CountIn(1)),
        Command::Arrangement(ArrangementCommand::Record),
    ] {
        assert!(control.commands.push(command).is_ok());
    }
    switcher.apply_commands();
    let beats_left = || shared.count_in_remaining.load(Ordering::Relaxed);
    switcher.process_tracks(&input, &[], &mut left, &mut right);
    assert_eq!(beats_left(), 4);
    let beat_blocks = 24_000 / BLOCK;
    for _ in 0..beat_blocks {
        switcher.process_tracks(&input, &[], &mut left, &mut right);
    }
    assert_eq!(beats_left(), 3);
    for _ in 0..3 * beat_blocks + 4 {
        switcher.process_tracks(&input, &[], &mut left, &mut right);
    }
    assert_eq!(beats_left(), 0);
    assert!(control.commands.push(Command::CloseArrangement).is_ok());
    switcher.apply_commands();
    assert_eq!(beats_left(), 0);
}

fn arrangement_at(project: &Project, rate: f32) -> Arrangement {
    let mut master = StereoRack::new(Vec::new()).expect("no Modules");
    master.prepare(rate, BLOCK);
    Arrangement::new(
        project,
        Vec::new(),
        Vec::new(),
        master,
        &HashMap::new(),
        rate,
        BLOCK,
    )
    .expect("no Takes")
}

#[test]
fn a_rebuild_at_a_new_rate_clicks_at_that_rate_with_the_metronome_volume() {
    let project = project(Vec::new());
    let mut old = arrangement_at(&project, 48_000.0);
    old.apply(ArrangementCommand::ClickVolume(-6.0));
    let mut rebuilt = arrangement_at(&project, 96_000.0);
    rebuilt.continue_from(&mut old);
    let mut fresh = arrangement_at(&project, 96_000.0);
    fresh.apply(ArrangementCommand::ClickVolume(-6.0));
    for arrangement in [&mut rebuilt, &mut fresh] {
        arrangement.apply(ArrangementCommand::Click(true));
        arrangement.apply(ArrangementCommand::Play);
    }
    let (continued, _) = run(&mut rebuilt, &[], 2 * 48_000);
    let (expected, _) = run(&mut fresh, &[], 2 * 48_000);
    assert_eq!(continued, expected);
}

fn click_peak(samples: &[f32], beat: usize) -> f32 {
    samples[beat * 24_000..beat * 24_000 + 240]
        .iter()
        .fold(0.0, |peak: f32, s| peak.max(s.abs()))
}

#[test]
fn metronome_volume_reaches_the_click_and_beat_one_is_six_db_louder() {
    let peaks_at = |volume_db: f32| {
        let mut arrangement = arrangement(&project(Vec::new()), &HashMap::new());
        arrangement.apply(ArrangementCommand::ClickVolume(volume_db));
        arrangement.apply(ArrangementCommand::Click(true));
        arrangement.apply(ArrangementCommand::Play);
        let (left, _) = run(&mut arrangement, &[], 5 * 24_000);
        (
            click_peak(&left, 0),
            click_peak(&left, 1),
            click_peak(&left, 4),
        )
    };
    let (accent, beat, next_accent) = peaks_at(0.0);
    let (_, quiet_beat, _) = peaks_at(-12.0);
    assert!((quiet_beat / beat - db(-12.0)).abs() < 1e-3);
    assert!((accent / next_accent - 1.0).abs() < 1e-3);
    let accent_db = 20.0 * (accent / beat).log10();
    assert!((accent_db - 6.0).abs() < 1.0, "accent {accent_db} dB");
}

#[test]
fn switcher_publishes_the_click_beat() {
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(RATE as f32, 20.0);
    let shared = Arc::new(EngineShared::default());
    let mut switcher = RackSwitcher::new(empty_rack(), BLOCK, fades, audio, Arc::clone(&shared));
    let input = [0.0; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let first = arrangement(&project(Vec::new()), &HashMap::new());
    for command in [
        Command::SwapArrangement(Box::new(first)),
        Command::Arrangement(ArrangementCommand::Click(true)),
        Command::Arrangement(ArrangementCommand::Play),
    ] {
        assert!(control.commands.push(command).is_ok());
    }
    switcher.apply_commands();
    let beat = || shared.metronome_beat.load(Ordering::Relaxed);
    switcher.process_tracks(&input, &[], &mut left, &mut right);
    assert_eq!(beat(), 1);
    for _ in 0..24_000 / BLOCK {
        switcher.process_tracks(&input, &[], &mut left, &mut right);
    }
    assert_eq!(beat(), 2);
    assert_eq!(shared.metronome_beats_played.load(Ordering::Relaxed), 2);
    assert!(
        control
            .commands
            .push(Command::Arrangement(ArrangementCommand::Click(false)))
            .is_ok()
    );
    switcher.apply_commands();
    switcher.process_tracks(&input, &[], &mut left, &mut right);
    assert_eq!(beat(), 0, "click off");
}

fn empty_master() -> StereoRack {
    let mut master = StereoRack::new(Vec::new()).expect("no Modules");
    master.prepare(RATE as f32, BLOCK);
    master
}

#[test]
fn stop_and_click_off_forget_the_last_click_beat() {
    let mut arrangement = arrangement(&project(Vec::new()), &HashMap::new());
    arrangement.apply(ArrangementCommand::Click(true));
    arrangement.apply(ArrangementCommand::Play);
    let beat = 24_000;
    run(&mut arrangement, &[], beat + BLOCK);
    assert_eq!(arrangement.click_beat(), 2);
    arrangement.apply(ArrangementCommand::Stop);
    arrangement.apply(ArrangementCommand::Locate(beat as u64 / 2));
    arrangement.apply(ArrangementCommand::Play);
    run(&mut arrangement, &[], BLOCK);
    assert_eq!(
        arrangement.click_beat(),
        0,
        "after Stop, until the next beat"
    );
    run(&mut arrangement, &[], beat);
    assert_eq!(arrangement.click_beat(), 2);
    arrangement.apply(ArrangementCommand::Click(false));
    arrangement.apply(ArrangementCommand::Click(true));
    run(&mut arrangement, &[], BLOCK);
    assert_eq!(
        arrangement.click_beat(),
        0,
        "after Click off, until the next beat"
    );
}

fn placed(items: &[Item], takes: &HashMap<String, Arc<[f32]>>) -> Vec<PlacedTake> {
    flatten(items, takes, item_fade_frames(RATE as f32)).expect("takes loaded")
}

fn steepest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

/// Four Takes on one 48 000-frame Item each: DC 1.0, 0.5 and 0.25, and [`frame_ramp`].
fn four_takes() -> (Vec<Item>, HashMap<String, Arc<[f32]>>) {
    let files = [
        "takes/t-1.wav",
        "takes/t-2.wav",
        "takes/t-3.wav",
        "takes/t-4.wav",
    ];
    let audio: [Vec<f32>; 4] = [
        vec![1.0; 48_000],
        vec![0.5; 48_000],
        vec![0.25; 48_000],
        frame_ramp(48_000),
    ];
    let items = files.iter().map(|file| item(0, file, 48_000)).collect();
    let takes = files
        .iter()
        .zip(audio)
        .map(|(file, samples)| ((*file).to_string(), Arc::from(samples)))
        .collect();
    (items, takes)
}

/// What a fresh build of one Track playing `items` gives over `frames` frames from `from`.
fn fresh_render(
    items: Vec<Item>,
    takes: &HashMap<String, Arc<[f32]>>,
    from: u64,
    frames: usize,
) -> Vec<f32> {
    let mut t = track("t");
    t.items = items;
    let mut fresh = arrangement(&project(vec![t]), takes);
    fresh.apply(ArrangementCommand::Locate(from));
    fresh.apply(ArrangementCommand::Play);
    run(&mut fresh, &[&[0.0; BLOCK]], frames).0
}

fn assert_same_from(heard: &[f32], expected: &[f32], from: usize) {
    for (frame, (a, b)) in heard.iter().zip(expected).enumerate().skip(from) {
        assert!(close(*a, *b), "frame {frame}: {a} vs {b}");
    }
}

#[test]
fn set_track_items_crossfades_mid_note_then_plays_like_a_fresh_build() {
    let (items, takes) = four_takes();
    let mut t = track("t");
    t.items = vec![items[0].clone()];
    let mut a = arrangement(&project(vec![t]), &takes);
    a.apply(ArrangementCommand::Play);
    let mut heard = run(&mut a, &[&[0.0; BLOCK]], 1000).0;
    let swap = ArrangementCommand::SetTrackItems {
        track: 0,
        items: placed(&items[3..], &takes),
    };
    assert!(a.apply(swap).is_none(), "nothing to hand back yet");
    heard.extend(run(&mut a, &[&[0.0; BLOCK]], 1000).0);
    let slope = 1.0 / FADE_FRAMES as f32;
    let steepest = steepest_step(&heard);
    assert!(
        steepest <= slope + TOLERANCE,
        "step {steepest} above {slope}"
    );
    assert!(close(heard[999], 1.0), "{}", heard[999]);
    let expected = fresh_render(vec![items[3].clone()], &takes, 1000, 1000);
    assert_same_from(&heard[1000..], &expected, FADE_FRAMES);
}

#[test]
fn set_track_items_during_a_fade_waits_and_the_newest_replaces_a_waiting_list() {
    let (items, takes) = four_takes();
    let mut t = track("t");
    t.items = vec![items[0].clone()];
    let mut a = arrangement(&project(vec![t]), &takes);
    a.apply(ArrangementCommand::Play);
    let mut heard = run(&mut a, &[&[0.0; BLOCK]], 1000).0;
    let swap = |index: usize| ArrangementCommand::SetTrackItems {
        track: 0,
        items: placed(&items[index..=index], &takes),
    };
    assert!(a.apply(swap(1)).is_none());
    heard.extend(run(&mut a, &[&[0.0; BLOCK]], 100).0);
    assert!(a.apply(swap(2)).is_none(), "waits for the running fade");
    let replaced = a.apply(swap(3)).expect("the waiting list comes back");
    assert!(close(replaced[0].samples[0], 0.25));
    heard.extend(run(&mut a, &[&[0.0; BLOCK]], 1000).0);
    let slope = 1.0 / FADE_FRAMES as f32;
    let steepest = steepest_step(&heard);
    assert!(
        steepest <= slope + TOLERANCE,
        "step {steepest} above {slope}"
    );
    assert!(
        close(heard[1300], 0.5),
        "the first fade ran to its end: {}",
        heard[1300]
    );
    let expected = fresh_render(vec![items[3].clone()], &takes, 1600, 500);
    assert_same_from(&heard[1600..], &expected, 0);
}

#[test]
fn set_track_items_for_a_track_out_of_range_hands_the_list_back() {
    let (items, takes) = four_takes();
    let mut a = arrangement(&project(vec![track("t")]), &takes);
    let swap = ArrangementCommand::SetTrackItems {
        track: 1,
        items: placed(&items[..1], &takes),
    };
    assert_eq!(a.apply(swap).map(|list| list.len()), Some(1));
}

#[test]
fn the_switcher_hands_the_replaced_item_list_back_after_the_fade() {
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(RATE as f32, 20.0);
    let mut switcher = RackSwitcher::new(empty_rack(), BLOCK, fades, audio, Arc::default());
    let (items, takes) = four_takes();
    let input = [0.0; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let swap = |index: usize| {
        Command::Arrangement(ArrangementCommand::SetTrackItems {
            track: 0,
            items: placed(&items[index..=index], &takes),
        })
    };
    assert!(control.commands.push(swap(1)).is_ok());
    switcher.apply_commands();
    assert!(
        matches!(control.from_audio.pop(), Ok(FromAudio::RetiredItems(_))),
        "with no Arrangement the list comes straight back"
    );

    let mut t = track("t");
    t.items = vec![items[0].clone()];
    let first = arrangement(&project(vec![t]), &takes);
    for command in [
        Command::SwapArrangement(Box::new(first)),
        Command::Arrangement(ArrangementCommand::Play),
    ] {
        assert!(control.commands.push(command).is_ok());
    }
    switcher.apply_commands();
    switcher.process_tracks(&input, &[], &mut left, &mut right);
    assert!(control.commands.push(swap(1)).is_ok());
    switcher.apply_commands();
    assert!(control.from_audio.pop().is_err(), "retired before the fade");
    switcher.process_tracks(&input, &[], &mut left, &mut right);
    let Ok(FromAudio::RetiredItems(old)) = control.from_audio.pop() else {
        panic!("expected the replaced Item list");
    };
    assert!(close(old[0].samples[0], 1.0));
    switcher.process_tracks(&input, &[], &mut left, &mut right);
    assert!(close(left[BLOCK - 1], 0.5), "{}", left[BLOCK - 1]);
}
