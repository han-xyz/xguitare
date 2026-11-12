use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use hound::{SampleFormat, WavReader};
use xguitare::disk_writer::{DiskThread, TakeWriter};
use xguitare_core::arrangement::{Arrangement, ArrangementCommand};
use xguitare_core::engine::{Command, FadeLengths, RackSwitcher, engine_queues};
use xguitare_core::project::{LoopRegion, Master, Project, Track};
use xguitare_core::take_capture::{FinishedTake, TakeSpan, TrackReader, WritingTakes, take_ring};
use xguitare_dsp::{Rack, StereoRack};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-disk-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("takes")).unwrap();
    TempDir(dir)
}

fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| i as f32 / 1000.0).collect()
}

fn wav_samples(dir: &Path, file: &str) -> Vec<f32> {
    let reader = WavReader::open(dir.join(file)).unwrap();
    let spec = reader.spec();
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.sample_format),
        (1, 48_000, SampleFormat::Float)
    );
    reader.into_samples::<f32>().map(Result::unwrap).collect()
}

#[test]
fn writes_exactly_the_pushed_frames_into_a_wav() {
    let dir = temp_dir("exact");
    let (mut recorder, reader) = take_ring(1000);
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000, WritingTakes::default());
    writer.add_readers(vec![TrackReader {
        track_id: 0,
        name: "Lead Gtr".to_string(),
        reader,
    }]);
    let input = ramp(700);
    recorder.push(4800, &input[..300]);
    assert_eq!(writer.drain(), []);
    recorder.push(5100, &input[300..]);
    recorder.end_take(false);
    let finished = writer.drain();
    assert_eq!(
        finished,
        [FinishedTake {
            track_id: 0,
            file: "takes/lead-gtr-1.wav".to_string(),
            span: TakeSpan {
                start: 4800,
                frames: 700,
                loop_take: false,
            },
        }]
    );
    assert_eq!(wav_samples(&dir.0, "takes/lead-gtr-1.wav"), input);
}

#[test]
fn a_handed_over_ring_continues_its_take_in_the_next_reader() {
    let dir = temp_dir("closed");
    let (mut old, old_reader) = take_ring(1000);
    let (mut new, new_reader) = take_ring(1000);
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000, WritingTakes::default());
    writer.add_readers(vec![TrackReader {
        track_id: 0,
        name: "t".to_string(),
        reader: old_reader,
    }]);
    let input = ramp(250);
    old.push(0, &input[..100]);
    new.continue_from(&mut old);
    drop(old);
    assert_eq!(writer.drain(), []);
    writer.add_readers(vec![TrackReader {
        track_id: 0,
        name: "t".to_string(),
        reader: new_reader,
    }]);
    new.push(100, &input[100..]);
    new.end_take(false);
    let finished = writer.drain();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].span.frames, 250);
    assert_eq!(wav_samples(&dir.0, &finished[0].file), input);
}

#[test]
fn a_ring_dropped_mid_take_finishes_that_take_and_the_next_take_gets_its_own_file() {
    let dir = temp_dir("dropped");
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000, WritingTakes::default());
    let (mut dropped, reader) = take_ring(1000);
    writer.add_readers(vec![TrackReader {
        track_id: 0,
        name: "t".to_string(),
        reader,
    }]);
    let input = ramp(300);
    dropped.push(480, &input);
    assert_eq!(writer.drain(), []);
    drop(dropped);
    assert_eq!(
        writer.drain(),
        [FinishedTake {
            track_id: 0,
            file: "takes/t-1.wav".to_string(),
            span: TakeSpan {
                start: 480,
                frames: 300,
                loop_take: false,
            },
        }]
    );
    assert_eq!(wav_samples(&dir.0, "takes/t-1.wav"), input);

    let (mut next, reader) = take_ring(1000);
    writer.add_readers(vec![TrackReader {
        track_id: 0,
        name: "t".to_string(),
        reader,
    }]);
    next.push(0, &[0.5; 100]);
    next.end_take(false);
    let finished = writer.drain();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].file, "takes/t-2.wav");
    assert_eq!(finished[0].span.frames, 100);
    assert_eq!(wav_samples(&dir.0, "takes/t-2.wav"), [0.5; 100]);
}

#[test]
fn disk_thread_reports_each_finished_take() {
    let dir = temp_dir("thread");
    let disk = DiskThread::spawn(TakeWriter::new(
        dir.0.clone(),
        48_000,
        WritingTakes::default(),
    ))
    .unwrap();
    let (mut first, first_reader) = take_ring(1000);
    let (mut second, second_reader) = take_ring(1000);
    disk.add_readers(vec![
        TrackReader {
            track_id: 0,
            name: "a".to_string(),
            reader: first_reader,
        },
        TrackReader {
            track_id: 1,
            name: "b".to_string(),
            reader: second_reader,
        },
    ]);
    first.push(0, &[0.5; 64]);
    first.end_take(true);
    first.push(0, &[0.25; 32]);
    first.end_take(false);
    second.push(10, &[0.75; 16]);
    second.end_take(false);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut finished = Vec::new();
    while finished.len() < 3 && Instant::now() < deadline {
        finished.extend(disk.finished());
        thread::sleep(Duration::from_millis(5));
    }
    disk.stop();
    finished.sort_by(|a, b| a.file.cmp(&b.file));
    let files: Vec<(&str, u64, bool)> = finished
        .iter()
        .map(|take| (take.file.as_str(), take.span.frames, take.span.loop_take))
        .collect();
    assert_eq!(
        files,
        [
            ("takes/a-1.wav", 64, false),
            ("takes/a-2.wav", 32, true),
            ("takes/b-1.wav", 16, false),
        ]
    );
}

