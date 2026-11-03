use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

use xguitare_core::{Config, secure_socket_dir};

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
    assert_eq!(config.ports.input, "auto");
    assert_eq!(config.ports.output_left, "auto");
    assert_eq!(config.ports.output_right, "auto");
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
    assert_eq!(config.ports.input, "auto");
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
