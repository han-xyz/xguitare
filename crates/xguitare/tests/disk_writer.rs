use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use xguitare::disk_writer::DiskThread;
use xguitare_core::take_capture::{TrackReader, WritingTakes, take_ring};
use xguitare_core::take_writer::TakeWriter;

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
