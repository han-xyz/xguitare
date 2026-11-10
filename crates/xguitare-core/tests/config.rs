use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

use xguitare_core::{BankKind, Config, secure_socket_dir};

fn config_file(test: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xguitare-config-{}-{test}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn explicit_missing_path_is_error() {
    let path = std::env::temp_dir().join("xguitare-config-does-not-exist/config.toml");
    assert_eq!(
        Config::load(Some(&path)).unwrap_err().to_string(),
        format!("{}: not found", path.display())
    );
}

#[test]
fn unknown_key_is_error() {
    let path = config_file("unknown", "buffer_siz = 64\n");
    let message = Config::load(Some(&path)).unwrap_err().to_string();
    assert!(message.contains("buffer_siz"), "{message}");
    assert!(
        message.starts_with(&format!("{}: line 1:", path.display())),
        "{message}"
    );
}

#[test]
fn bad_toml_reports_position() {
    let path = config_file("bad", "buffer_size = 64\n[ports\n");
    let message = Config::load(Some(&path)).unwrap_err().to_string();
    assert!(
        message.starts_with(&format!("{}: line 2:", path.display())),
        "{message}"
    );
}

#[test]
fn partial_file_fills_defaults() {
    let path = config_file("partial", "buffer_size = 64\n");
    let config = Config::load(Some(&path)).unwrap();
    let defaults = Config::load(Some(&config_file("partial-empty", ""))).unwrap();
    assert_eq!(config.buffer_size, 64);
    assert_eq!(config.bank_dir, path.parent().unwrap().join("bank"));
    assert_eq!(config.socket, defaults.socket);
    assert_eq!(config.crossfade_ms, 20.0);
    assert_eq!(config.ports.input, "auto");
}

#[test]
fn bank_kind_defaults_to_guitar() {
    let config = Config::load(Some(&config_file("kind-default", ""))).unwrap();
    assert_eq!(config.bank_kind, BankKind::Guitar);
}

#[test]
fn bank_kind_bass_parses() {
    let path = config_file("kind-bass", "bank_kind = \"bass\"\n");
    assert_eq!(Config::load(Some(&path)).unwrap().bank_kind, BankKind::Bass);
}

#[test]
fn bank_kind_unknown_is_error() {
    let path = config_file("kind-unknown", "bank_kind = \"cello\"\n");
    let message = Config::load(Some(&path)).unwrap_err().to_string();
    assert_eq!(
        message,
        format!(
            "{}: line 1:13: bank_kind must be guitar or bass",
            path.display()
        )
    );
}

fn socket_dir(test: &str, mode: u32) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xguitare-sockdir-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
    dir
}

#[test]
fn socket_dir_ok_at_0700() {
    let dir = socket_dir("ok", 0o700);
    assert!(secure_socket_dir(&dir).is_ok());
}

#[test]
fn socket_dir_own_0755_is_tightened() {
    let dir = socket_dir("tighten", 0o755);
    assert!(secure_socket_dir(&dir).is_ok());
    assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
}

#[test]
fn socket_dir_refuses_symlink() {
    let target = socket_dir("target", 0o700);
    let link = target.with_file_name(format!("xguitare-sockdir-{}-link", std::process::id()));
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let message = secure_socket_dir(&link).unwrap_err().to_string();
    assert_eq!(
        message,
        format!(
            "refusing socket directory {}: it is a symlink",
            link.display()
        )
    );
}

#[test]
fn buffer_size_out_of_range() {
    for (test, size) in [("buffer-zero", 0), ("buffer-big", 8193)] {
        let path = config_file(test, &format!("buffer_size = {size}\n"));
        assert_eq!(
            Config::load(Some(&path)).unwrap_err().to_string(),
            format!("{}: buffer_size must be between 1 and 8192", path.display())
        );
    }
    let path = config_file("buffer-max", "buffer_size = 8192\n");
    assert_eq!(Config::load(Some(&path)).unwrap().buffer_size, 8192);
}

#[test]
fn crossfade_ms_and_ir_max_ms_validated() {
    for (key, max, above) in [
        ("crossfade_ms", 1000, "1000.5"),
        ("ir_max_ms", 10000, "10000.5"),
    ] {
        for value in ["-1.0", "0.0", "nan", "inf", above] {
            let path = config_file(&format!("{key}-{value}"), &format!("{key} = {value}\n"));
            assert_eq!(
                Config::load(Some(&path)).unwrap_err().to_string(),
                format!(
                    "{}: {key} must be above 0 and at most {max}",
                    path.display()
                ),
                "{value}"
            );
        }
        let path = config_file(&format!("{key}-max"), &format!("{key} = {max}.0\n"));
        assert!(Config::load(Some(&path)).is_ok(), "{key} = {max}");
    }
}

