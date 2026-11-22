use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use xguitare_core::control::{EngineState, handle};
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues};
use xguitare_core::firstrun::ensure_bank;
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, Config};
use xguitare_protocol::{BankKind, Request, Response};

const AMP_PRESETS: [&str; 6] = [
    "16-amp-tweed.toml",
    "17-amp-american.toml",
    "18-amp-chime.toml",
    "19-amp-crunch.toml",
    "20-amp-lead.toml",
    "21-amp-modern.toml",
];
const AMP_IRS: [&str; 4] = [
    "cab-4x10.wav",
    "cab-2x12-open.wav",
    "cab-2x12-alnico.wav",
    "cab-4x12.wav",
];

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ReadOnly(PathBuf);

impl Drop for ReadOnly {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

/// A state whose Bank is a fresh starter Bank of the config's kind; `config_toml` may use `{root}` for the temp dir.
fn state(test: &str, config_toml: &str) -> (EngineState, AudioQueues, TempDir) {
    let root =
        std::env::temp_dir().join(format!("xguitare-starters-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config_path = root.join("config.toml");
    fs::write(
        &config_path,
        config_toml.replace("{root}", &root.display().to_string()),
    )
    .unwrap();
    let config = Config::load(Some(&config_path)).unwrap();
    ensure_bank(&config.bank_dir, config.bank_kind, 48_000).unwrap();
    let bank = Bank::load_dir(&config.bank_dir).unwrap();
    let (queues, audio, tap) = engine_queues(64);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state = EngineState::new(
        bank,
        Arc::new(EngineShared::default()),
        queues,
        tuner,
        config,
        48_000.0,
        128,
    );
    (state, audio, TempDir(root))
}

/// The guitar Bank as first run wrote it before the amp Presets existed: 15 Presets and `default-cab.wav`.
fn old_state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let (mut state, audio, dir) = state(test, "");
    for file in AMP_PRESETS.iter().chain(&AMP_IRS) {
        fs::remove_file(state.config.bank_dir.join(file)).unwrap();
    }
    state.bank = Bank::load_dir(&state.config.bank_dir).unwrap();
    (state, audio, dir)
}

fn starters(state: &mut EngineState) -> Response {
    handle(state, Request::BankStarters).0
}

fn ok(response: Response) -> Value {
    match response {
        Response::Ok(payload) => payload,
        Response::Err { error, .. } => panic!("expected ok, got error {error}"),
    }
}

fn error(response: Response) -> String {
    match response {
        Response::Err { error, .. } => error,
        Response::Ok(payload) => panic!("expected error, got {payload}"),
    }
}

fn files(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    files
}

fn gain_preset(name: &str) -> String {
    format!("name = \"{name}\"\n[[module]]\ntype = \"gain\"\n[module.params]\nlevel = -3.5\n")
}

#[test]
fn old_starter_bank_gets_the_amp_presets_and_their_irs() {
    let (mut state, _audio, _dir) = old_state("old");
    let fresh = TempDir(state.config.bank_dir.with_file_name("fresh"));
    ensure_bank(&fresh.0, BankKind::Guitar, 48_000).unwrap();
    let revision = state.revision;
    assert_eq!(state.bank.entries.len(), 15);
    assert_eq!(
        ok(starters(&mut state)),
        json!({"presets": AMP_PRESETS, "irs": AMP_IRS})
    );
    let bank_dir = &state.config.bank_dir;
    assert_eq!(files(bank_dir), files(&fresh.0));
    for file in AMP_PRESETS.iter().chain(&AMP_IRS) {
        assert_eq!(
            fs::read(bank_dir.join(file)).unwrap(),
            fs::read(fresh.0.join(file)).unwrap(),
            "{file}"
        );
    }
    assert_eq!(state.bank.entries.len(), 21);
    assert!(state.revision > revision);
}

#[test]
fn second_run_adds_nothing() {
    let (mut state, _audio, _dir) = old_state("again");
    ok(starters(&mut state));
    assert_eq!(ok(starters(&mut state)), json!({"presets": [], "irs": []}));
}

#[test]
fn a_name_in_other_case_counts_as_the_starter() {
    let (mut state, _audio, _dir) = old_state("case");
    let bank_dir = state.config.bank_dir.clone();
    fs::write(bank_dir.join("16-my-tweed.toml"), gain_preset("amp TWEED")).unwrap();
    assert_eq!(
        ok(starters(&mut state))["presets"],
        json!([
            "17-amp-american.toml",
            "18-amp-chime.toml",
            "19-amp-crunch.toml",
            "20-amp-lead.toml",
            "21-amp-modern.toml"
        ])
    );
}

#[test]
fn a_user_preset_keeps_its_file_and_number() {
    let (mut state, _audio, _dir) = old_state("mine");
    let mine = state.config.bank_dir.join("16-mine.toml");
    fs::write(&mine, gain_preset("Mine")).unwrap();
    assert_eq!(
        ok(starters(&mut state))["presets"],
        json!([
            "17-amp-tweed.toml",
            "18-amp-american.toml",
            "19-amp-chime.toml",
            "20-amp-crunch.toml",
            "21-amp-lead.toml",
            "22-amp-modern.toml"
        ])
    );
    assert_eq!(fs::read_to_string(&mine).unwrap(), gain_preset("Mine"));
}

#[test]
fn numbering_starts_after_the_highest_file_even_one_that_does_not_parse() {
    let (mut state, _audio, _dir) = old_state("highest");
    let broken = state.config.bank_dir.join("30-broken.toml");
    fs::write(&broken, "not a preset").unwrap();
    assert_eq!(
        ok(starters(&mut state))["presets"][0],
        json!("31-amp-tweed.toml")
    );
    assert_eq!(fs::read_to_string(&broken).unwrap(), "not a preset");
}

#[test]
fn a_deleted_starter_comes_back() {
    let (mut state, _audio, _dir) = state("deleted", "");
    fs::remove_file(state.config.bank_dir.join("03-lead.toml")).unwrap();
    assert_eq!(
        ok(starters(&mut state)),
        json!({"presets": ["22-lead.toml"], "irs": []})
    );
}

#[test]
fn a_file_that_does_not_parse_is_left_alone_and_names_nothing() {
    let (mut state, _audio, _dir) = state("broken", "");
    let broken = state.config.bank_dir.join("16-amp-tweed.toml");
    let text = "name = \"Amp Tweed\"\n[[module]]\ntype = \"nonsense\"\n";
    fs::write(&broken, text).unwrap();
    assert_eq!(
        ok(starters(&mut state)),
        json!({"presets": ["22-amp-tweed.toml"], "irs": []})
    );
    assert_eq!(fs::read_to_string(&broken).unwrap(), text);
}

#[test]
fn an_ir_in_the_ir_dir_is_not_copied_into_the_bank() {
    let (mut state, _audio, dir) = state("irdir", "ir_dir = \"{root}/irs\"\n");
    let irs = dir.0.join("irs");
    fs::create_dir_all(&irs).unwrap();
    let bank_dir = state.config.bank_dir.clone();
    fs::rename(bank_dir.join("cab-4x10.wav"), irs.join("cab-4x10.wav")).unwrap();
    assert_eq!(ok(starters(&mut state)), json!({"presets": [], "irs": []}));
    assert!(!bank_dir.join("cab-4x10.wav").exists());
}

#[test]
fn a_failed_ir_write_keeps_what_came_before_and_a_later_run_adds_the_rest() {
    let (mut state, _audio, _dir) = old_state("squat");
    let bank_dir = state.config.bank_dir.clone();
    let squat = bank_dir.join("cab-4x12.wav");
    fs::create_dir(&squat).unwrap();
    assert_eq!(
        error(starters(&mut state)),
        format!(
            "cannot write {}: File exists (os error 17)",
            squat.display()
        )
    );
    let left = files(&bank_dir);
    for file in AMP_PRESETS.iter().chain(&AMP_IRS[..3]) {
        assert!(left.contains(&file.to_string()), "{file} in {left:?}");
    }
    assert!(
        left.iter()
            .all(|file| Path::new(file).extension() != Some("tmp".as_ref())),
        "{left:?}"
    );
    assert_eq!(state.bank.entries.len(), 21);
    fs::remove_dir(&squat).unwrap();
    assert_eq!(
        ok(starters(&mut state)),
        json!({"presets": [], "irs": ["cab-4x12.wav"]})
    );
}

#[test]
fn a_failed_preset_write_leaves_no_file() {
    let (mut state, _audio, _dir) = old_state("readonly");
    let bank_dir = state.config.bank_dir.clone();
    let before = files(&bank_dir);
    fs::set_permissions(&bank_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let _restore = ReadOnly(bank_dir.clone());
    assert_eq!(
        error(starters(&mut state)),
        format!(
            "cannot write {}: Permission denied (os error 13)",
            bank_dir.join("16-amp-tweed.toml").display()
        )
    );
    assert_eq!(files(&bank_dir), before);
}

#[test]
fn bass_kind_decides_the_set_in_a_custom_bank_dir() {
    let (mut state, _audio, dir) =
        state("bass", "bank_kind = \"bass\"\nbank_dir = \"{root}/mine\"\n");
    let bank_dir = dir.0.join("mine");
    for file in [
        "16-amp-bass-valve.toml",
        "17-amp-bass-solid.toml",
        "bass-cab-8x10.wav",
    ] {
        fs::remove_file(bank_dir.join(file)).unwrap();
    }
    assert_eq!(
        ok(starters(&mut state)),
        json!({
            "presets": ["16-amp-bass-valve.toml", "17-amp-bass-solid.toml"],
            "irs": ["bass-cab-8x10.wav"]
        })
    );
}
