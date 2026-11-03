use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use xguitare::socket_server::bind;
use xguitare_core::control::EngineState;
use xguitare_core::engine::{AudioQueues, EngineShared, engine_queues};
use xguitare_core::{Bank, Config};

struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
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
    let dir = PathBuf::from(format!("/tmp/xguitare-srv-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let path = dir.join("s.sock");
    let socket = bind(&path).unwrap();
    let (queues, audio) = engine_queues(64);
    let state = EngineState {
        bank: Bank {
            dir: dir.clone(),
            entries: Vec::new(),
        },
        active: None,
        shared: Arc::new(EngineShared::default()),
        queues,
        config: Config::load(Some(&dir.join("absent.toml"))).unwrap(),
        connections: Vec::new(),
        sample_rate: 48_000.0,
        max_frames: 128,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let thread = socket
        .spawn(Arc::new(Mutex::new(state)), Arc::clone(&stop))
        .unwrap();
    Server {
        path,
        stop,
        thread: Some(thread),
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
fn stop_removes_socket_file() {
    let mut server = server("stop");
    assert!(server.path.exists());
    server.stop.store(true, Ordering::Relaxed);
    server.thread.take().unwrap().join().unwrap();
    assert!(!server.path.exists());
}