#[test]
fn tuner_keys_parse() {
    let path = config_file("tuner", "tuner_reference_hz = 442.0\ntuner_mute = false\n");
    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.tuner_reference_hz, 442.0);
    assert!(!config.tuner_mute);
    for (test, edge) in [("tuner-low-edge", 400.0), ("tuner-high-edge", 480.0)] {
        let path = config_file(test, &format!("tuner_reference_hz = {edge:.1}\n"));
        assert_eq!(Config::load(Some(&path)).unwrap().tuner_reference_hz, edge);
    }
}

#[test]
fn tuner_reference_out_of_range() {
    for (test, hz) in [("tuner-low", "399.0"), ("tuner-high", "481.0")] {
        let path = config_file(test, &format!("tuner_reference_hz = {hz}\n"));
        assert_eq!(
            Config::load(Some(&path)).unwrap_err().to_string(),
            format!(
                "{}: tuner_reference_hz must be between 400 and 480",
                path.display()
            )
        );
    }
}

#[test]
fn ir_dir_defaults_to_bank_dir() {
    let config = Config::load(Some(&config_file("ir-dir-default", ""))).unwrap();
    assert_eq!(config.ir_dir, config.bank_dir);
    let path = config_file("ir-dir-bank", "bank_dir = \"/srv/xg/bank\"\n");
    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.ir_dir, PathBuf::from("/srv/xg/bank"));
}

#[test]
fn ir_dir_parses() {
    let path = config_file(
        "ir-dir",
        "bank_dir = \"/srv/xg/bank\"\nir_dir = \"/srv/xg/irs\"\n",
    );
    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.ir_dir, PathBuf::from("/srv/xg/irs"));
    assert_eq!(config.bank_dir, PathBuf::from("/srv/xg/bank"));
}

#[test]
fn bass_without_state_keeps_bank_dir() {
    let path = config_file("kind-dir", "bank_kind = \"bass\"\n");
    let config = Config::load(Some(&path)).unwrap();
    let dir = path.parent().unwrap();
    assert_eq!(config.bank_dir, dir.join("bank"));
    assert_eq!(config.ir_dir, dir.join("bank"));
}

#[test]
fn state_toml_overrides_config() {
    let path = config_file(
        "state-overrides",
        "bank_kind = \"guitar\"\nir_dir = \"/srv/xg/irs\"\n[ports]\ninput = \"a*\"\noutput_left = \"b*\"\n",
    );
    let dir = path.parent().unwrap();
    fs::write(
        dir.join("state.toml"),
        "bank_kind = \"bass\"\nir_dir = \"/srv/xg/state-irs\"\n[banks]\nguitar = \"/srv/xg/guitar\"\nbass = \"/srv/xg/bass\"\n[ports]\ninput = \"card:capture_1\"\noutput_left = \"card:playback_1\"\noutput_right = \"card:playback_2\"\n",
    )
    .unwrap();
    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.state_warning, None);
    assert_eq!(config.bank_kind, BankKind::Bass);
    assert_eq!(config.bank_dir, PathBuf::from("/srv/xg/bass"));
    assert_eq!(config.ir_dir, PathBuf::from("/srv/xg/state-irs"));
    assert_eq!(config.ports.input, "card:capture_1");
    assert_eq!(config.ports.output_left, "card:playback_1");
    assert_eq!(config.ports.output_right, "card:playback_2");
    assert_eq!(config.state_file, dir.join("state.toml"));
}

#[test]
fn state_toml_names_projects_dir() {
    let path = config_file("state-projects-dir", "");
    fs::write(
        path.parent().unwrap().join("state.toml"),
        "projects_dir = \"/srv/xg/projects\"\n",
    )
    .unwrap();
    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.state_warning, None);
    assert_eq!(config.projects_dir, PathBuf::from("/srv/xg/projects"));
}

#[test]
fn state_toml_keeps_config_bank_dir() {
    let path = config_file("state-bank-dir", "bank_dir = \"/srv/xg/bank\"\n");
    fs::write(
        path.parent().unwrap().join("state.toml"),
        "bank_kind = \"bass\"\n",
    )
    .unwrap();
    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.bank_dir, PathBuf::from("/srv/xg/bank"));
    assert_eq!(config.ir_dir, PathBuf::from("/srv/xg/bank"));
    assert_eq!(config.bank_kind, BankKind::Guitar);
}

#[test]
fn bad_state_toml_is_ignored_with_warning() {
    let path = config_file("state-bad", "buffer_size = 64\n");
    let state = path.parent().unwrap().join("state.toml");
    fs::write(&state, "bank_kind = \"bass\"\n[ports\n").unwrap();
    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.buffer_size, 64);
    assert_eq!(config.bank_kind, BankKind::Guitar);
    let warning = config.state_warning.unwrap();
    assert!(
        warning.starts_with(&format!("{}: line 2:", state.display())),
        "{warning}"
    );
}
