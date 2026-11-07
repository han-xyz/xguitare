use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use xguitare_core::Config;
use xguitare_gui::client;
use xguitare_gui::model::{Command, Link};

const WAIT: Duration = Duration::from_secs(2);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let path = PathBuf::from(format!("/tmp/xguitare-gc-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    TempDir(path)
}

fn config(dir: &TempDir) -> Config {
    let path = dir.0.join("config.toml");
    fs::write(&path, "").unwrap();
    Config {
        socket: dir.0.join("s.sock"),
        socket_is_fallback: false,
        ..Config::load(Some(&path)).unwrap()
    }
}

fn reply(cmd: &str) -> &'static str {
    match cmd {
        "status" => r#"{"ok":true,"preset":"Clean","program":0,"revision":1}"#,
        "list" => r#"{"ok":true,"active":0,"entries":[]}"#,
        "get" => r#"{"ok":true,"params":[]}"#,
        _ => r#"{"ok":true,"param":"gain.level","value":1.0,"clamped":false}"#,
    }
}

/// Answers every request line by its `cmd` and reports each `cmd` with its arrival time.
fn serve_and_record(
    listener: UnixListener,
    mut reply: impl FnMut(&str) -> String + Send + 'static,
) -> Receiver<(String, Instant)> {
    let (arrivals, received) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&stream);
        let mut writer = &stream;
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let at = Instant::now();
            let request: Value = serde_json::from_str(&line).unwrap();
            let cmd = request["cmd"].as_str().unwrap().to_string();
            writer
                .write_all(format!("{}\n", reply(&cmd)).as_bytes())
                .unwrap();
            if arrivals.send((cmd, at)).is_err() {
                return;
            }
            line.clear();
        }
    });
    received
}

fn next_arrival(received: &Receiver<(String, Instant)>, cmd: &str) -> Instant {
    loop {
        let (arrived, at) = received.recv_timeout(WAIT).unwrap();
        if arrived == cmd {
            return at;
        }
    }
}

#[test]
fn set_goes_out_before_the_next_poll() {
    let dir = temp_dir("set-now");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let received = serve_and_record(listener, |cmd| reply(cmd).to_string());
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, _snapshots, handle) = client::spawn(config, None, stop, || {});
    next_arrival(&received, "status");
    let polled = next_arrival(&received, "status");
    thread::sleep(Duration::from_millis(5));
    let sent = Instant::now();
    commands
        .send(Command::Set {
            param: "gain.level".to_string(),
            value: 1.0,
        })
        .unwrap();
    let set = next_arrival(&received, "set");
    assert!(
        set.duration_since(sent) < Duration::from_millis(25),
        "set took {:?} after the send, {:?} after the poll",
        set.duration_since(sent),
        set.duration_since(polled)
    );
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
}

#[test]
fn modules_error_is_retried_without_disconnect() {
    let dir = temp_dir("modules-retry");
    let config = config(&dir);
    let mut modules_asked = 0;
    let listener = UnixListener::bind(&config.socket).unwrap();
    let received = serve_and_record(listener, move |cmd| {
        if cmd != "modules" {
            return reply(cmd).to_string();
        }
        modules_asked += 1;
        if modules_asked == 1 {
            r#"{"ok":false,"error":"busy"}"#.to_string()
        } else {
            r#"{"ok":true,"modules":[{"type":"gate","channels":"mono","params":[]}]}"#.to_string()
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    let deadline = Instant::now() + WAIT;
    let mut modules_seen = 0;
    while modules_seen < 2 {
        assert!(
            Instant::now() < deadline,
            "modules asked {modules_seen} times"
        );
        let (cmd, _) = received.recv_timeout(WAIT).unwrap();
        modules_seen += usize::from(cmd == "modules");
    }
    let snapshot = loop {
        let snapshot = snapshots.recv_timeout(WAIT).unwrap();
        if !snapshot.module_types.is_empty() {
            break snapshot;
        }
    };
    assert_eq!(snapshot.link, Link::Connected);
    assert_eq!(snapshot.module_types[0].type_name, "gate");
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
}
