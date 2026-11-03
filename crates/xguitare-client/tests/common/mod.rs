use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;

use xguitare_core::Config;

pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(test: &str) -> TempDir {
        let path = PathBuf::from(format!("/tmp/xguitare-cl-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }

    pub fn socket(&self) -> PathBuf {
        self.path.join("s.sock")
    }

    pub fn config(&self) -> Config {
        Config {
            socket: self.socket(),
            ..Config::load(Some(&self.path.join("absent.toml"))).unwrap()
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Accepts one client and answers each request line with the next of `replies`, then closes the stream.
pub fn serve(socket: &Path, replies: &[&str]) -> thread::JoinHandle<()> {
    let listener = UnixListener::bind(socket).unwrap();
    let replies: Vec<String> = replies.iter().map(|line| format!("{line}\n")).collect();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        answer(&stream, &replies);
    })
}

pub fn answer(stream: &UnixStream, replies: &[String]) {
    let mut reader = BufReader::new(stream);
    let mut writer = stream;
    for reply in replies {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            return;
        }
        writer.write_all(reply.as_bytes()).unwrap();
    }
}
