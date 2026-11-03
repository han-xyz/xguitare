use std::fs;
use std::path::PathBuf;

use xguitare_core::Config;

fn config_file(test: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xguitare-config-{}-{test}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn defaults_when_missing() {
    let path = std::env::temp_dir().join("xguitare-config-does-not-exist/config.toml");
    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.buffer_size, 128);
    assert_eq!(config.crossfade_ms, 20.0);
    assert_eq!(config.ir_max_ms, 100.0);
    assert_eq!(config.ports.input, "system:capture_1");
    assert_eq!(config.ports.output_left, "system:playback_1");
    assert_eq!(config.ports.output_right, "system:playback_2");
    assert!(config.bank_dir.ends_with("xguitare/bank"));
    assert!(config.socket.ends_with("xguitare.sock"));
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
    let defaults = Config::load(Some(&path.with_file_name("absent.toml"))).unwrap();
    assert_eq!(config.buffer_size, 64);
    assert_eq!(config.bank_dir, defaults.bank_dir);
    assert_eq!(config.socket, defaults.socket);
    assert_eq!(config.crossfade_ms, 20.0);
    assert_eq!(config.ports.input, "system:capture_1");
}
