use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use hound::{SampleFormat, WavReader};
use xguitare::disk_writer::{DiskThread, TakeWriter};
use xguitare_core::take_capture::{FinishedTake, TakeSpan, take_ring};

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
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000);
    writer.add_readers(vec![("Lead Gtr".to_string(), reader)]);
    let input = ramp(700);
    recorder.push(4800, &input[..300]);
    assert_eq!(writer.drain(), []);
    recorder.push(5100, &input[300..]);
    recorder.end_take(false);
    let finished = writer.drain();
    assert_eq!(
        finished,
        [FinishedTake {
            track: 0,
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
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000);
    writer.add_readers(vec![("t".to_string(), old_reader)]);
    let input = ramp(250);
    old.push(0, &input[..100]);
    new.continue_from(&mut old);
    drop(old);
    assert_eq!(writer.drain(), []);
    writer.add_readers(vec![("t".to_string(), new_reader)]);
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
    let mut writer = TakeWriter::new(dir.0.clone(), 48_000);
    let (mut dropped, reader) = take_ring(1000);
    writer.add_readers(vec![("t".to_string(), reader)]);
    let input = ramp(300);
    dropped.push(480, &input);
    assert_eq!(writer.drain(), []);
    drop(dropped);
    assert_eq!(
        writer.drain(),
        [FinishedTake {
            track: 0,
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
    writer.add_readers(vec![("t".to_string(), reader)]);
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
    let disk = DiskThread::spawn(TakeWriter::new(dir.0.clone(), 48_000)).unwrap();
    let (mut first, first_reader) = take_ring(1000);
    let (mut second, second_reader) = take_ring(1000);
    disk.add_readers(vec![
        ("a".to_string(), first_reader),
        ("b".to_string(), second_reader),
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
