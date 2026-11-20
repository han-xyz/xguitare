use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::{Value, json};
use xguitare_core::control::{EngineState, Request, Response, TrackRef, TransportAction, handle};
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues};
use xguitare_core::project::AUTOSAVE_FILE;
use xguitare_core::project::{Item, Project, Take};
use xguitare_core::project_control::{ProjectEvent, add_finished_takes, take_project_events};
use xguitare_core::take_capture::{FinishedTake, Latency, TakeSpan, WritingTakes};
use xguitare_core::takes_control::delete_take_files;
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BuildWarning, Config};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let dir = std::env::temp_dir().join(format!("xguitare-clean-{}-{test}", std::process::id()));
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
    let state = EngineState::new(
        Bank::load_dir(&bank_dir).unwrap(),
        shared,
        queues,
        tuner,
        config,
        48_000.0,
        128,
    );
    (state, audio, TempDir(dir))
}

fn ok((response, _): (Response, Vec<BuildWarning>)) -> Value {
    match response {
        Response::Ok(payload) => payload,
        Response::Err { error, .. } => panic!("expected ok, got error {error}"),
    }
}

fn error((response, _): (Response, Vec<BuildWarning>)) -> String {
    match response {
        Response::Err { error, .. } => error,
        Response::Ok(payload) => panic!("expected error, got {payload}"),
    }
}

fn write_take(path: &Path, frames: usize) {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for _ in 0..frames {
        writer.write_sample(0.25_f32).unwrap();
    }
    writer.finalize().unwrap();
}

fn take(file: &str) -> Take {
    Take {
        file: file.to_string(),
        frames: 480,
    }
}

