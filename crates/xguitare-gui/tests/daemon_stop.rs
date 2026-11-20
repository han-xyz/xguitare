use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use xguitare_client::ClientConfig;
use xguitare_gui::client;
use xguitare_gui::model::Command;

const WAIT: Duration = Duration::from_secs(5);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait_for_file(path: &Path) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn quit_stops_a_daemon_the_gui_started() {
    let dir = TempDir(PathBuf::from(format!(
        "/tmp/xguitare-gd-{}",
        std::process::id()
    )));
    let _ = fs::remove_dir_all(&dir.0);
    fs::create_dir_all(&dir.0).unwrap();
    let started = dir.0.join("started");
    let stopped = dir.0.join("stopped");
    let fake_daemon = dir.0.join("xguitare");
    fs::write(
        &fake_daemon,
        format!(
            "#!/bin/sh\ntrap 'touch {}; exit 0' TERM\ntouch {}\ni=0\nwhile [ $i -lt 500 ]; do sleep 0.02; i=$((i + 1)); done\n",
            stopped.display(),
            started.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_daemon, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        dir.0.display(),
        env::var("PATH").unwrap_or_default()
    );
    // SAFETY: this test binary holds this one test, so no other thread reads the environment yet.
    unsafe { env::set_var("PATH", path) };
    let config_path = dir.0.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let config = ClientConfig {
        socket: dir.0.join("s.sock"),
        socket_is_fallback: false,
        ..ClientConfig::load(Some(&config_path)).unwrap()
    };

    let (commands, _snapshots, handle) =
        client::spawn(config, None, Arc::new(AtomicBool::new(false)), || {});
    assert!(wait_for_file(&started), "GUI did not start the Daemon");
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();

    assert!(stopped.exists(), "Daemon not stopped on quit");
}
