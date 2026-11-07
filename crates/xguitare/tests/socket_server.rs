use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use xguitare::socket_server::{IDLE_TIMEOUT, MAX_CONNECTIONS, bind};
use xguitare_core::control::{EngineState, PeakMeters, Value};
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues, raise_peak};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, Config};

struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<EngineShared>,
    _audio: AudioQueues,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        let _ = fs::remove_dir_all(self.path.parent().unwrap());
    }
}

fn server(test: &str) -> Server {
    server_with(test, MAX_CONNECTIONS, IDLE_TIMEOUT)
}

fn server_with(test: &str, max_connections: usize, idle_timeout: Duration) -> Server {
    let dir = PathBuf::from(format!("/tmp/xguitare-srv-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let path = dir.join("s.sock");
    let socket = bind(&path, max_connections, idle_timeout).unwrap();
    let (queues, audio, tap) = engine_queues(64);
    let config = empty_config(&dir);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let shared = Arc::new(EngineShared::default());
    let state = EngineState {
        bank: Bank {
            dir: dir.clone(),
            entries: Vec::new(),
        },
        active: None,
        shared: Arc::clone(&shared),
        queues,
        tuner,
        config,
        connections: Vec::new(),
        sample_rate: 48_000.0,
        max_frames: 128,
        revision: 1,
        peaks: PeakMeters::default(),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let thread = socket
        .spawn(Arc::new(Mutex::new(state)), Arc::clone(&stop))
        .unwrap();
    Server {
        path,
        stop,
        thread: Some(thread),
        shared,
        _audio: audio,
    }
}

fn request(stream: &UnixStream, line: &str) -> String {
    let mut writer = stream;
    writer.write_all(format!("{line}\n").as_bytes()).unwrap();
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).unwrap();
    reply
}

#[test]
fn over_long_line_is_refused_and_dropped() {
    let server = server("long");
    let mut stream = UnixStream::connect(&server.path).unwrap();
    stream.write_all(&vec![b'x'; 70 * 1024]).unwrap();
    let mut reply = String::new();
    let mut reader = BufReader::new(&stream);
    reader.read_line(&mut reply).unwrap();
    assert_eq!(reply, "{\"ok\":false,\"error\":\"request too long\"}\n");
    let mut rest = Vec::new();
    assert_eq!(reader.read_to_end(&mut rest).unwrap(), 0);
}

#[test]
fn idle_connection_does_not_block_others() {
    let server = server("idle");
    let idle = UnixStream::connect(&server.path).unwrap();
    let busy = UnixStream::connect(&server.path).unwrap();
    let reply = request(&busy, r#"{"cmd":"list"}"#);
    assert_eq!(reply, "{\"ok\":true,\"active\":null,\"entries\":[]}\n");
    drop(idle);
}

#[test]
fn idle_client_stays_connected() {
    let server = server("idle-stays");
    let stream = UnixStream::connect(&server.path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let reply = request(&stream, r#"{"cmd":"status"}"#);
    assert!(reply.starts_with("{\"ok\":true,"), "{reply}");
}

#[test]
fn stop_removes_socket_file() {
    let mut server = server("stop");
    assert!(server.path.exists());
    server.stop.store(true, Ordering::Relaxed);
    server.thread.take().unwrap().join().unwrap();
    assert!(!server.path.exists());
}

#[test]
fn stop_removes_lock_file_and_allows_restart() {
    let mut server = server("relock");
    let lock = server.path.with_extension("sock.lock");
    assert!(lock.exists());
    server.stop.store(true, Ordering::Relaxed);
    server.thread.take().unwrap().join().unwrap();
    assert!(!lock.exists());
    let again = bind(&server.path, MAX_CONNECTIONS, IDLE_TIMEOUT).unwrap();
    assert!(lock.exists());
    drop(again);
    assert!(!lock.exists());
}

#[test]
fn regular_file_at_socket_path_is_refused_and_kept() {
    let dir = PathBuf::from(format!("/tmp/xguitare-srv-{}-file", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.sock");
    fs::write(&path, "mine").unwrap();
    let error = bind(&path, MAX_CONNECTIONS, IDLE_TIMEOUT).err().unwrap();
    assert_eq!(
        error.to_string(),
        format!("socket path {} exists and is not a socket", path.display())
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn second_daemon_refused() {
    let server = server("second");
    let error = bind(&server.path, MAX_CONNECTIONS, IDLE_TIMEOUT)
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "another Daemon is running");
    let stream = UnixStream::connect(&server.path).unwrap();
    let reply = request(&stream, r#"{"cmd":"list"}"#);
    assert!(reply.starts_with("{\"ok\":true,"), "{reply}");
}

#[test]
fn connection_cap_refuses_extra() {
    let server = server_with("cap", 2, IDLE_TIMEOUT);
    let first = UnixStream::connect(&server.path).unwrap();
    let second = UnixStream::connect(&server.path).unwrap();
    for stream in [&first, &second] {
        assert!(request(stream, r#"{"cmd":"list"}"#).starts_with("{\"ok\":true,"));
    }
    let extra = UnixStream::connect(&server.path).unwrap();
    extra
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(&extra);
    let mut reply = String::new();
    reader.read_line(&mut reply).unwrap();
    assert_eq!(reply, "{\"ok\":false,\"error\":\"too many connections\"}\n");
    let mut rest = Vec::new();
    assert_eq!(reader.read_to_end(&mut rest).unwrap(), 0);
    drop(first);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let stream = UnixStream::connect(&server.path).unwrap();
        let reply = request(&stream, r#"{"cmd":"list"}"#);
        if reply.starts_with("{\"ok\":true,") {
            break;
        }
        assert!(Instant::now() < deadline, "slot not freed: {reply}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn idle_connection_closed() {
    let server = server_with("idle-closed", MAX_CONNECTIONS, Duration::from_millis(300));
    let mut stream = UnixStream::connect(&server.path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    assert!(request(&stream, r#"{"cmd":"list"}"#).starts_with("{\"ok\":true,"));
    let started = Instant::now();
    let mut rest = Vec::new();
    assert_eq!(stream.read_to_end(&mut rest).unwrap(), 0);
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[test]
fn peaks_per_connection() {
    let server = server("peaks");
    let first = UnixStream::connect(&server.path).unwrap();
    let second = UnixStream::connect(&server.path).unwrap();
    let input_peak = |stream: &UnixStream| {
        let reply: Value = request(stream, r#"{"cmd":"status"}"#).parse().unwrap();
        reply["input_peak_db"].clone()
    };
    input_peak(&first);
    input_peak(&second);
    raise_peak(&server.shared.input_peak_bits, 0.5);
    assert_eq!(input_peak(&first), -6.0);
    raise_peak(&server.shared.input_peak_bits, 0.25);
    assert_eq!(input_peak(&second), -6.0);
    assert_eq!(input_peak(&first), -12.0);
    assert_eq!(input_peak(&first), -120.0);
}

fn empty_config(dir: &Path) -> Config {
    let path = dir.join("config");
    fs::write(&path, "").unwrap();
    Config::load(Some(&path)).unwrap()
}