/// The Song folder, its Take files and the disk thread's set of Takes being written: Item 1 plays `a` with `b` as its other Take, Item 2 plays `c`; `d`, `e` and `f` are on no Item.
fn open_song(state: &mut EngineState) -> (PathBuf, WritingTakes) {
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
    for (name, frames) in [
        ("a", 480),
        ("b", 480),
        ("c", 480),
        ("d", 480),
        ("e", 100),
        ("f", 200),
    ] {
        write_take(&folder.join(format!("takes/{name}.wav")), frames);
    }
    let mut project = Project::load(&folder).unwrap().0;
    project.tracks[0].items = vec![
        Item {
            id: 1,
            position: 0,
            offset: 0,
            length: None,
            take: 0,
            takes: vec![take("takes/a.wav"), take("takes/b.wav")],
        },
        Item {
            id: 2,
            position: 1000,
            offset: 0,
            length: None,
            take: 0,
            takes: vec![take("takes/c.wav")],
        },
    ];
    project.next_item_id = 3;
    project.save(&folder).unwrap();
    let _ = take_project_events(state);
    ok(handle(
        state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
    let writing = take_project_events(state)
        .into_iter()
        .find_map(|event| match event {
            ProjectEvent::Opened { writing, .. } => Some(writing),
            _ => None,
        })
        .unwrap();
    (folder, writing)
}

fn bytes(folder: &Path, file: &str) -> u64 {
    fs::metadata(folder.join(file)).unwrap().len()
}

fn unused_files(state: &mut EngineState) -> Vec<String> {
    ok(handle(state, Request::TakesUnused))["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["file"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn takes_unused_needs_an_open_project() {
    let (mut state, _audio, _dir) = state("no-project");
    assert_eq!(
        error(handle(&mut state, Request::TakesUnused)),
        "no project open"
    );
}

#[test]
fn takes_unused_lists_only_regular_wav_files_nothing_references() {
    let (mut state, _audio, dir) = state("listing");
    let (folder, writing) = open_song(&mut state);
    ok(handle(&mut state, Request::ItemCopy { ids: vec![2] }));
    ok(handle(&mut state, Request::ItemDelete { ids: vec![2] }));
    writing
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert("takes/d.wav".to_string());
    let takes = folder.join("takes");
    fs::write(takes.join("notes.txt"), "stray").unwrap();
    fs::create_dir(takes.join("sub.wav")).unwrap();
    let outside = dir.0.join("outside.wav");
    write_take(&outside, 10);
    std::os::unix::fs::symlink(&outside, takes.join("link.wav")).unwrap();
    let reply = ok(handle(&mut state, Request::TakesUnused));
    let (e, f) = (bytes(&folder, "takes/e.wav"), bytes(&folder, "takes/f.wav"));
    assert_eq!(
        reply,
        json!({
            "files": [
                {"file": "takes/e.wav", "bytes": e},
                {"file": "takes/f.wav", "bytes": f},
            ],
            "bytes": e + f,
        })
    );
}

#[test]
fn the_takes_of_an_item_whose_active_take_is_short_stay_in_use_after_open() {
    let (mut state, _audio, _dir) = state("short-active-take");
    ok(handle(
        &mut state,
        Request::ProjectNew {
            name: "Song".to_string(),
        },
    ));
    ok(handle(&mut state, Request::TrackAdd { name: None }));
    ok(handle(&mut state, Request::ProjectClose));
    let folder = state.config.projects_dir.join("song");
    for (name, frames) in [("a", 4800), ("b", 4800), ("c", 100)] {
        write_take(&folder.join(format!("takes/{name}.wav")), frames);
    }
    let mut project = Project::load(&folder).unwrap().0;
    let pass = |file: &str, frames| Take {
        file: file.to_string(),
        frames,
    };
    project.tracks[0].items = vec![Item {
        id: 1,
        position: 0,
        offset: 200,
        length: None,
        take: 1,
        takes: vec![
            pass("takes/a.wav", 4800),
            pass("takes/b.wav", 4800),
            pass("takes/c.wav", 100),
        ],
    }];
    project.next_item_id = 2;
    project.save(&folder).unwrap();
    let toml = folder.join("project.toml");
    let text = fs::read_to_string(&toml).unwrap();
    fs::write(&toml, text.replace("take = 1", "take = 2")).unwrap();
    ok(handle(
        &mut state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
    assert_eq!(unused_files(&mut state), Vec::<String>::new());
}

fn record(state: &mut EngineState) {
    ok(handle(
        state,
        Request::TrackSet {
            track: TrackRef::Index(0),
            input: None,
            volume: None,
            pan: None,
            mute: None,
            solo: None,
            arm: Some(true),
            name: None,
            to_master: None,
        },
    ));
    ok(handle(
        state,
        Request::Transport {
            action: TransportAction::Record,
            position: None,
        },
    ));
}

#[test]
fn a_take_being_recorded_is_not_listed() {
    let (mut state, _audio, _dir) = state("recording-listed");
    let (_folder, writing) = open_song(&mut state);
    record(&mut state);
    writing
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert("takes/e.wav".to_string());
    assert_eq!(unused_files(&mut state), ["takes/d.wav", "takes/f.wav"]);
}

#[test]
fn a_finished_take_leaves_the_writing_set_once_the_project_has_it() {
    let (mut state, _audio, _dir) = state("finished");
    let (_folder, writing) = open_song(&mut state);
    writing
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert("takes/e.wav".to_string());
    let finished = FinishedTake {
        track_id: 0,
        file: "takes/e.wav".to_string(),
        span: TakeSpan {
            start: 50_000,
            frames: 100,
            loop_take: false,
        },
    };
    assert_eq!(
        add_finished_takes(&mut state, &[finished], |_| Latency::default()),
        Vec::<String>::new()
    );
    assert_eq!(unused_files(&mut state), ["takes/d.wav", "takes/f.wav"]);
    ok(handle(&mut state, Request::ItemDelete { ids: vec![3] }));
    assert_eq!(
        unused_files(&mut state),
        ["takes/d.wav", "takes/e.wav", "takes/f.wav"]
    );
}

fn clean(state: &mut EngineState, files: &[&str]) -> (Response, Vec<BuildWarning>) {
    handle(
        state,
        Request::TakesClean {
            files: files.iter().map(ToString::to_string).collect(),
        },
    )
}

fn exists(folder: &Path, file: &str) -> bool {
    fs::symlink_metadata(folder.join(file)).is_ok()
}

#[test]
fn takes_clean_needs_an_open_project() {
    let (mut state, _audio, _dir) = state("clean-no-project");
    assert_eq!(
        error(clean(&mut state, &["takes/e.wav"])),
        "no project open"
    );
}

#[test]
fn takes_clean_deletes_exactly_the_named_unused_files() {
    let (mut state, _audio, _dir) = state("clean");
    let (folder, _writing) = open_song(&mut state);
    let e = bytes(&folder, "takes/e.wav");
    assert_eq!(
        ok(clean(&mut state, &["takes/e.wav"])),
        json!({"deleted": 1, "bytes": e, "failed": []})
    );
    assert!(!exists(&folder, "takes/e.wav"));
    for name in ["a", "b", "c", "d", "f"] {
        assert!(exists(&folder, &format!("takes/{name}.wav")), "{name}");
    }
    assert_eq!(unused_files(&mut state), ["takes/d.wav", "takes/f.wav"]);
}

#[test]
fn a_referenced_take_fails_the_whole_request() {
    let (mut state, _audio, _dir) = state("clean-referenced");
    let (folder, _writing) = open_song(&mut state);
    for file in ["takes/a.wav", "takes/b.wav"] {
        assert_eq!(
            error(clean(&mut state, &["takes/e.wav", file])),
            format!("not an unused take: {file}")
        );
    }
    assert!(exists(&folder, "takes/e.wav"));
    assert!(exists(&folder, "takes/a.wav"));
}

#[test]
fn a_name_outside_takes_is_refused_and_nothing_is_deleted() {
    let (mut state, _audio, _dir) = state("clean-names");
    let (folder, _writing) = open_song(&mut state);
    let absolute = folder.join("takes/e.wav").display().to_string();
    for (file, shown) in [
        ("takes/../project.toml", "takes/../project.toml"),
        (absolute.as_str(), absolute.as_str()),
        ("takes/sub/e.wav", "takes/sub/e.wav"),
        ("e.wav", "e.wav"),
        ("takes/e\nx.wav", "takes/e\\nx.wav"),
        ("takes/e\u{7}.wav", "takes/e\\u{7}.wav"),
        ("takes/e\u{202e}vaw.wav", "takes/e\\u{202e}vaw.wav"),
        ("takes/e\u{2066}.wav", "takes/e\\u{2066}.wav"),
    ] {
        assert_eq!(
            error(clean(&mut state, &["takes/f.wav", file])),
            format!("invalid take file '{shown}'")
        );
    }
    assert!(exists(&folder, "project.toml"));
    assert!(exists(&folder, "takes/e.wav"));
    assert!(exists(&folder, "takes/f.wav"));
}

#[test]
fn a_symlink_placed_after_listing_is_refused_and_its_target_untouched() {
    let (mut state, _audio, dir) = state("clean-symlink");
    let (folder, _writing) = open_song(&mut state);
    assert_eq!(
        unused_files(&mut state),
        ["takes/d.wav", "takes/e.wav", "takes/f.wav"]
    );
    let outside = dir.0.join("outside.wav");
    write_take(&outside, 10);
    fs::remove_file(folder.join("takes/e.wav")).unwrap();
    std::os::unix::fs::symlink(&outside, folder.join("takes/e.wav")).unwrap();
    assert_eq!(
        error(clean(&mut state, &["takes/e.wav"])),
        "not an unused take: takes/e.wav"
    );
    assert!(outside.is_file());
    assert!(exists(&folder, "takes/e.wav"));
}

#[test]
fn takes_must_be_a_real_directory() {
    let (mut state, _audio, _dir) = state("clean-takes-symlink");
    let (folder, _writing) = open_song(&mut state);
    ok(handle(&mut state, Request::ItemCopy { ids: vec![1] }));
    ok(handle(&mut state, Request::ItemDelete { ids: vec![2] }));
    fs::write(folder.join(AUTOSAVE_FILE), "kept").unwrap();
    let real = folder.join("elsewhere");
    fs::rename(folder.join("takes"), &real).unwrap();
    std::os::unix::fs::symlink(&real, folder.join("takes")).unwrap();
    assert_eq!(
        error(handle(&mut state, Request::TakesUnused)),
        "takes is not a directory"
    );
    assert_eq!(
        error(clean(&mut state, &["takes/e.wav"])),
        "takes is not a directory"
    );
    assert!(real.join("e.wav").is_file());
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(
        (&status["undo"], &status["clipboard"]),
        (&json!("delete Item"), &json!(1))
    );
    assert_eq!(
        fs::read_to_string(folder.join(AUTOSAVE_FILE)).unwrap(),
        "kept"
    );
}

#[test]
fn a_project_without_a_takes_folder_has_no_unused_takes() {
    let (mut state, _audio, _dir) = state("no-takes-dir");
    let (folder, _writing) = open_song(&mut state);
    fs::remove_dir_all(folder.join("takes")).unwrap();
    assert_eq!(
        ok(handle(&mut state, Request::TakesUnused)),
        json!({"files": [], "bytes": 0})
    );
    assert_eq!(
        ok(clean(&mut state, &[])),
        json!({"deleted": 0, "bytes": 0, "failed": []})
    );
}

#[test]
fn a_failed_save_deletes_nothing_and_keeps_undo() {
    let (mut state, _audio, _dir) = state("clean-save-fails");
    let (folder, _writing) = open_song(&mut state);
    ok(handle(&mut state, Request::ItemDelete { ids: vec![2] }));
    fs::remove_file(folder.join("project.toml")).unwrap();
    fs::create_dir(folder.join("project.toml")).unwrap();
    fs::write(folder.join("project.toml/in-the-way"), "").unwrap();
    error(clean(&mut state, &["takes/c.wav", "takes/e.wav"]));
    assert!(exists(&folder, "takes/c.wav"));
    assert!(exists(&folder, "takes/e.wav"));
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["undo"], "delete Item");
}

#[test]
fn a_take_that_cannot_be_deleted_is_reported_after_the_save() {
    let (mut state, _audio, _dir) = state("clean-undeletable");
    let (folder, _writing) = open_song(&mut state);
    ok(handle(&mut state, Request::ItemDelete { ids: vec![2] }));
    let takes = folder.join("takes");
    fs::set_permissions(&takes, fs::Permissions::from_mode(0o555)).unwrap();
    let reply = clean(&mut state, &["takes/e.wav", "takes/f.wav"]);
    fs::set_permissions(&takes, fs::Permissions::from_mode(0o755)).unwrap();
    let denied = "Permission denied (os error 13)";
    assert_eq!(
        ok(reply),
        json!({"deleted": 0, "bytes": 0, "failed": [
            {"file": "takes/e.wav", "error": denied},
            {"file": "takes/f.wav", "error": denied},
        ]})
    );
    assert!(exists(&folder, "takes/e.wav"));
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(status["undo"], Value::Null);
}

#[test]
fn a_take_file_swapped_for_a_symlink_before_its_deletion_is_kept() {
    let (_state, _audio, dir) = state("delete-recheck");
    let takes = dir.0.join("takes");
    fs::create_dir(&takes).unwrap();
    write_take(&takes.join("x.wav"), 10);
    let outside = dir.0.join("outside.wav");
    write_take(&outside, 10);
    std::os::unix::fs::symlink(&outside, takes.join("y.wav")).unwrap();
    fs::create_dir(takes.join("z.wav")).unwrap();
    let x = fs::metadata(takes.join("x.wav")).unwrap().len();
    write_take(&dir.0.join("x.wav"), 10);
    let files = [
        "takes/x.wav",
        "takes/y.wav",
        "takes/z.wav",
        "y.wav",
        "takes/../x.wav",
    ]
    .map(str::to_string);
    assert_eq!(
        delete_take_files(&takes, &files),
        json!({"deleted": 1, "bytes": x, "failed": [
            {"file": "takes/y.wav", "error": "not a regular file"},
            {"file": "takes/z.wav", "error": "not a regular file"},
            {"file": "y.wav", "error": "not a take file"},
            {"file": "takes/../x.wav", "error": "not a take file"},
        ]})
    );
    assert!(!takes.join("x.wav").exists());
    assert!(dir.0.join("x.wav").is_file());
    assert!(
        fs::symlink_metadata(takes.join("y.wav"))
            .unwrap()
            .is_symlink()
    );
    assert!(outside.is_file());
}

#[test]
fn a_take_file_with_a_bidi_control_in_its_name_is_not_listed() {
    let (mut state, _audio, _dir) = state("bidi");
    let (folder, _writing) = open_song(&mut state);
    write_take(&folder.join("takes/x\u{202e}vaw.wav"), 10);
    write_take(&folder.join("takes/y\u{2069}.wav"), 10);
    assert_eq!(
        unused_files(&mut state),
        ["takes/d.wav", "takes/e.wav", "takes/f.wav"]
    );
}

#[test]
fn the_project_is_saved_before_its_takes_go() {
    let (mut state, _audio, _dir) = state("clean-saves");
    let (folder, _writing) = open_song(&mut state);
    fs::copy(folder.join("project.toml"), folder.join(AUTOSAVE_FILE)).unwrap();
    ok(handle(&mut state, Request::ItemDelete { ids: vec![2] }));
    ok(clean(&mut state, &["takes/c.wav"]));
    assert!(!exists(&folder, "takes/c.wav"));
    assert!(!exists(&folder, AUTOSAVE_FILE));
    let saved = Project::load(&folder).unwrap().0;
    let files: Vec<&str> = saved.tracks[0]
        .items
        .iter()
        .flat_map(|item| &item.takes)
        .map(|take| take.file.as_str())
        .collect();
    assert_eq!(files, ["takes/a.wav", "takes/b.wav"]);
}

#[test]
fn takes_clean_clears_undo_and_the_clipboard_and_forgets_deleted_audio() {
    let (mut state, _audio, _dir) = state("clean-history");
    let (_folder, _writing) = open_song(&mut state);
    ok(handle(&mut state, Request::ItemCopy { ids: vec![1] }));
    ok(handle(&mut state, Request::ItemDelete { ids: vec![2] }));
    let peaks = || Request::TakePeaks {
        file: "takes/c.wav".to_string(),
        first: None,
        count: None,
    };
    ok(handle(&mut state, peaks()));
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(
        (&status["undo"], &status["clipboard"]),
        (&json!("delete Item"), &json!(1))
    );
    ok(clean(&mut state, &["takes/c.wav"]));
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(
        (&status["undo"], &status["redo"], &status["clipboard"]),
        (&Value::Null, &Value::Null, &json!(0))
    );
    assert_eq!(
        error(handle(&mut state, peaks())),
        "no such take: takes/c.wav"
    );
}

#[test]
fn takes_clean_is_refused_while_recording() {
    let (mut state, _audio, _dir) = state("clean-recording");
    let (folder, _writing) = open_song(&mut state);
    record(&mut state);
    assert_eq!(
        error(clean(&mut state, &["takes/e.wav"])),
        "cannot clean takes while recording"
    );
    assert!(exists(&folder, "takes/e.wav"));
}
