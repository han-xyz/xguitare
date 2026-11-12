use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use hound::{SampleFormat, WavSpec, WavWriter};
use xguitare_core::arrangement::{Arrangement, ArrangementCommand};
use xguitare_core::project::{LoopRegion, Master, Project, Take, Track};
use xguitare_core::take_capture::{
    FinishedTake, Latency, ReadStop, TakePlacement, TakeSpan, add_finished_take, add_take,
    next_take_file, take_position, take_ring,
};
use xguitare_dsp::{Rack, StereoRack};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!(
        "xguitare-take-capture-{}-{test}",
        std::process::id()
    ));
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
        to_master: true,
        sends: Vec::new(),
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

fn at(position: u64) -> TakePlacement {
    TakePlacement {
        position,
        offset: 0,
    }
}

fn write_take(path: &std::path::Path, samples: &[f32]) {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn read_all(
    reader: &mut xguitare_core::take_capture::TakeReader,
    written: u64,
) -> (Vec<f32>, ReadStop) {
    let mut out = Vec::new();
    let stop = reader.read(written, &mut out);
    (out, stop)
}

#[test]
fn take_position_moves_earlier_by_both_latencies_and_the_offset() {
    assert_eq!(take_position(48_000, 100, 200, 0.0, 48_000), at(47_700));
    assert_eq!(take_position(48_000, 100, 200, 10.0, 48_000), at(47_220));
    assert_eq!(take_position(48_000, 0, 0, -5.0, 48_000), at(48_240));
    assert_eq!(take_position(48_000, 0, 0, 1.0, 44_100), at(47_956));
}

#[test]
fn take_position_before_the_project_start_skips_into_the_take() {
    assert_eq!(
        take_position(100, 64, 64, 0.0, 48_000),
        TakePlacement {
            position: 0,
            offset: 28
        }
    );
}

#[test]
fn reader_gets_exactly_the_pushed_frames_and_the_take_end() {
    let (mut recorder, mut reader) = take_ring(100);
    recorder.push(500, &[0.1, 0.2, 0.3]);
    recorder.push(503, &[0.4]);
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples, [0.1, 0.2, 0.3, 0.4]);
    assert_eq!(stop, ReadStop::Empty);
    recorder.end_take(false);
    let (samples, stop) = read_all(&mut reader, 4);
    assert_eq!(samples, [0.0; 0]);
    assert_eq!(
        stop,
        ReadStop::TakeEnd(TakeSpan {
            start: 500,
            frames: 4,
            loop_take: false
        })
    );
}

#[test]
fn read_stops_at_the_end_of_a_take() {
    let (mut recorder, mut reader) = take_ring(100);
    recorder.push(0, &[1.0, 2.0]);
    recorder.end_take(false);
    recorder.push(10, &[3.0]);
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples, [1.0, 2.0]);
    assert!(matches!(
        stop,
        ReadStop::TakeEnd(TakeSpan { frames: 2, .. })
    ));
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
    recorder.end_take(false);
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples.len(), 10);
    assert!(matches!(
        stop,
        ReadStop::TakeEnd(TakeSpan { frames: 10, .. })
    ));
}

#[test]
fn a_take_after_a_loop_wrap_is_a_loop_take_until_recording_stops() {
    let (mut recorder, mut reader) = take_ring(100);
    recorder.push(0, &[0.0; 4]);
    recorder.end_take(true);
    recorder.push(0, &[0.0; 4]);
    recorder.end_take(false);
    recorder.push(50, &[0.0; 4]);
    recorder.end_take(false);
    let mut loop_takes = Vec::new();
    while let (_, ReadStop::TakeEnd(span)) = read_all(&mut reader, 0) {
        loop_takes.push(span.loop_take);
    }
    assert_eq!(loop_takes, [false, true, false]);
}

#[test]
fn a_replacing_recorder_continues_the_open_take() {
    let (mut old, mut old_reader) = take_ring(100);
    let (mut new, mut new_reader) = take_ring(100);
    old.push(200, &[1.0; 5]);
    new.continue_from(&mut old);
    old.push(205, &[9.0; 5]);
    new.push(205, &[2.0; 3]);
    new.end_take(false);
    let (samples, stop) = read_all(&mut old_reader, 0);
    assert_eq!(samples, [1.0; 5]);
    assert_eq!(stop, ReadStop::HandedOver);
    let (samples, stop) = read_all(&mut new_reader, 5);
    assert_eq!(samples, [2.0; 3]);
    assert_eq!(
        stop,
        ReadStop::TakeEnd(TakeSpan {
            start: 200,
            frames: 8,
            loop_take: false
        })
    );
}

#[test]
fn a_dropped_recorder_ends_its_open_take_then_is_abandoned() {
    let (mut recorder, mut reader) = take_ring(100);
    recorder.push(7, &[0.25; 3]);
    drop(recorder);
    let (samples, stop) = read_all(&mut reader, 0);
    assert_eq!(samples, [0.25; 3]);
    assert_eq!(
        stop,
        ReadStop::TakeEnd(TakeSpan {
            start: 7,
            frames: 3,
            loop_take: false
        })
    );
    assert_eq!(read_all(&mut reader, 0), (Vec::new(), ReadStop::Abandoned));
}

