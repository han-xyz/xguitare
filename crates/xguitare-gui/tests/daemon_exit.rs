use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use xguitare_client::ClientConfig;
use xguitare_gui::client;
use xguitare_gui::model::{Command, Link, respawn_delay};

const WAIT: Duration = Duration::from_secs(5);
const STAND_IN_SOCKET: &str = "XGUITARE_STAND_IN_SOCKET";
/// Long enough for the window to connect to the bound socket and wait on its `status` reply.
const STAND_IN_LIFETIME: Duration = Duration::from_millis(300);
const STAND_IN_LINE_LEN: usize = 5_000;
const KEPT_LINE_MAX: usize = 4_096;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stand_in_line() -> String {
    let error = "\u{1b}[31merror\u{1b}[0m: cannot open JACK ";
    format!("{error}{}", "x".repeat(STAND_IN_LINE_LEN - error.len()))
}

#[test]
#[ignore = "the stand-in Daemon the other test starts: it binds the socket, as `xguitare run` does before JACK, and exits"]
fn stand_in_daemon() {
    let Some(socket) = env::var_os(STAND_IN_SOCKET) else {
        return;
    };
    let listener = UnixListener::bind(&socket).unwrap();
    thread::sleep(STAND_IN_LIFETIME);
    let _ = writeln!(io::stderr(), "loading config\n{}\n", stand_in_line());
    drop(listener);
    let _ = fs::remove_file(&socket);
    std::process::exit(1);
}

#[test]
fn a_daemon_that_exits_before_answering_is_shown_and_started_again_later() {
    let dir = TempDir(PathBuf::from(format!(
        "/tmp/xguitare-ge-{}",
        std::process::id()
    )));
    let _ = fs::remove_dir_all(&dir.0);
    fs::create_dir_all(&dir.0).unwrap();
    let starts = dir.0.join("starts");
    let socket = dir.0.join("s.sock");
    let fake_daemon = dir.0.join("xguitare");
    fs::write(
        &fake_daemon,
        format!(
            "#!/bin/sh\necho started >> {}\n{STAND_IN_SOCKET}={} exec {} --exact stand_in_daemon --ignored --nocapture\n",
            starts.display(),
            socket.display(),
            env::current_exe().unwrap().display()
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
        socket,
        socket_is_fallback: false,
        ..ClientConfig::load(Some(&config_path)).unwrap()
    };

    let (commands, snapshots, handle) =
        client::spawn(config, None, Arc::new(AtomicBool::new(false)), || {});
    let is_exited = |link: &Link| matches!(link, Link::Exited { .. });
    let deadline = Instant::now() + WAIT;
    let mut link = None;
    while Instant::now() < deadline && !link.as_ref().is_some_and(is_exited) {
        link = snapshots
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
            .map(|snapshot| snapshot.link);
    }
    let Some(Link::Exited {
        status, last_line, ..
    }) = link
    else {
        panic!("the link never showed the exit: {link:?}");
    };
    assert_eq!(status, "exit status 1");
    assert_eq!(
        last_line,
        Some(stand_in_line()[..KEPT_LINE_MAX].to_string())
    );

    let second_start = Instant::now() + respawn_delay(1) + WAIT;
    while Instant::now() < second_start && fs::read_to_string(&starts).unwrap().lines().count() < 2
    {
        if let Ok(snapshot) = snapshots.recv_timeout(Duration::from_millis(100)) {
            assert!(
                is_exited(&snapshot.link),
                "the banner left before a connection: {:?}",
                snapshot.link
            );
        }
    }
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();

    assert_eq!(fs::read_to_string(&starts).unwrap(), "started\nstarted\n");
}
