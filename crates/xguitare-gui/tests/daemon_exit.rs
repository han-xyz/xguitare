use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use xguitare_client::ClientConfig;
use xguitare_gui::client;
use xguitare_gui::model::{Command, Link, SPAWN_BACKOFF};

const WAIT: Duration = Duration::from_secs(5);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_daemon_that_exits_at_once_is_shown_and_not_started_again() {
    let dir = TempDir(PathBuf::from(format!(
        "/tmp/xguitare-ge-{}",
        std::process::id()
    )));
    let _ = fs::remove_dir_all(&dir.0);
    fs::create_dir_all(&dir.0).unwrap();
    let starts = dir.0.join("starts");
    let fake_daemon = dir.0.join("xguitare");
    fs::write(
        &fake_daemon,
        format!(
            "#!/bin/sh\necho started >> {}\necho 'loading config' >&2\nprintf '\\033[31merror\\033[0m: bad buffer_size\\n\\n' >&2\nexit 1\n",
            starts.display()
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

    let (commands, snapshots, handle) =
        client::spawn(config, None, Arc::new(AtomicBool::new(false)), || {});
    let expected = Link::Exited {
        status: "exit status: 1".to_string(),
        last_line: Some("\u{1b}[31merror\u{1b}[0m: bad buffer_size".to_string()),
    };
    let deadline = Instant::now() + WAIT;
    let mut link = None;
    while Instant::now() < deadline && link.as_ref() != Some(&expected) {
        link = snapshots
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
            .map(|snapshot| snapshot.link);
    }
    assert_eq!(link, Some(expected.clone()));

    let after_backoff = Instant::now() + SPAWN_BACKOFF + Duration::from_secs(1);
    while let Some(left) = after_backoff.checked_duration_since(Instant::now()) {
        if let Ok(snapshot) = snapshots.recv_timeout(left) {
            assert_eq!(
                snapshot.link, expected,
                "the banner left before a connection"
            );
        }
    }
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();

    assert_eq!(fs::read_to_string(&starts).unwrap(), "started\n");
}