#[test]
fn first_take_makes_an_item_and_loop_takes_stack_on_it() {
    let mut t = track("t");
    let mut next = 1;
    add_take(&mut t, at(1000), take("takes/t-1.wav"), false, &mut next);
    add_take(&mut t, at(1000), take("takes/t-2.wav"), true, &mut next);
    add_take(&mut t, at(1000), take("takes/t-3.wav"), true, &mut next);
    assert_eq!(t.items.len(), 1);
    assert_eq!(t.items[0].position, 1000);
    assert_eq!(t.items[0].takes.len(), 3);
    assert_eq!(t.items[0].take, 2);
    add_take(&mut t, at(1000), take("takes/t-4.wav"), false, &mut next);
    add_take(&mut t, at(500), take("takes/t-5.wav"), true, &mut next);
    let skipped = TakePlacement {
        position: 500,
        offset: 10,
    };
    add_take(&mut t, skipped, take("takes/t-6.wav"), true, &mut next);
    assert_eq!(t.items.len(), 4);
    assert_eq!((t.items[2].position, t.items[2].take), (500, 0));
    assert_eq!((t.items[3].position, t.items[3].offset), (500, 10));
    let ids: Vec<u64> = t.items.iter().map(|item| item.id).collect();
    assert_eq!((ids, next), (vec![1, 2, 3, 4], 5));
    let mut last = u64::MAX;
    add_take(&mut t, at(9000), take("takes/t-7.wav"), false, &mut last);
    assert_eq!((t.items[4].id, last), (u64::MAX, u64::MAX));
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
    write_take(&dir.0.join("takes/t-1.wav"), &[0.5, -0.5, 0.25]);
    let mut project = Project {
        name: "p".to_string(),
        sample_rate: 48_000,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 1.0,
        click: false,
        count_in: 0,
        tracks: vec![track("t")],
        next_item_id: 1,
        buses: Vec::new(),
        master: Master::default(),
    };
    let mut takes = HashMap::new();
    let finished = FinishedTake {
        track_id: 0,
        file: "takes/t-1.wav".to_string(),
        span: TakeSpan {
            start: 10_000,
            frames: 3,
            loop_take: false,
        },
    };
    let latency = Latency {
        capture: 100,
        playback: 200,
    };
    let position = add_finished_take(&mut project, &dir.0, &mut takes, 0, &finished, latency);
    assert_eq!(position.unwrap(), Some(at(9652)));
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
    let position = add_finished_take(&mut project, &dir.0, &mut takes, 5, &finished, latency);
    assert_eq!(position.unwrap(), None);
}

#[test]
fn a_take_recorded_from_frame_0_starts_its_item_latency_frames_into_the_take() {
    let dir = temp_dir("offset");
    let samples: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
    write_take(&dir.0.join("takes/t-1.wav"), &samples);
    let mut project = Project {
        name: "p".to_string(),
        sample_rate: 48_000,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 0.0,
        click: false,
        count_in: 0,
        tracks: vec![track("t")],
        next_item_id: 1,
        buses: Vec::new(),
        master: Master::default(),
    };
    let mut takes = HashMap::new();
    let finished = FinishedTake {
        track_id: 0,
        file: "takes/t-1.wav".to_string(),
        span: TakeSpan {
            start: 0,
            frames: 1000,
            loop_take: false,
        },
    };
    let latency = Latency {
        capture: 128,
        playback: 128,
    };
    let placed =
        add_finished_take(&mut project, &dir.0, &mut takes, 0, &finished, latency).unwrap();
    assert_eq!(
        placed,
        Some(TakePlacement {
            position: 0,
            offset: 256
        })
    );
    let item = &project.tracks[0].items[0];
    assert_eq!((item.position, item.offset), (0, 256));

    project.tracks[0].arm = false;
    let mut rack = Rack::new(Vec::new()).unwrap();
    rack.prepare(48_000.0, 64);
    let mut arrangement = Arrangement::new(
        &project,
        vec![rack],
        Vec::new(),
        empty_master(),
        &takes,
        48_000.0,
        64,
    )
    .unwrap();
    arrangement.apply(ArrangementCommand::Play);
    let mut output = Vec::new();
    for _ in 0..5 {
        let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
        arrangement.process(&[], &mut left, &mut right);
        output.extend_from_slice(&left);
    }
    assert!((output[300] - samples[556]).abs() < 1e-6, "{}", output[300]);
    assert!((output[319] - samples[575]).abs() < 1e-6, "{}", output[319]);
}

fn empty_master() -> StereoRack {
    let mut master = StereoRack::new(Vec::new()).expect("no Modules");
    master.prepare(48_000.0, 64);
    master
}
