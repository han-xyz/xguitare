use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use xguitare::socket_server::{IDLE_TIMEOUT, MAX_CONNECTIONS, bind};
use xguitare_core::control::EngineState;
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues, raise_peak};
use xguitare_core::ports::{AudioPorts, AvailablePort};
use xguitare_core::project::{Item, Project, Take};
use xguitare_core::take_peaks::MAX_PEAKS_PER_REPLY;
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, Config, Ports};
use xguitare_protocol::{FRAMES_PER_PEAK, Value};

struct NoPorts;

impl AudioPorts for NoPorts {
    fn audio_ports(&self) -> (Vec<AvailablePort>, Vec<AvailablePort>) {
        (Vec::new(), Vec::new())
    }

    fn own_ports(&self) -> Ports {
        unreachable!("the socket tests never ask for ports")
    }

    fn connect(&self, _: &str, _: &str) -> Result<(), String> {
        unreachable!("the socket tests never connect")
    }

    fn disconnect(&self, _: &str, _: &str) -> Result<(), String> {
        unreachable!("the socket tests never disconnect")
    }
}

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
    server_with(test, MAX_CONNECTIONS, IDLE_TIMEOUT, &[])
}

/// A Server whose Bank holds `presets`, each a file name and its TOML.
fn server_with(
    test: &str,
    max_connections: usize,
    idle_timeout: Duration,
    presets: &[(&str, &str)],
) -> Server {
    let dir = PathBuf::from(format!("/tmp/xguitare-srv-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let path = dir.join("s.sock");
    let socket = bind(&path, max_connections, idle_timeout).unwrap();
    for (file, toml) in presets {
        fs::write(dir.join(file), toml).unwrap();
    }
    let (queues, audio, tap) = engine_queues(64);
    let config = empty_config(&dir);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let shared = Arc::new(EngineShared::default());
    let state = EngineState::new(
        Bank::load_dir(&dir).unwrap(),
        Arc::clone(&shared),
        queues,
        tuner,
        config,
        48_000.0,
        128,
    );
    let stop = Arc::new(AtomicBool::new(false));
    let thread = socket
        .spawn(
            Arc::new(Mutex::new(state)),
            Weak::<NoPorts>::new(),
            Arc::clone(&stop),
        )
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
fn malformed_line_gets_the_invalid_request_reply() {
    let server = server("malformed");
    let stream = UnixStream::connect(&server.path).unwrap();
    assert_eq!(
        request(&stream, "{\"cmd\":"),
        "{\"ok\":false,\"error\":\"invalid request: EOF while parsing a value at line 1 column 7\"}\n"
    );
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_load_build_warning_is_logged() {
    let log = LogBuffer::default();
    let sink = log.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish(),
    )
    .unwrap();
    let server = server_with(
        "load-warning",
        MAX_CONNECTIONS,
        IDLE_TIMEOUT,
        &[(
            "01-cab.toml",
            "name = \"Cab\"\n[[module]]\ntype = \"cabinet\"\nir = \"missing.wav\"\n",
        )],
    );
    let stream = UnixStream::connect(&server.path).unwrap();
    assert!(request(&stream, r#"{"cmd":"load","preset":"Cab"}"#).starts_with(r#"{"ok":true"#));
    let logged = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("WARN") && logged.contains("cabinet 'cabinet': "),
        "{logged}"
    );
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
    assert!(!path.with_extension("sock.lock").exists());
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
    let server = server_with("cap", 2, IDLE_TIMEOUT, &[]);
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
    let server = server_with(
        "idle-closed",
        MAX_CONNECTIONS,
        Duration::from_millis(300),
        &[],
    );
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
    Config {
        projects_dir: dir.join("projects"),
        ..Config::load(Some(&path)).unwrap()
    }
}

/// One more pair than a reply carries, each pair at full scale: the longest `take_peaks` line.
const TAKE_FRAMES: usize = (MAX_PEAKS_PER_REPLY + 1) * FRAMES_PER_PEAK;

fn write_full_scale_take(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..TAKE_FRAMES {
        writer
            .write_sample(if n % 2 == 0 { -1.0_f32 } else { 1.0 })
            .unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn a_full_take_peaks_reply_crosses_the_socket() {
    let server = server("take-peaks");
    let dir = server.path.parent().unwrap();
    let stream = UnixStream::connect(&server.path).unwrap();
    assert!(
        request(&stream, r#"{"cmd":"project_new","name":"Song"}"#).starts_with(r#"{"ok":true"#)
    );
    assert!(
        request(&stream, r#"{"cmd":"track_add","name":"Guitar"}"#).starts_with(r#"{"ok":true"#)
    );
    assert!(request(&stream, r#"{"cmd":"project_close"}"#).starts_with(r#"{"ok":true"#));
    let folder = dir.join("projects/song");
    write_full_scale_take(&folder.join("takes/a.wav"));
    let mut project = Project::load(&folder).unwrap().0;
    project.tracks[0].items = vec![Item {
        id: 1,
        position: 0,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![Take {
            file: "takes/a.wav".to_string(),
            frames: TAKE_FRAMES as u64,
        }],
    }];
    project.next_item_id = 2;
    project.save(&folder).unwrap();
    assert!(
        request(&stream, r#"{"cmd":"project_open","name":"Song"}"#).starts_with(r#"{"ok":true"#)
    );
    for line in [
        r#"{"cmd":"take_peaks","file":"takes/a.wav"}"#,
        r#"{"cmd":"take_peaks","file":"takes/a.wav","first":0,"count":20000}"#,
    ] {
        let reply: Value = request(&stream, line).parse().unwrap();
        assert_eq!(reply["total"], MAX_PEAKS_PER_REPLY + 1);
        let peaks = reply["peaks"].as_array().unwrap();
        assert_eq!(peaks.len(), MAX_PEAKS_PER_REPLY * 2);
        assert!(peaks.chunks(2).all(|pair| pair == [-127, 127]));
    }
}

/// Song on `server`, whose one Item's active Take is a FIFO, so a read of it waits until the test writes; returns the FIFO's path.
fn song_with_fifo_take(server: &Server) -> PathBuf {
    let dir = server.path.parent().unwrap();
    let opener = UnixStream::connect(&server.path).unwrap();
    assert!(
        request(&opener, r#"{"cmd":"project_new","name":"Song"}"#).starts_with(r#"{"ok":true"#)
    );
    assert!(
        request(&opener, r#"{"cmd":"track_add","name":"Guitar"}"#).starts_with(r#"{"ok":true"#)
    );
    assert!(request(&opener, r#"{"cmd":"project_close"}"#).starts_with(r#"{"ok":true"#));
    let folder = dir.join("projects/song");
    let fifo = folder.join("takes/a.wav");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let mut project = Project::load(&folder).unwrap().0;
    project.tracks[0].items = vec![Item {
        id: 1,
        position: 0,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![Take {
            file: "takes/a.wav".to_string(),
            frames: 4,
        }],
    }];
    project.next_item_id = 2;
    project.save(&folder).unwrap();
    fifo
}

/// The open file descriptors of this process on `path`.
fn open_fds(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|fd| fs::read_link(fd.path()).is_ok_and(|target| target == path))
        .count()
}

#[test]
fn status_is_answered_while_a_project_is_being_read() {
    let server = server("read-outside-lock");
    let fifo = song_with_fifo_take(&server);
    let opener = UnixStream::connect(&server.path).unwrap();
    let mut writer = &opener;
    writer
        .write_all(b"{\"cmd\":\"project_open\",\"name\":\"Song\"}\n")
        .unwrap();
    // Opening the FIFO for writing returns once the read phase has opened it for reading.
    let mut take = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    let watcher = UnixStream::connect(&server.path).unwrap();
    watcher
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let status: Value = request(&watcher, r#"{"cmd":"status"}"#).parse().unwrap();
    assert_eq!(status["project"], Value::Null);
    take.write_all(&wav_bytes(&[0.5; 4])).unwrap();
    drop(take);
    let mut reply = String::new();
    BufReader::new(&opener).read_line(&mut reply).unwrap();
    assert!(reply.starts_with(r#"{"ok":true"#), "{reply}");
    let status: Value = request(&watcher, r#"{"cmd":"status"}"#).parse().unwrap();
    assert_eq!(status["project"], "Song");
}

fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut bytes = io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut bytes, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
    bytes.into_inner()
}

#[test]
fn a_second_project_open_waits_for_the_first_read() {
    let server = server("one-read");
    let fifo = song_with_fifo_take(&server);
    let first = UnixStream::connect(&server.path).unwrap();
    let mut writer = &first;
    writer
        .write_all(b"{\"cmd\":\"project_open\",\"name\":\"Song\"}\n")
        .unwrap();
    // Opening the FIFO for writing returns once the first read has opened it for reading.
    let mut take = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    let second = UnixStream::connect(&server.path).unwrap();
    second
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut writer = &second;
    writer
        .write_all(b"{\"cmd\":\"project_open\",\"name\":\"Song\"}\n")
        .unwrap();
    let mut waiting = String::new();
    let err = BufReader::new(&second).read_line(&mut waiting).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock, "{waiting}");
    assert_eq!(open_fds(&fifo), 2, "only the first open reads the Take");
    let watcher = UnixStream::connect(&server.path).unwrap();
    watcher
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let status: Value = request(&watcher, r#"{"cmd":"status"}"#).parse().unwrap();
    assert_eq!(status["project"], Value::Null);
    take.write_all(&wav_bytes(&[0.5; 4])).unwrap();
    drop(take);
    let mut reply = String::new();
    BufReader::new(&first).read_line(&mut reply).unwrap();
    assert!(reply.starts_with(r#"{"ok":true"#), "{reply}");
    second
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut refusal = String::new();
    BufReader::new(&second).read_line(&mut refusal).unwrap();
    assert_eq!(
        refusal.trim_end(),
        r#"{"ok":false,"error":"a project is open; close it first"}"#
    );
}
