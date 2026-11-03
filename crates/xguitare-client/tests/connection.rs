mod common;

use std::fs;
use std::os::unix::net::UnixListener;
use std::thread;
use std::time::{Duration, Instant};

use common::{TempDir, answer, serve};
use xguitare_client::{ClientError, Connection};
use xguitare_core::control::{Request, Response};

const STATUS: &str = r#"{"ok":true,"preset":"Clean","revision":3}"#;
const LIST: &str = r#"{"ok":true,"active":0,"entries":[]}"#;

#[test]
fn request_round_trip() {
    let dir = TempDir::new("round-trip");
    let server = serve(&dir.socket(), &[STATUS, LIST]);
    let mut connection = Connection::connect(&dir.config()).unwrap();
    assert_eq!(
        connection.request(&Request::Status).unwrap(),
        Response::from_line(STATUS).unwrap()
    );
    assert_eq!(
        connection.request(&Request::List).unwrap(),
        Response::from_line(LIST).unwrap()
    );
    drop(connection);
    server.join().unwrap();
}

#[test]
fn read_timeout_is_not_responding() {
    let dir = TempDir::new("timeout");
    let listener = UnixListener::bind(dir.socket()).unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(2));
        drop(stream);
    });
    let mut connection = Connection::connect(&dir.config()).unwrap();
    let started = Instant::now();
    let err = connection.request(&Request::Status).unwrap_err();
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(
        matches!(&err, ClientError::NotResponding(path) if *path == dir.socket()),
        "{err:?}"
    );
    server.join().unwrap();
}

#[test]
fn server_gone_is_io_error_then_reconnect_works() {
    let dir = TempDir::new("reconnect");
    let listener = UnixListener::bind(dir.socket()).unwrap();
    let server = thread::spawn(move || drop(listener.accept().unwrap()));
    let mut connection = Connection::connect(&dir.config()).unwrap();
    server.join().unwrap();
    fs::remove_file(dir.socket()).unwrap();

    let err = connection.request(&Request::Status).unwrap_err();
    assert!(matches!(err, ClientError::Io(_)), "{err:?}");
    let err = connection.request(&Request::Status).unwrap_err();
    assert!(matches!(err, ClientError::Io(_)), "{err:?}");

    let listener = UnixListener::bind(dir.socket()).unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        answer(&stream, &[format!("{STATUS}\n")]);
    });
    let mut connection = Connection::connect(&dir.config()).unwrap();
    assert_eq!(
        connection.request(&Request::Status).unwrap(),
        Response::from_line(STATUS).unwrap()
    );
    server.join().unwrap();
}

#[test]
fn missing_socket_is_not_running() {
    let dir = TempDir::new("missing");
    let err = Connection::connect(&dir.config()).err().unwrap();
    assert_eq!(
        err.to_string(),
        format!("daemon not running (socket {})", dir.socket().display())
    );
}

#[test]
fn malformed_line_is_protocol_error() {
    let dir = TempDir::new("malformed");
    let server = serve(&dir.socket(), &["not json"]);
    let mut connection = Connection::connect(&dir.config()).unwrap();
    let err = connection.request(&Request::Status).unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)), "{err:?}");
    assert!(
        err.to_string().starts_with("bad response from daemon: "),
        "{err}"
    );
    server.join().unwrap();
}
