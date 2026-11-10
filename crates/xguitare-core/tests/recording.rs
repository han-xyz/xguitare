use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use hound::{SampleFormat, WavSpec, WavWriter};
use xguitare_core::project::{LoopRegion, Project, Take, Track};
use xguitare_core::recording::{
    FinishedTake, Latency, Pass, ReadStop, add_finished_take, add_take, next_take_file,
    take_position, take_ring,
};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir =
        std::env::temp_dir().join(format!("xguitare-recording-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("takes")).unwrap();
    TempDir(dir)
}

fn track(name: &str) -> Track {
    Track {
        name: name.to_string(),
        input: None,
        volume_db: 0.0,
        pan: 0.0,
        mute: false,
        solo: false,
        arm: true,
        modules: Vec::new(),
        items: Vec::new(),
    }
}

fn take(file: &str) -> Take {
    Take {
        file: file.to_string(),
        frames: 10,
    }
}

fn read_all(
    reader: &mut xguitare_core::recording::TakeReader,
    written: u64,
) -> (Vec<f32>, ReadStop) {
    let mut out = Vec::new();
    let stop = reader.read(written, &mut out);
    (out, stop)
}

#[test]
fn take_position_moves_earlier_by_both_latencies_and_the_offset() {
    assert_eq!(take_position(48_000, 100, 200, 0.0, 48_000), 47_700);
    assert_eq!(take_position(48_000, 100, 200, 10.0, 48_000), 47_220);
    assert_eq!(take_position(48_000, 0, 0, -5.0, 48_000), 48_240);
    assert_eq!(take_position(48_000, 0, 0, 1.0, 44_100), 47_956);
}

#[test]
fn take_position_stops_at_the_project_start() {
    assert_eq!(take_position(100, 64, 64, 0.0, 48_000), 0);
}

#[test]
fn reader_gets_exactly_the_pushed_frames_and_the_pass() {
    let (mut recorder, mut reader) = take_ring(100);
    recorder.push(500, &[0.1, 0.2, 0.3]);
    recorder.push(503, &[0.4]);
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples, [0.1, 0.2, 0.3, 0.4]);
    assert_eq!(stop, ReadStop::Empty);
    recorder.end_pass(false);
    let (samples, stop) = read_all(&mut reader, 4);
    assert_eq!(samples, [0.0; 0]);
    assert_eq!(
        stop,
        ReadStop::PassEnd(Pass {
            start: 500,
            frames: 4,
            loop_pass: false
        })
    );
}

#[test]
fn read_stops_at_the_end_of_a_pass() {
    let (mut recorder, mut reader) = take_ring(100);
    recorder.push(0, &[1.0, 2.0]);
    recorder.end_pass(false);
    recorder.push(10, &[3.0]);
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples, [1.0, 2.0]);
    assert!(matches!(stop, ReadStop::PassEnd(Pass { frames: 2, .. })));
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples, [3.0]);
    assert_eq!(stop, ReadStop::Empty);
}

#[test]
fn a_full_ring_drops_frames_and_counts_one_overflow_per_push() {
    let (mut recorder, mut reader) = take_ring(10);
    recorder.push(0, &[0.5; 8]);
    assert_eq!(recorder.take_overflows(), 0);
    recorder.push(8, &[0.5; 8]);
    recorder.push(16, &[0.5; 8]);
    assert_eq!(recorder.take_overflows(), 2);
    assert_eq!(recorder.take_overflows(), 0);
    recorder.end_pass(false);
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples.len(), 10);
    assert!(matches!(stop, ReadStop::PassEnd(Pass { frames: 10, .. })));
}

#[test]
fn a_pass_after_a_loop_wrap_is_a_loop_pass_until_the_recording_ends() {
    let (mut recorder, mut reader) = take_ring(100);
    recorder.push(0, &[0.0; 4]);
    recorder.end_pass(true);
    recorder.push(0, &[0.0; 4]);
    recorder.end_pass(false);
    recorder.push(50, &[0.0; 4]);
    recorder.end_pass(false);
    let mut loop_passes = Vec::new();
    while let (_, ReadStop::PassEnd(pass)) = read_all(&mut reader, 0) {
        loop_passes.push(pass.loop_pass);
    }
    assert_eq!(loop_passes, [false, true, false]);
}

