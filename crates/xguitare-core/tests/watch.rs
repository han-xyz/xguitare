use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use xguitare_core::control::{EngineState, PeakMeters, Request, Response, handle};
use xguitare_core::engine::{AudioQueues, Command, EngineShared, engine_queues};
use xguitare_core::tuner::Tuner;
use xguitare_core::watch::{Action, FileStamp, apply, changed_paths};
use xguitare_core::{Bank, BankEntry, Config};

const QUEUE_CAPACITY: usize = 64;
const CLEAN: &str = "name = \"Clean\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = -3\n";

fn state(test: &str) -> (EngineState, AudioQueues) {
    let dir = std::env::temp_dir().join(format!("xguitare-watch-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("01-clean.toml"), CLEAN).unwrap();
    fs::write(
        dir.join("02-loud.toml"),
        "name = \"Loud\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = 6\n",
    )
    .unwrap();
    let bank = Bank::load_dir(&dir).unwrap();
    let BankEntry::Valid { file, preset } = &bank.entries[0] else {
        panic!("01-clean.toml should be valid");
    };
    let active = Some((file.clone(), preset.clone()));
    let mut config = empty_config(&dir);
    config.bank_dir = dir;
    let (queues, audio, tap) = engine_queues(QUEUE_CAPACITY);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state = EngineState {
        bank,
        active,
        shared: Arc::new(EngineShared::default()),
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
    };
    (state, audio)
}

fn active_file(state: &EngineState) -> PathBuf {
    state.config.bank_dir.join("01-clean.toml")
}

fn active_level(state: &EngineState) -> f32 {
    state.active.as_ref().unwrap().1.modules[0].params[0].1
}

#[test]
#[allow(clippy::float_cmp, reason = "the test sets this exact value")]
fn active_file_changed_reloads() {
    let (mut state, mut audio) = state("reload");
    let file = active_file(&state);
    fs::write(&file, CLEAN.replace("level = -3", "level = 2")).unwrap();
    let actions = apply(&mut state, &[file]);
    assert_eq!(actions, [Action::Reindex, Action::Reload { program: 0 }]);
    assert!(matches!(
        audio.commands.pop(),
        Ok(Command::SwapRack(rack)) if rack.modules()[0].1.param(0) == 2.0
    ));
    assert_eq!(active_level(&state), 2.0);
}

#[test]
fn reload_build_warning_is_an_action() {
    let (mut state, _audio) = state("reload-warning");
    let file = active_file(&state);
    fs::write(
        &file,
        "name = \"Clean\"\n[[module]]\ntype = \"cabinet\"\nir = \"missing.wav\"\n",
    )
    .unwrap();
    let actions = apply(&mut state, &[file]);
    assert!(
        matches!(&actions[..], [Action::Reindex, Action::Warn(warning), Action::Reload { program: 0 }] if warning.starts_with("cabinet 'cabinet': ")),
        "{actions:?}"
    );
}

#[test]
fn unchanged_content_is_nothing() {
    let (mut state, mut audio) = state("unchanged");
    let set = Request::Set {
        param: "gain.level".to_string(),
        value: -6.0,
    };
    assert!(matches!(handle(&mut state, set).0, Response::Ok(_)));
    assert!(matches!(
        handle(&mut state, Request::Save { name: None }).0,
        Response::Ok(_)
    ));
    assert!(matches!(audio.commands.pop(), Ok(Command::SetParam { .. })));
    let file = active_file(&state);
    let actions = apply(&mut state, &[file]);
    assert_eq!(actions, [Action::Reindex, Action::Nothing]);
    assert!(audio.commands.pop().is_err());
    assert_eq!(active_level(&state), -6.0);
}

#[test]
fn removed_active_file_keeps_rack_and_clears_program() {
    let (mut state, mut audio) = state("removed");
    let file = active_file(&state);
    let loud = state.config.bank_dir.join("02-loud.toml");
    let loud_text = fs::read_to_string(&loud).unwrap();
    fs::remove_file(&file).unwrap();
    assert_eq!(
        apply(&mut state, std::slice::from_ref(&file)),
        [
            Action::Reindex,
            Action::Warn(format!(
                "active preset file {} removed; still running",
                file.display()
            ))
        ]
    );
    assert!(audio.commands.pop().is_err());
    assert_eq!(active_level(&state), -3.0);
    let (Response::Ok(status), _) = handle(&mut state, Request::Status) else {
        panic!("status failed");
    };
    assert!(status["program"].is_null(), "{status}");
    assert!(matches!(
        handle(&mut state, Request::Save { name: None }).0,
        Response::Err { .. }
    ));
    assert_eq!(fs::read_to_string(&loud).unwrap(), loud_text);
    fs::write(&loud, loud_text.replace("level = 6", "level = 1")).unwrap();
    assert_eq!(apply(&mut state, &[loud]), [Action::Reindex]);
    assert!(audio.commands.pop().is_err());
}

#[test]
fn engine_busy_retries_next_tick() {
    let (mut state, mut audio) = state("busy");
    let set = || Command::SetParam {
        module: 0,
        param: 0,
        value: 0.0,
    };
    while state.queues.commands.push(set()).is_ok() {}
    let file = active_file(&state);
    fs::write(&file, CLEAN.replace("level = -3", "level = 2")).unwrap();
    assert_eq!(
        apply(&mut state, std::slice::from_ref(&file)),
        [Action::Reindex, Action::EngineBusy { file: file.clone() }]
    );
    while audio.commands.pop().is_ok() {}
    assert_eq!(
        apply(&mut state, &[file]),
        [Action::Reindex, Action::Reload { program: 0 }]
    );
    assert_eq!(active_level(&state), 2.0);
}

#[test]
fn other_file_added_reindexes() {
    let (mut state, mut audio) = state("added");
    let file = state.config.bank_dir.join("03-x.toml");
    fs::write(&file, CLEAN.replace("Clean", "X")).unwrap();
    let actions = apply(&mut state, &[file]);
    assert_eq!(actions, [Action::Reindex]);
    assert_eq!(state.bank.entries.len(), 3);
    assert!(audio.commands.pop().is_err());
}

#[test]
fn non_toml_ignored() {
    let (mut state, mut audio) = state("non-toml");
    let file = state.config.bank_dir.join("notes.txt");
    fs::write(&file, "not a preset").unwrap();
    let toml = state.config.bank_dir.join("04-late.toml");
    fs::write(&toml, CLEAN).unwrap();
    assert_eq!(apply(&mut state, &[file]), []);
    assert_eq!(state.bank.entries.len(), 2);
    assert!(audio.commands.pop().is_err());
}

#[test]
fn broken_active_file_logs_and_keeps() {
    let (mut state, mut audio) = state("broken");
    let file = active_file(&state);
    let before = state.active.clone();
    fs::write(&file, "name = \n").unwrap();
    let actions = apply(&mut state, std::slice::from_ref(&file));
    assert_eq!(actions.len(), 2, "{actions:?}");
    assert_eq!(actions[0], Action::Reindex);
    assert!(
        matches!(&actions[1], Action::LogAndKeep { file: logged, error } if *logged == file && error.starts_with("line 1:")),
        "{actions:?}"
    );
    assert!(audio.commands.pop().is_err());
    assert_eq!(state.active, before);
}

fn stamp(secs: u64, len: u64) -> FileStamp {
    (SystemTime::UNIX_EPOCH + Duration::from_secs(secs), len)
}

#[test]
fn unchanged_stat_is_filtered_out() {
    let file = PathBuf::from("/bank/01-clean.toml");
    let snapshot = HashMap::from([(file.clone(), stamp(100, 80))]);
    assert_eq!(
        changed_paths(&snapshot, &snapshot.clone(), &[file]),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn changed_mtime_passes() {
    let file = PathBuf::from("/bank/01-clean.toml");
    let added = PathBuf::from("/bank/03-x.toml");
    let previous = HashMap::from([(file.clone(), stamp(100, 80))]);
    let current = HashMap::from([
        (file.clone(), stamp(101, 80)),
        (added.clone(), stamp(5, 10)),
    ]);
    assert_eq!(
        changed_paths(&previous, &current, &[file.clone(), added.clone()]),
        [file, added]
    );
}

#[test]
fn reindex_and_reload_bump_revision() {
    let (mut state, _audio) = state("revision");
    let other = state.config.bank_dir.join("03-x.toml");
    fs::write(&other, CLEAN.replace("Clean", "X")).unwrap();
    assert_eq!(apply(&mut state, &[other]), [Action::Reindex]);
    assert_eq!(state.revision, 2);
    let file = active_file(&state);
    fs::write(&file, CLEAN.replace("level = -3", "level = 2")).unwrap();
    assert_eq!(
        apply(&mut state, &[file]),
        [Action::Reindex, Action::Reload { program: 0 }]
    );
    assert_eq!(state.revision, 4);
}

fn module_ids(state: &EngineState) -> Vec<String> {
    let (_, preset) = state.active.as_ref().unwrap();
    preset
        .modules
        .iter()
        .map(|module| module.id.clone())
        .collect()
}

#[test]
fn own_save_keeps_later_live_change() {
    for (test, name) in [
        ("own-save", None),
        ("own-save-as", Some("Saved".to_string())),
    ] {
        let (mut state, _audio) = state(test);
        let add = Request::RackAdd {
            r#type: "gate".to_string(),
            after: None,
            at: Some(0),
        };
        assert!(matches!(handle(&mut state, add).0, Response::Ok(_)));
        assert!(matches!(
            handle(&mut state, Request::Save { name }).0,
            Response::Ok(_)
        ));
        let saved = state.active.as_ref().unwrap().0.clone();
        let rack_move = Request::RackMove {
            id: "gate".to_string(),
            to: 1,
        };
        assert!(matches!(handle(&mut state, rack_move).0, Response::Ok(_)));
        assert_eq!(
            apply(&mut state, std::slice::from_ref(&saved)),
            [Action::Reindex, Action::Nothing]
        );
        assert_eq!(module_ids(&state), ["gain", "gate"], "{test}");
        assert!(state.modified, "{test}");
        let text = fs::read_to_string(&saved).unwrap();
        fs::write(&saved, text).unwrap();
        assert!(
            matches!(
                &apply(&mut state, &[saved])[..],
                [Action::Reindex, Action::Reload { .. }]
            ),
            "{test}"
        );
        assert_eq!(module_ids(&state), ["gate", "gain"], "{test}");
    }
}

#[test]
fn outside_edit_after_save_reloads() {
    let (mut state, _audio) = state("outside-after-save");
    assert!(matches!(
        handle(&mut state, Request::Save { name: None }).0,
        Response::Ok(_)
    ));
    let file = active_file(&state);
    fs::write(&file, CLEAN.replace("level = -3", "level = 2")).unwrap();
    assert_eq!(
        apply(&mut state, &[file]),
        [Action::Reindex, Action::Reload { program: 0 }]
    );
    assert_eq!(active_level(&state), 2.0);
}

#[test]
fn record_of_inactive_file_does_not_hide_outside_edit() {
    let (mut state, _audio) = state("stale-record");
    let file = active_file(&state);
    let load = |preset: &str| Request::Load {
        preset: preset.to_string(),
    };
    let set = Request::Set {
        param: "gain.level".to_string(),
        value: -6.0,
    };
    assert!(matches!(handle(&mut state, set).0, Response::Ok(_)));
    assert!(matches!(
        handle(&mut state, Request::Save { name: None }).0,
        Response::Ok(_)
    ));
    let saved_text = fs::read_to_string(&file).unwrap();
    assert!(matches!(
        handle(&mut state, load("Loud")).0,
        Response::Ok(_)
    ));
    assert_eq!(
        apply(&mut state, std::slice::from_ref(&file)),
        [Action::Reindex]
    );
    fs::write(&file, CLEAN.replace("level = -3", "level = 2")).unwrap();
    assert_eq!(
        apply(&mut state, std::slice::from_ref(&file)),
        [Action::Reindex]
    );
    assert!(matches!(
        handle(&mut state, load("Clean")).0,
        Response::Ok(_)
    ));
    fs::write(&file, saved_text).unwrap();
    assert_eq!(
        apply(&mut state, &[file]),
        [Action::Reindex, Action::Reload { program: 0 }]
    );
    assert_eq!(active_level(&state), -6.0);
}

fn empty_config(dir: &Path) -> Config {
    let path = dir.join("config");
    fs::write(&path, "").unwrap();
    Config::load(Some(&path)).unwrap()
}
