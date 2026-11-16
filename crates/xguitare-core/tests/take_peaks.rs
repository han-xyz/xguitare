use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::{Value, json};
use xguitare_core::control::{EngineState, PeakMeters, Request, Response, handle};
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues};
use xguitare_core::metronome::Metronome;
use xguitare_core::project::{Item, Project, Take};
use xguitare_core::project_control::{Projects, add_finished_takes};
use xguitare_core::take_capture::{FinishedTake, Latency, TakeSpan};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BuildWarning, Config};

const FRAMES_PER_PEAK: usize = 256;
const STEP: f32 = 1.0 / 127.0;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let dir = std::env::temp_dir().join(format!("xguitare-peaks-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bank_dir = dir.join("bank");
    fs::create_dir_all(&bank_dir).unwrap();
    let config_path = dir.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let mut config = Config::load(Some(&config_path)).unwrap();
    config.bank_dir.clone_from(&bank_dir);
    config.projects_dir = dir.join("projects");
    let shared = Arc::new(EngineShared::default());
    shared.sample_rate.store(48_000, Ordering::Relaxed);
    let (queues, audio, tap) = engine_queues(64);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state = EngineState {
        bank: Bank::load_dir(&bank_dir).unwrap(),
        active: None,
        shared,
        queues,
        tuner,
        config,
        connections: Vec::new(),
        sample_rate: 48_000.0,
        max_frames: 128,
        revision: 1,
        modified: false,
        peaks: PeakMeters::default(),
        saved: HashMap::new(),
        projects: Projects::default(),
        metronome: Metronome::default(),
    };
    (state, audio, TempDir(dir))
}

fn ok((response, _): (Response, Vec<BuildWarning>)) -> Value {
    match response {
        Response::Ok(payload) => payload,
        Response::Err { error } => panic!("expected ok, got error {error}"),
    }
}

fn error((response, _): (Response, Vec<BuildWarning>)) -> String {
    match response {
        Response::Err { error } => error,
        Response::Ok(payload) => panic!("expected error, got {payload}"),
    }
}

fn write_take(path: &Path, samples: &[f32]) {
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

/// Opens a Project "Song" with one Track whose one Item has the Takes `takes/a.wav` (active) and `takes/b.wav`, holding `a` and `b`.
fn open_with_takes(state: &mut EngineState, a: &[f32], b: &[f32]) {
    ok(handle(
        state,
        Request::ProjectNew {
            name: "Song".to_string(),
        },
    ));
    ok(handle(
        state,
        Request::TrackAdd {
            name: Some("Guitar".to_string()),
        },
    ));
    ok(handle(state, Request::ProjectClose));
    let folder = state.config.projects_dir.join("song");
    write_take(&folder.join("takes/a.wav"), a);
    write_take(&folder.join("takes/b.wav"), b);
    let mut project = Project::load(&folder).unwrap().0;
    project.tracks[0].items = vec![Item {
        id: 1,
        position: 0,
        offset: 0,
        length: None,
        take: 0,
        takes: [("takes/a.wav", a), ("takes/b.wav", b)]
            .map(|(file, samples)| Take {
                file: file.to_string(),
                frames: samples.len() as u64,
            })
            .to_vec(),
    }];
    project.next_item_id = 2;
    project.save(&folder).unwrap();
    ok(handle(
        state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
}

fn take_peaks(file: &str, first: Option<usize>, count: Option<usize>) -> Request {
    Request::TakePeaks {
        file: file.to_string(),
        first,
        count,
    }
}

/// A plucked note: a 220 Hz sine decaying from full scale.
fn pluck(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            let t = n as f32 / 48_000.0;
            (-t * 6.0).exp() * (std::f32::consts::TAU * 220.0 * t).sin()
        })
        .collect()
}

fn peak_values(reply: &Value) -> Vec<i64> {
    reply["peaks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|peak| peak.as_i64().unwrap())
        .collect()
}

#[test]
fn the_peak_table_holds_each_256_frames_min_and_max_with_the_last_partial_bucket() {
    let (mut state, _audio, _dir) = state("table");
    let samples = pluck(FRAMES_PER_PEAK * 40 + 100);
    open_with_takes(&mut state, &samples, &[0.0; 10]);
    let reply = ok(handle(&mut state, take_peaks("takes/a.wav", None, None)));
    assert_eq!(reply["frames_per_peak"], 256);
    assert_eq!(reply["total"], 41);
    assert_eq!(reply["first"], 0);
    let peaks = peak_values(&reply);
    assert_eq!(peaks.len(), 82);
    for (bucket, (frames, pair)) in samples
        .chunks(FRAMES_PER_PEAK)
        .zip(peaks.chunks(2))
        .enumerate()
    {
        let min = frames.iter().copied().fold(f32::INFINITY, f32::min);
        let max = frames.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for (peak, sample) in pair.iter().zip([min, max]) {
            assert!(
                (*peak as f32 * STEP - sample).abs() <= STEP,
                "bucket {bucket}: {peak} for {sample}"
            );
        }
    }
}

#[test]
fn a_silent_take_has_zero_peaks() {
    let (mut state, _audio, _dir) = state("silent");
    open_with_takes(&mut state, &[0.0; 1000], &[0.0; 10]);
    let reply = ok(handle(&mut state, take_peaks("takes/a.wav", None, None)));
    assert_eq!(reply["total"], 4);
    assert_eq!(peak_values(&reply), [0; 8]);
}

#[test]
fn take_peaks_pages_from_first_for_count_pairs() {
    let (mut state, _audio, _dir) = state("paging");
    let samples: Vec<f32> = (0..FRAMES_PER_PEAK * 10)
        .map(|n| (n / FRAMES_PER_PEAK) as f32 / 10.0)
        .collect();
    open_with_takes(&mut state, &samples, &[0.0; 10]);
    let all = peak_values(&ok(handle(
        &mut state,
        take_peaks("takes/a.wav", None, None),
    )));
    let page = ok(handle(
        &mut state,
        take_peaks("takes/a.wav", Some(3), Some(4)),
    ));
    assert_eq!(
        (page["first"].clone(), page["total"].clone()),
        (json!(3), json!(10))
    );
    assert_eq!(peak_values(&page), all[6..14]);
    let tail = ok(handle(
        &mut state,
        take_peaks("takes/a.wav", Some(8), Some(5)),
    ));
    assert_eq!(peak_values(&tail), all[16..]);
    let past = ok(handle(
        &mut state,
        take_peaks("takes/a.wav", Some(12), None),
    ));
    assert_eq!(peak_values(&past), Vec::<i64>::new());
}

#[test]
fn take_peaks_names_its_errors() {
    let (mut state, _audio, _dir) = state("errors");
    assert_eq!(
        error(handle(&mut state, take_peaks("takes/a.wav", None, None))),
        "no project open"
    );
    open_with_takes(&mut state, &[0.5; 300], &[0.5; 300]);
    for file in ["takes/nothing.wav", "takes/b.wav"] {
        assert_eq!(
            error(handle(&mut state, take_peaks(file, None, None))),
            format!("no such take: {file}"),
            "b.wav is not the active Take, so not loaded"
        );
    }
    ok(handle(&mut state, Request::ItemTake { id: 1, take: 1 }));
    let reply = ok(handle(&mut state, take_peaks("takes/b.wav", None, None)));
    assert_eq!(peak_values(&reply), [64, 64, 64, 64]);
    ok(handle(&mut state, Request::ProjectClose));
    assert_eq!(
        error(handle(&mut state, take_peaks("takes/b.wav", None, None))),
        "no project open"
    );
}

#[test]
fn a_take_recorded_again_under_its_file_name_gets_a_new_table() {
    let (mut state, _audio, _dir) = state("reused");
    open_with_takes(&mut state, &[0.5; 300], &[0.0; 10]);
    let first = ok(handle(&mut state, take_peaks("takes/a.wav", None, None)));
    assert_eq!(first["frames"], 300);
    let items = ok(handle(&mut state, Request::Items));
    let track_id = items["tracks"][0]["id"].as_u64().unwrap();
    let folder = state.config.projects_dir.join("song");
    write_take(&folder.join("takes/a.wav"), &[-0.25; 600]);
    let finished = FinishedTake {
        track_id,
        file: "takes/a.wav".to_string(),
        span: TakeSpan {
            start: 48_000,
            frames: 600,
            loop_take: false,
        },
    };
    assert_eq!(
        add_finished_takes(&mut state, &[finished], |_| Latency::default()),
        Vec::<String>::new()
    );
    let reply = ok(handle(&mut state, take_peaks("takes/a.wav", None, None)));
    assert_eq!(
        (reply["frames"].clone(), reply["total"].clone()),
        (json!(600), json!(3))
    );
    assert_eq!(peak_values(&reply), [-32; 6]);
}
