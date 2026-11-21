use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use xguitare_client::ClientConfig;
use xguitare_core::Config;
use xguitare_protocol::BankKind;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn agreed(path: Option<&Path>, label: &str) -> ClientConfig {
    let daemon = Config::load(path).unwrap();
    let client = ClientConfig::load(path).unwrap();
    assert_eq!(client.socket, daemon.socket, "{label}");
    assert_eq!(
        client.socket_is_fallback, daemon.socket_is_fallback,
        "{label}"
    );
    assert_eq!(client.tuner_mute, daemon.tuner_mute, "{label}");
    assert_eq!(client.bank_kind, daemon.bank_kind, "{label}");
    client
}

// One test only: it sets process environment, and a second test in this file would race it.
#[test]
fn client_and_daemon_find_the_same_socket_for_every_environment() {
    let root = TempDir(PathBuf::from(format!(
        "/tmp/xguitare-cse-{}",
        std::process::id()
    )));
    let _ = fs::remove_dir_all(&root.0);
    let home = root.0.join("home");
    let config_home = root.0.join("config");
    let runtime = root.0.join("runtime");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(config_home.join("xguitare")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", &config_home);
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    }
    let uid = fs::metadata("/proc/self").unwrap().uid();
    let fallback = PathBuf::from(format!("/tmp/xguitare-{uid}/xguitare.sock"));
    let under_runtime = runtime.join("xguitare/xguitare.sock");
    let write = |name: &str, text: &str| {
        let path = root.0.join(name);
        fs::write(&path, text).unwrap();
        path
    };

    let missing_default = agreed(None, "missing default file, runtime dir set");
    assert_eq!(missing_default.socket, under_runtime);
    assert!(!missing_default.socket_is_fallback);
    assert!(missing_default.tuner_mute);
    assert_eq!(missing_default.bank_kind, BankKind::Guitar);

    for (name, text) in [
        ("empty", ""),
        ("comments", "# nothing\n  # here\n"),
        (
            "daemon-keys",
            "buffer_size = 64\ncrossfade_ms = 10\nir_max_ms = 50\ntuner_reference_hz = 442.0\n",
        ),
        ("ports", "[ports]\ninput = \"\"\noutput_left = \"x*\"\n"),
    ] {
        let path = write(&format!("{name}.toml"), text);
        assert_eq!(agreed(Some(&path), name).socket, under_runtime, "{name}");
    }

    fs::write(
        config_home.join("xguitare/config.toml"),
        "socket = \"/tmp/xguitare-cse-default-file.sock\"\ntuner_mute = false\n",
    )
    .unwrap();
    let default_file = agreed(None, "default file present");
    assert_eq!(
        default_file.socket,
        PathBuf::from("/tmp/xguitare-cse-default-file.sock")
    );
    assert!(!default_file.tuner_mute);
    fs::remove_file(config_home.join("xguitare/config.toml")).unwrap();

    for (label, socket) in [
        ("absolute", "/tmp/xguitare-cse-abs/x.sock"),
        ("relative", "rel/x.sock"),
    ] {
        let path = write("explicit.toml", &format!("socket = \"{socket}\"\n"));
        let config = agreed(Some(&path), label);
        assert_eq!(config.socket, PathBuf::from(socket), "{label}");
        assert!(!config.socket_is_fallback, "{label}");
    }

    unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    let empty_file = write("fallback.toml", "");
    let unset = agreed(Some(&empty_file), "runtime dir unset");
    assert_eq!(unset.socket, fallback);
    assert!(unset.socket_is_fallback);
    let path = write(
        "explicit.toml",
        "socket = \"/tmp/xguitare-cse-abs/x.sock\"\n",
    );
    let config = agreed(Some(&path), "explicit socket, runtime dir unset");
    assert_eq!(config.socket, PathBuf::from("/tmp/xguitare-cse-abs/x.sock"));
    assert!(!config.socket_is_fallback);

    unsafe { std::env::set_var("XDG_RUNTIME_DIR", "") };
    let empty_runtime = agreed(Some(&empty_file), "runtime dir empty");
    assert_eq!(empty_runtime.socket, fallback);
    assert!(empty_runtime.socket_is_fallback);

    unsafe { std::env::set_var("XDG_RUNTIME_DIR", "relative/run") };
    let relative_runtime = agreed(Some(&empty_file), "runtime dir relative");
    assert_eq!(relative_runtime.socket, fallback);
    assert!(relative_runtime.socket_is_fallback);
}