const BLOCK: usize = 256;

fn empty_rack() -> Rack {
    let mut rack = Rack::new(Vec::new()).expect("valid layout");
    rack.prepare(48_000.0, BLOCK);
    rack
}

fn armed_arrangement() -> Box<Arrangement> {
    let project = Project {
        name: "p".to_string(),
        sample_rate: 48_000,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 0.0,
        click: false,
        count_in: 0,
        next_item_id: 1,
        tracks: vec![Track {
            name: "t".to_string(),
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
        }],
        buses: Vec::new(),
        master: Master::default(),
    };
    let arrangement = Arrangement::new(
        &project,
        vec![empty_rack()],
        Vec::new(),
        empty_master(),
        &HashMap::new(),
        48_000.0,
        BLOCK,
    )
    .expect("no Takes");
    Box::new(arrangement)
}

#[test]
fn a_take_recorded_while_a_pending_arrangement_is_replaced_stays_one_file() {
    let dir = temp_dir("replaced");
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000, WritingTakes::default());
    let (mut control, audio, _tap) = engine_queues(16);
    let fades = FadeLengths::at(48_000.0, 20.0);
    let mut switcher =
        RackSwitcher::new(Box::new(empty_rack()), BLOCK, fades, audio, Arc::default());
    let mut arrangements = [(); 4].map(|()| armed_arrangement());
    for arrangement in &mut arrangements {
        writer.add_readers(
            arrangement
                .take_readers()
                .into_iter()
                .map(|reader| TrackReader {
                    track_id: 0,
                    name: "t".to_string(),
                    reader,
                })
                .collect(),
        );
    }
    let input = ramp(BLOCK * 16);
    let mut blocks = input.chunks(BLOCK);
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let [running, incoming, replaced, newest] = arrangements;
    let swaps = [running, incoming, replaced, newest].map(Command::SwapArrangement);
    let [running, incoming, replaced, newest] = swaps;
    for command in [running, Command::Arrangement(ArrangementCommand::Record)] {
        assert!(control.commands.push(command).is_ok());
    }
    switcher.apply_commands();
    let block = blocks.next().unwrap();
    switcher.process_tracks(block, &[block], &mut left, &mut right);
    for command in [incoming, replaced, newest] {
        assert!(control.commands.push(command).is_ok());
    }
    switcher.apply_commands();
    control.drain_from_audio();
    for block in blocks.by_ref().take(14) {
        switcher.process_tracks(block, &[block], &mut left, &mut right);
        control.drain_from_audio();
        assert_eq!(writer.drain(), []);
    }
    let stop = Command::Arrangement(ArrangementCommand::Stop);
    assert!(control.commands.push(stop).is_ok());
    switcher.apply_commands();
    let block = blocks.next().unwrap();
    switcher.process_tracks(block, &[block], &mut left, &mut right);
    let recorded = BLOCK * 15;
    assert_eq!(
        writer.drain(),
        [FinishedTake {
            track_id: 0,
            file: "takes/t-1.wav".to_string(),
            span: TakeSpan {
                start: 0,
                frames: recorded as u64,
                loop_take: false,
            },
        }]
    );
    assert_eq!(wav_samples(&dir.0, "takes/t-1.wav"), input[..recorded]);
}

fn empty_master() -> StereoRack {
    let mut master = StereoRack::new(Vec::new()).expect("no Modules");
    master.prepare(48_000.0, BLOCK);
    master
}

fn writing_files(writing: &WritingTakes) -> Vec<String> {
    let mut files: Vec<String> = writing
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .cloned()
        .collect();
    files.sort();
    files
}

#[test]
fn a_take_file_is_marked_writing_from_its_creation_until_the_project_receives_it() {
    let dir = temp_dir("writing");
    let writing = WritingTakes::default();
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000, Arc::clone(&writing));
    let (mut recorder, reader) = take_ring(1000);
    writer.add_readers(vec![TrackReader {
        track_id: 0,
        name: "t".to_string(),
        reader,
    }]);
    recorder.push(0, &[0.5; 100]);
    assert_eq!(writer.drain(), []);
    assert_eq!(writing_files(&writing), ["takes/t-1.wav"]);
    recorder.end_take(false);
    assert_eq!(writer.drain().len(), 1);
    assert_eq!(writing_files(&writing), ["takes/t-1.wav"]);
}

#[test]
fn a_take_file_that_cannot_be_created_is_not_marked_writing() {
    let dir = temp_dir("writing-failed");
    fs::remove_dir(dir.0.join("takes")).unwrap();
    let writing = WritingTakes::default();
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000, Arc::clone(&writing));
    let (mut recorder, reader) = take_ring(1000);
    writer.add_readers(vec![TrackReader {
        track_id: 0,
        name: "t".to_string(),
        reader,
    }]);
    recorder.push(0, &[0.5; 100]);
    recorder.end_take(false);
    assert_eq!(writer.drain(), []);
    assert_eq!(writing_files(&writing), Vec::<String>::new());
}
