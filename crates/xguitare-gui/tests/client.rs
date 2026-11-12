use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use xguitare_core::Config;
use xguitare_gui::client;
use xguitare_gui::model::{Command, Link, Restore};

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
    let set_arrival = next_arrival(&received, "set");
    assert!(
        set_arrival.duration_since(sent) < Duration::from_millis(25),
        "set took {:?} after the send, {:?} after the poll",
        set_arrival.duration_since(sent),
        set_arrival.duration_since(polled)
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

#[test]
fn modules_transport_error_leaves_no_stale_status() {
    let dir = temp_dir("modules-broken");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let received = serve_and_record(listener, |cmd| match cmd {
        "modules" => String::new(),
        _ => reply(cmd).to_string(),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    next_arrival(&received, "modules");
    let deadline = Instant::now() + Duration::from_millis(300);
    while let Ok(snapshot) =
        snapshots.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        assert!(
            snapshot.link == Link::Connected || snapshot.status.is_none(),
            "disconnected with a status: {:?}",
            snapshot.status
        );
    }
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
}

#[test]
fn mute_queued_after_stop_is_sent_before_the_thread_ends() {
    let dir = temp_dir("stop-mute");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let received = serve_and_record(listener, |cmd| match cmd {
        "mute" => r#"{"ok":true,"muted":false}"#.to_string(),
        _ => reply(cmd).to_string(),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, _snapshots, handle) = client::spawn(config, None, Arc::clone(&stop), || {});
    next_arrival(&received, "status");
    stop.store(true, Ordering::Relaxed);
    thread::sleep(client::TICK * 3);
    let _ = commands.send(Command::Mute(false));
    let _ = commands.send(Command::Quit);
    next_arrival(&received, "mute");
    handle.join().unwrap();
}

#[test]
fn revision_change_fetches_ports_and_settings_again() {
    let dir = temp_dir("refetch");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let mut statuses = 0;
    let mut kind = "guitar";
    let received = serve_and_record(listener, move |cmd| match cmd {
        "status" => {
            statuses += 1;
            let revision = if statuses < 3 { 1 } else { 2 };
            format!(r#"{{"ok":true,"preset":"Clean","program":0,"revision":{revision}}}"#)
        }
        "settings" => {
            let payload = format!(r#"{{"ok":true,"bank_kind":"{kind}"}}"#);
            kind = "bass";
            payload
        }
        "ports" => r#"{"ok":true,"connected":{}}"#.to_string(),
        _ => reply(cmd).to_string(),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    let deadline = Instant::now() + WAIT;
    let mut ports_asked = 0;
    while ports_asked < 2 {
        assert!(Instant::now() < deadline, "ports asked {ports_asked} times");
        let (cmd, _) = received.recv_timeout(WAIT).unwrap();
        ports_asked += usize::from(cmd == "ports");
    }
    loop {
        let snapshot = snapshots.recv_timeout(WAIT).unwrap();
        if snapshot
            .settings
            .as_ref()
            .is_some_and(|s| s["bank_kind"] == "bass")
        {
            break;
        }
    }
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
}

/// Answers every request line by its `cmd` and passes on each request as sent.
fn serve_and_capture(
    listener: UnixListener,
    mut reply: impl FnMut(&str) -> String + Send + 'static,
) -> Receiver<Value> {
    let (requests, received) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&stream);
        let mut writer = &stream;
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let request: Value = serde_json::from_str(&line).unwrap();
            let cmd = request["cmd"].as_str().unwrap().to_string();
            writer
                .write_all(format!("{}\n", reply(&cmd)).as_bytes())
                .unwrap();
            if requests.send(request).is_err() {
                return;
            }
            line.clear();
        }
    });
    received
}

const RACK_AFTER_A_NEW_DRIVE: &str = r#"{"ok":true,"params":[
    {"module":"drive","type":"drive","param":"gain","value":30.0},
    {"module":"drive-2","type":"drive","param":"gain","value":50.0}]}"#;

fn drive_restore() -> Restore {
    Restore {
        type_name: "drive".to_string(),
        after: Some("drive".to_string()),
        at: 1,
        values: vec![("gain".to_string(), 80.0)],
        bypass: true,
        ir: None,
        track: None,
        bus: None,
        master: false,
    }
}

/// The requests the client sends after `get` has answered, up to `until`.
fn requests_after_get(received: &Receiver<Value>, until: &str) -> Vec<Value> {
    while received.recv_timeout(WAIT).unwrap()["cmd"] != "get" {}
    let mut sent = Vec::new();
    loop {
        let request = received.recv_timeout(WAIT).unwrap();
        let cmd = request["cmd"].clone();
        if cmd != "status" && cmd != "tuner" {
            sent.push(request);
        }
        if cmd == until {
            return sent;
        }
    }
}

#[test]
fn undo_sends_its_values_to_the_id_rack_add_answers() {
    let dir = temp_dir("undo-id");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let received = serve_and_capture(listener, |cmd| match cmd {
        "get" => RACK_AFTER_A_NEW_DRIVE.replace('\n', ""),
        "rack_add" => r#"{"ok":true,"id":"drive-3"}"#.to_string(),
        "bypass" => r#"{"ok":true,"id":"drive-3","bypass":true}"#.to_string(),
        _ => reply(cmd).to_string(),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, _snapshots, handle) = client::spawn(config, None, stop, || {});
    thread::sleep(client::TICK * 2);
    commands.send(Command::Restore(drive_restore())).unwrap();
    let sent = requests_after_get(&received, "bypass");
    assert_eq!(
        sent,
        [
            json!({"cmd": "rack_add", "type": "drive", "after": "drive", "at": null}),
            json!({"cmd": "set", "param": "drive-3.gain", "value": 80.0}),
            json!({"cmd": "bypass", "id": "drive-3", "on": true}),
        ]
    );
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
}

#[test]
fn refused_undo_sends_nothing_more() {
    let dir = temp_dir("undo-refused");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let received = serve_and_capture(listener, |cmd| match cmd {
        "get" => RACK_AFTER_A_NEW_DRIVE.replace('\n', ""),
        "rack_add" => {
            r#"{"ok":false,"error":"mono module 'drive-3' after the cabinet"}"#.to_string()
        }
        _ => reply(cmd).to_string(),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, _snapshots, handle) = client::spawn(config, None, stop, || {});
    thread::sleep(client::TICK * 2);
    commands.send(Command::Restore(drive_restore())).unwrap();
    commands.send(Command::Mute(false)).unwrap();
    let sent = requests_after_get(&received, "mute");
    let cmds: Vec<&str> = sent
        .iter()
        .map(|req| req["cmd"].as_str().unwrap())
        .collect();
    assert_eq!(cmds, ["rack_add", "mute"]);
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
}

/// Answers every request line by the request itself and passes on each request as sent.
fn serve_requests(
    listener: UnixListener,
    mut reply: impl FnMut(&Value) -> Value + Send + 'static,
) -> Receiver<Value> {
    let (requests, received) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&stream);
        let mut writer = &stream;
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let request: Value = serde_json::from_str(&line).unwrap();
            writer
                .write_all(format!("{}\n", reply(&request)).as_bytes())
                .unwrap();
            if requests.send(request).is_err() {
                return;
            }
            line.clear();
        }
    });
    received
}