#[test]
fn a_replacing_recorder_continues_the_open_pass() {
    let (mut old, mut old_reader) = take_ring(100);
    let (mut new, mut new_reader) = take_ring(100);
    old.push(200, &[1.0; 5]);
    new.continue_from(&mut old);
    old.push(205, &[9.0; 5]);
    new.push(205, &[2.0; 3]);
    new.end_pass(false);
    let (samples, stop) = read_all(&mut old_reader, 0);
    assert_eq!(samples, [1.0; 5]);
    assert_eq!(stop, ReadStop::Done);
    let (samples, stop) = read_all(&mut new_reader, 5);
    assert_eq!(samples, [2.0; 3]);
    assert_eq!(
        stop,
        ReadStop::PassEnd(Pass {
            start: 200,
            frames: 8,
            loop_pass: false
        })
    );
}

#[test]
fn a_dropped_recorder_leaves_its_samples_then_is_done() {
    let (mut recorder, mut reader) = take_ring(100);
    recorder.push(0, &[0.25; 3]);
    drop(recorder);
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples, [0.25; 3]);
    assert_eq!(stop, ReadStop::Done);
}

#[test]
fn first_pass_makes_an_item_and_loop_passes_stack_takes_on_it() {
    let mut t = track("t");
    add_take(&mut t, 1000, take("takes/t-1.wav"), false);
    add_take(&mut t, 1000, take("takes/t-2.wav"), true);
    add_take(&mut t, 1000, take("takes/t-3.wav"), true);
    assert_eq!(t.items.len(), 1);
    assert_eq!(t.items[0].position, 1000);
    assert_eq!(t.items[0].takes.len(), 3);
    assert_eq!(t.items[0].take, 2);
    add_take(&mut t, 1000, take("takes/t-4.wav"), false);
    add_take(&mut t, 500, take("takes/t-5.wav"), true);
    assert_eq!(t.items.len(), 3);
    assert_eq!(t.items[2].position, 500);
    assert_eq!(t.items[2].take, 0);
}

#[test]
fn take_files_are_numbered_per_track_slug() {
    let dir = temp_dir("numbered");
    assert_eq!(
        next_take_file(&dir.0, "Lead Guitar"),
        "takes/lead-guitar-1.wav"
    );
    fs::write(dir.0.join("takes/lead-guitar-1.wav"), b"").unwrap();
    assert_eq!(
        next_take_file(&dir.0, "Lead Guitar"),
        "takes/lead-guitar-2.wav"
    );
    assert_eq!(next_take_file(&dir.0, "!!"), "takes/track-1.wav");
}

#[test]
fn finished_take_is_placed_latency_compensated_and_loaded() {
    let dir = temp_dir("finished");
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(dir.0.join("takes/t-1.wav"), spec).unwrap();
    for sample in [0.5, -0.5, 0.25] {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
    let mut project = Project {
        name: "p".to_string(),
        sample_rate: 48_000,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 1.0,
        tracks: vec![track("t")],
    };
    let mut takes = HashMap::new();
    let finished = FinishedTake {
        track: 0,
        file: "takes/t-1.wav".to_string(),
        pass: Pass {
            start: 10_000,
            frames: 3,
            loop_pass: false,
        },
    };
    let latency = Latency {
        capture: 100,
        playback: 200,
    };
    let position = add_finished_take(&mut project, &dir.0, &mut takes, &finished, latency);
    assert_eq!(position.unwrap(), Some(9652));
    assert_eq!(&*takes["takes/t-1.wav"], [0.5, -0.5, 0.25]);
    let item = &project.tracks[0].items[0];
    assert_eq!(item.position, 9652);
    assert_eq!(
        item.takes,
        [Take {
            file: "takes/t-1.wav".to_string(),
            frames: 3
        }]
    );
    let gone = FinishedTake {
        track: 5,
        ..finished
    };
    let position = add_finished_take(&mut project, &dir.0, &mut takes, &gone, latency);
    assert_eq!(position.unwrap(), None);
}
