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
use xguitare_client::ClientConfig;
use xguitare_gui::client;
use xguitare_gui::model::{Command, Link, Message, Restore, mute_banner};
use xguitare_gui::project_model::item_blocks;

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

fn config(dir: &TempDir) -> ClientConfig {
    let path = dir.0.join("config.toml");
    fs::write(&path, "").unwrap();
    ClientConfig {
        socket: dir.0.join("s.sock"),
        socket_is_fallback: false,
        ..ClientConfig::load(Some(&path)).unwrap()
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

/// A Project "Song" with one Item per `(Take file, its frames)` in `takes`.
fn items_of(takes: &[(&str, u64)]) -> Value {
    let items: Vec<Value> = takes
        .iter()
        .enumerate()
        .map(|(id, (file, frames))| {
            json!({"id": id, "position": 0, "offset": 0, "length": 100, "take": 0, "takes": [file], "take_frames": frames})
        })
        .collect();
    json!({"ok": true, "sample_rate": 48000, "tracks": [{"id": 0, "name": "Guitar", "items": items}]})
}

/// A page of `take_peaks` for a Take of `frames`, each pair `[-1, 1]`.
fn peak_page(request: &Value, frames: u64) -> Value {
    let total = frames.div_ceil(256);
    let first = request["first"].as_u64().unwrap_or(0);
    let count = request["count"].as_u64().unwrap_or(16_384);
    let pairs = count.min(total.saturating_sub(first));
    let peaks: Vec<i64> = (0..pairs).flat_map(|_| [-1, 1]).collect();
    json!({"ok": true, "frames_per_peak": 256, "frames": frames, "total": total, "first": first, "peaks": peaks})
}

/// A Take of 20000 peak pairs, more than one page.
const A_FRAMES: u64 = 20_000 * 256;

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
            "items" if stage == 0 => items_of(&[("takes/a.wav", A_FRAMES)]),
            "items" => items_of(&[("takes/a.wav", A_FRAMES), ("takes/b.wav", 700)]),
            "take_peaks" if request["file"] == "takes/a.wav" => peak_page(request, A_FRAMES),
            "take_peaks" => peak_page(request, 700),
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

/// The `items` Take frames of `takes/a.wav`, one entry per Revision: refused by the Daemon at the first, then a new recording under the same file name twice.
const REUSED_FRAMES: [u64; 6] = [768, 768, 1280, 1280, 2560, 2560];

#[test]
fn a_refused_take_is_not_asked_again_and_a_reused_file_name_is_fetched_again() {
    let dir = temp_dir("reused-peaks");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let last = REUSED_FRAMES.len() - 1;
    let mut statuses = 0;
    let received = serve_requests(listener, move |request| {
        let stage = (statuses / 3).min(last);
        let frames = REUSED_FRAMES[stage];
        match request["cmd"].as_str().unwrap() {
            "status" => {
                statuses += 1;
                json!({"ok": true, "preset": "Clean", "program": 0, "revision": stage + 1, "project": "Song"})
            }
            "items" => items_of(&[("takes/a.wav", frames)]),
            "take_peaks" if stage == 0 => {
                json!({"ok": false, "error": "no such take: takes/a.wav"})
            }
            "take_peaks" => peak_page(request, frames),
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
        let length = snapshot
            .peaks
            .get("takes/a.wav")
            .map_or(0, |peaks| peaks.len());
        if lengths_seen.last() != Some(&length) {
            lengths_seen.push(length);
        }
        let revision = snapshot.status.as_ref().map(|status| status.revision);
        if revision == Some(REUSED_FRAMES.len() as u64) && snapshot.items.is_some() {
            break;
        }
    }
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
    assert_eq!(lengths_seen, [0, 10, 20]);
    let asked = received
        .try_iter()
        .filter(|request| request["cmd"] == "take_peaks")
        .count();
    assert_eq!(asked, 3, "once refused, once per new recording");
}

#[test]
fn a_reply_with_a_wrong_typed_key_is_reported() {
    let dir = temp_dir("wrong-typed");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let mut statuses = 0;
    let _received = serve_requests(listener, move |request| {
        let stage = (statuses / 3).min(1);
        match request["cmd"].as_str().unwrap() {
            "status" => {
                statuses += 1;
                json!({"ok": true, "preset": "Clean", "program": 0, "revision": stage + 1, "project": "Song"})
            }
            "items" if stage == 0 => items_of(&[("takes/a.wav", 768)]),
            "items" => {
                let mut items = items_of(&[("takes/a.wav", 768), ("takes/b.wav", 768)]);
                items["tracks"][0]["items"][1]["position"] = json!("late");
                items
            }
            "take_peaks" => peak_page(request, 768),
            cmd => serde_json::from_str(reply(cmd)).unwrap(),
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    let (message, items) = loop {
        let snapshot = snapshots
            .recv_timeout(WAIT)
            .expect("no error for the items reply");
        if let Some((message, _)) = snapshot.last_message {
            break (message, snapshot.items);
        }
    };
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
    let Message::Error { title, detail } = message else {
        panic!("not an error: {message:?}");
    };
    assert_eq!(title, "Could not read the Items");
    assert!(detail.contains("invalid type"), "{detail}");
    let files: Vec<String> = items
        .as_ref()
        .map(item_blocks)
        .unwrap_or_default()
        .into_iter()
        .map(|item| item.file)
        .collect();
    assert_eq!(files, ["takes/a.wav"]);
}

#[test]
fn a_status_with_a_wrong_typed_key_is_reported_and_keeps_the_last_status() {
    let dir = temp_dir("wrong-typed-status");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let mut statuses = 0;
    let _received = serve_requests(listener, move |request| {
        match request["cmd"].as_str().unwrap() {
            "status" if statuses < 3 => {
                statuses += 1;
                json!({"ok": true, "preset": "Clean", "program": 0, "revision": 1, "muted": true})
            }
            "status" => {
                json!({"ok": true, "preset": "Clean", "program": 0, "revision": 1, "muted": false, "xruns": "many"})
            }
            cmd => serde_json::from_str(reply(cmd)).unwrap(),
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    let (message, banner) = loop {
        let snapshot = snapshots
            .recv_timeout(WAIT)
            .expect("no error for the status reply");
        if let Some((message, _)) = snapshot.last_message {
            break (message, mute_banner(snapshot.status.as_ref(), false));
        }
    };
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
    let Message::Error { title, detail } = message else {
        panic!("not an error: {message:?}");
    };
    assert_eq!(title, "Could not read the status");
    assert!(detail.contains("invalid type"), "{detail}");
    assert!(banner.is_some(), "the last status, muted, is kept");
}

#[test]
fn a_clean_up_drops_every_cached_peak_table_and_reads_the_unused_takes() {
    let dir = temp_dir("clean-peaks");
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let mut revision = 1;
    let received = serve_requests(listener, move |request| {
        match request["cmd"].as_str().unwrap() {
            "status" => {
                json!({"ok": true, "preset": "Clean", "program": 0, "revision": revision, "project": "Song"})
            }
            "items" => items_of(&[("takes/a.wav", 768)]),
            "take_peaks" => peak_page(request, 768),
            "takes_unused" => {
                json!({"ok": true, "files": [{"file": "takes/b.wav", "bytes": 10}], "bytes": 10})
            }
            "takes_clean" => {
                revision += 1;
                json!({"ok": true, "deleted": 1, "bytes": 10})
            }
            cmd => serde_json::from_str(reply(cmd)).unwrap(),
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    let deadline = Instant::now() + WAIT;
    while !snapshots
        .recv_timeout(WAIT)
        .unwrap()
        .peaks
        .contains_key("takes/a.wav")
    {
        assert!(Instant::now() < deadline, "no peaks");
    }
    commands.send(Command::RefreshUnusedTakes).unwrap();
    let unused = loop {
        assert!(Instant::now() < deadline, "no unused Takes");
        if let Some((unused, _)) = snapshots.recv_timeout(WAIT).unwrap().unused_takes {
            break unused.unwrap();
        }
    };
    assert_eq!(unused["files"][0]["file"], "takes/b.wav");
    commands
        .send(Command::CleanTakes(vec!["takes/b.wav".to_string()]))
        .unwrap();
    let asked = |received: &Receiver<Value>| {
        received
            .try_iter()
            .filter(|request| request["cmd"] == "take_peaks")
            .count()
    };
    let mut peak_requests = asked(&received);
    while peak_requests < 2 {
        assert!(Instant::now() < deadline, "peaks not read again");
        thread::sleep(Duration::from_millis(10));
        peak_requests += asked(&received);
    }
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
}

/// Sends a clean-up of 5000 Takes to a Daemon that reports the first file of the `takes_clean` batches numbered in `failing` (from 1) as not deleted; the message shown and the size of each batch sent.
fn clean_thousands(test: &str, failing: &'static [usize]) -> (Message, Vec<usize>) {
    const MAX_LINE: usize = 64 * 1024;
    let dir = temp_dir(test);
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let mut batch = 0;
    let received = serve_requests(listener, move |request| {
        if request.to_string().len() > MAX_LINE {
            return json!({"ok": false, "error": "request too long"});
        }
        match request["cmd"].as_str().unwrap() {
            "takes_clean" => {
                batch += 1;
                let files = request["files"].as_array().unwrap();
                if !failing.contains(&batch) {
                    return json!({"ok": true, "deleted": files.len(), "bytes": files.len() * 1000});
                }
                let deleted = files.len() - 1;
                let failed =
                    json!([{"file": files[0], "error": "Permission denied (os error 13)"}]);
                json!({"ok": true, "deleted": deleted, "bytes": deleted * 1000, "failed": failed})
            }
            cmd => serde_json::from_str(reply(cmd)).unwrap(),
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    let files: Vec<String> = (0..5000).map(|n| format!("takes/guitar-{n}.wav")).collect();
    commands.send(Command::CleanTakes(files)).unwrap();
    let deadline = Instant::now() + WAIT;
    let message = loop {
        assert!(Instant::now() < deadline, "no message");
        if let Some((message, _)) = snapshots.recv_timeout(WAIT).unwrap().last_message {
            break message;
        }
    };
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
    let batches: Vec<usize> = received
        .try_iter()
        .filter(|request| request["cmd"] == "takes_clean")
        .map(|request| request["files"].as_array().unwrap().len())
        .collect();
    assert_eq!(batches.iter().sum::<usize>(), 5000);
    (message, batches)
}

#[test]
fn a_clean_up_of_thousands_of_takes_goes_in_batches_and_notes_the_total() {
    let (message, batches) = clean_thousands("clean-batches", &[]);
    assert!(batches.len() > 1, "one batch only");
    assert_eq!(
        message,
        Message::Notice("Deleted 5000 Takes, 5.0 MB freed".to_string())
    );
}

#[test]
fn a_clean_up_in_batches_shows_an_error_with_the_files_of_every_batch_not_deleted() {
    let (message, batches) = clean_thousands("clean-batches-failed", &[1, 3]);
    assert!(batches.len() > 3, "too few batches");
    assert_eq!(
        message,
        Message::Error {
            title: "Could not clean up the Takes",
            detail: "Deleted 4998 Takes, 5.0 MB freed; could not delete takes/guitar-0.wav: \
                     Permission denied (os error 13), and 1 more"
                .to_string(),
        }
    );
}

fn notices_for_connections(test: &str, versions: &[&str]) -> Vec<Message> {
    messages_for_connections(test, versions)
        .into_iter()
        .filter(|message| matches!(message, Message::Notice(_)))
        .collect()
}

fn messages_for_connections(test: &str, versions: &[&str]) -> Vec<Message> {
    let versions: Vec<String> = versions.iter().map(ToString::to_string).collect();
    let dir = temp_dir(test);
    let config = config(&dir);
    let listener = UnixListener::bind(&config.socket).unwrap();
    let (answered, done) = mpsc::channel();
    thread::spawn(move || {
        for version in &versions {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut writer = &stream;
            let mut line = String::new();
            let mut statuses = 0;
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                let request: Value = serde_json::from_str(&line).unwrap();
                let cmd = request["cmd"].as_str().unwrap();
                let body = match cmd {
                    "status" => format!(
                        r#"{{"ok":true,"preset":"Clean","program":0,"revision":1{version}}}"#
                    ),
                    cmd => reply(cmd).to_string(),
                };
                writer.write_all(format!("{body}\n").as_bytes()).unwrap();
                line.clear();
                if cmd == "status" {
                    statuses += 1;
                    if statuses == 3 {
                        break;
                    }
                }
            }
        }
        let _ = answered.send(());
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, snapshots, handle) = client::spawn(config, None, stop, || {});
    done.recv_timeout(Duration::from_secs(10)).unwrap();
    thread::sleep(Duration::from_millis(300));
    commands.send(Command::Quit).unwrap();
    handle.join().unwrap();
    let mut seen: Vec<(Message, Instant)> = Vec::new();
    for (message, at) in snapshots.try_iter().filter_map(|s| s.last_message) {
        if !seen.iter().any(|(_, shown_at)| *shown_at == at) {
            seen.push((message, at));
        }
    }
    seen.into_iter().map(|(message, _)| message).collect()
}

#[test]
fn a_reconnect_to_a_newer_daemon_shows_the_notice_again() {
    let notice = Message::Notice(
        "the Daemon speaks protocol 2, this xguitare knows 1; update xguitare".to_string(),
    );
    assert_eq!(
        notices_for_connections(
            "newer-reconnect",
            &[r#","protocol_version":2"#, r#","protocol_version":2"#]
        ),
        [notice.clone(), notice]
    );
}

#[test]
fn an_unreadable_status_from_a_newer_daemon_still_says_to_update() {
    let messages = messages_for_connections(
        "newer-unreadable",
        &[r#","protocol_version":2,"xruns":"many""#],
    );
    assert!(
        messages.iter().any(|message| matches!(message,
            Message::Error { detail, .. }
                if detail.ends_with("; the Daemon speaks protocol 2, this xguitare knows 1; update xguitare"))),
        "{messages:?}"
    );
}

#[test]
fn no_notice_for_a_daemon_without_a_higher_version() {
    for (test, version) in [
        ("no-version", ""),
        ("same-version", r#","protocol_version":1"#),
        ("text-version", r#","protocol_version":"9""#),
        ("negative-version", r#","protocol_version":-4"#),
    ] {
        assert_eq!(notices_for_connections(test, &[version]), [], "{test}");
    }
}
