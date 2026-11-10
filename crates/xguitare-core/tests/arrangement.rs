use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use xguitare_core::arrangement::{Arrangement, ArrangementCommand, TakeNotLoaded, Transport};
use xguitare_core::engine::{
    Command, EngineShared, FadeLengths, FromAudio, RackSwitcher, engine_queues,
};
use xguitare_core::project::{Item, LoopRegion, Project, Take, Track};
use xguitare_core::take_capture::{ReadStop, TakeReader, TakeSpan};
use xguitare_dsp::Rack;

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
        tracks,
    }
}

fn item(position: u64, file: &str, frames: u64) -> Item {
    Item {
        position,
        offset: 0,
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
    Arrangement::new(project, racks, takes, RATE as f32, BLOCK).expect("takes loaded")
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
        let expected = if (100..150).contains(&frame) {
            1.0
        } else {
            0.0
        };
        assert!(close(sample, expected), "frame {frame}: {sample}");
    }
    assert_eq!(arrangement.playhead(), BLOCK as u64);
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
    let ramp: Vec<f32> = (0..1000).map(|frame| frame as f32).collect();
    let takes = HashMap::from([("takes/t-1.wav".to_string(), Arc::from(ramp))]);
    let mut p = project(vec![t]);
    p.loop_region = LoopRegion {
        start: 10,
        end: 110,
        on: true,
    };
    let mut arrangement = arrangement(&p, &takes);
    arrangement.apply(ArrangementCommand::Play);
    let (left, _) = run(&mut arrangement, &[&[0.0; BLOCK]], BLOCK);
    for (frame, &sample) in left.iter().enumerate() {
        let expected = if frame < 110 {
            frame
        } else {
            10 + (frame - 110) % 100
        };
        assert!(close(sample, expected as f32), "frame {frame}: {sample}");
    }
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
    let error = Arrangement::new(&p, vec![rack], &HashMap::new(), RATE as f32, BLOCK)
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