/// A Project "Song" with one Item per Take file in `files`, `None` with no Project open.
fn items_of(files: &[&str]) -> Value {
    let items: Vec<Value> = files
        .iter()
        .enumerate()
        .map(|(id, file)| {
            json!({"id": id, "position": 0, "offset": 0, "length": 100, "take": 0, "takes": [file]})
        })
        .collect();
    json!({"ok": true, "sample_rate": 48000, "tracks": [{"id": 0, "name": "Guitar", "items": items}]})
}

/// A page of `take_peaks` for a Take of `total` pairs, each pair `[-1, 1]`.
fn peak_page(request: &Value, total: u64) -> Value {
    let first = request["first"].as_u64().unwrap_or(0);
    let count = request["count"].as_u64().unwrap_or(16_384);
    let pairs = count.min(total.saturating_sub(first));
    let peaks: Vec<i64> = (0..pairs).flat_map(|_| [-1, 1]).collect();
    json!({"ok": true, "frames_per_peak": 256, "total": total, "first": first, "peaks": peaks})
}

#[test]
fn peaks_are_fetched_in_pages_once_per_take_and_dropped_on_close() {
    let dir = temp_dir("peaks");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let mut statuses = 0;
    let received = serve_requests(listener, move |request| {
        let stage = statuses / 3;
        match request["cmd"].as_str().unwrap() {
            "status" => {
                statuses += 1;
                let project = if stage < 2 {
                    json!("Song")
                } else {
                    Value::Null
                };
                json!({"ok": true, "preset": "Clean", "program": 0, "revision": stage + 1, "project": project})
            }
            "items" if stage == 0 => items_of(&["takes/a.wav"]),
            "items" => items_of(&["takes/a.wav", "takes/b.wav"]),
            "take_peaks" if request["file"] == "takes/a.wav" => peak_page(request, 20_000),
            "take_peaks" => peak_page(request, 3),
            cmd => serde_json::from_str(reply(cmd)).unwrap(),
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    let deadline = Instant::now() + WAIT;
    let mut lengths_seen = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "saw peaks {lengths_seen:?}");
        let snapshot = snapshots.recv_timeout(WAIT).unwrap();
        let mut lengths: Vec<(String, usize)> = snapshot
            .peaks
            .iter()
            .map(|(file, peaks)| (file.clone(), peaks.len()))
            .collect();
        lengths.sort();
        if lengths.is_empty() && !lengths_seen.is_empty() {
            break;
        }
        if !lengths.is_empty() && lengths_seen.last() != Some(&lengths) {
            lengths_seen.push(lengths);
        }
    }
    assert_eq!(
        lengths_seen,
        [
            vec![("takes/a.wav".to_string(), 40_000)],
            vec![
                ("takes/a.wav".to_string(), 40_000),
                ("takes/b.wav".to_string(), 6)
            ],
        ]
    );
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
    let pages: Vec<(Value, Value)> = received
        .try_iter()
        .filter(|request| request["cmd"] == "take_peaks")
        .map(|request| (request["file"].clone(), request["first"].clone()))
        .collect();
    assert_eq!(
        pages,
        [
            (json!("takes/a.wav"), json!(0)),
            (json!("takes/a.wav"), json!(16_384)),
            (json!("takes/b.wav"), json!(0)),
        ]
    );
}
