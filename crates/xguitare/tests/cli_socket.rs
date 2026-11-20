use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;

const STATUS_REPLY: &str = r#"{"ok":true,"preset":"Clean","program":0,"modified":false,"sample_rate":48000,"buffer_size":128,"xruns":0,"callback_max_us":0,"callback_last_us":0,"connections":[]}"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(test: &str) -> TempDir {
        let path = PathBuf::from(format!("/tmp/xguitare-cs-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        for dir in ["home", "config/xguitare", "runtime/xguitare"] {
            fs::create_dir_all(path.join(dir)).unwrap();
        }
        fs::set_permissions(path.join("runtime"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(
            path.join("runtime/xguitare"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        TempDir(path)
    }

    fn default_config_file(&self) -> PathBuf {
        self.0.join("config/xguitare/config.toml")
    }

    fn derived_socket(&self) -> PathBuf {
        self.0.join("runtime/xguitare/xguitare.sock")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs `xguitare <args> status` with the user's directories pointed into `dir`, against a listener at `socket` that answers one `status`.
fn status_through(dir: &TempDir, socket: &Path, args: &[&str]) -> Output {
    status_answered(dir, socket, args, STATUS_REPLY.to_string())
}

/// As `status_through`, with `reply` as the listener's answer.
fn status_answered(dir: &TempDir, socket: &Path, args: &[&str], reply: String) -> Output {
    let listener = UnixListener::bind(socket).unwrap();
    let daemon = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(&stream).read_line(&mut request).unwrap();
        (&stream)
            .write_all(format!("{reply}\n").as_bytes())
            .unwrap();
        request
    });
    let output = Command::new(env!("CARGO_BIN_EXE_xguitare"))
        .args(args)
        .arg("status")
        .env("HOME", dir.0.join("home"))
        .env("XDG_CONFIG_HOME", dir.0.join("config"))
        .env("XDG_RUNTIME_DIR", dir.0.join("runtime"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(daemon.join().unwrap(), "{\"cmd\":\"status\"}\n");
    output
}

fn assert_status_printed(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("preset: Clean\n"), "{stdout}");
}

#[test]
fn config_flag_socket_reaches_the_daemon() {
    let dir = TempDir::new("config-flag");
    let socket = dir.0.join("s.sock");
    let config = dir.0.join("elsewhere.toml");
    fs::write(&config, format!("socket = \"{}\"\n", socket.display())).unwrap();
    let output = status_through(&dir, &socket, &["--config", config.to_str().unwrap()]);
    assert_status_printed(&output);
}

#[test]
fn default_config_file_socket_reaches_the_daemon() {
    let dir = TempDir::new("default-file");
    let socket = dir.0.join("s.sock");
    fs::write(
        dir.default_config_file(),
        format!("socket = \"{}\"\n", socket.display()),
    )
    .unwrap();
    let output = status_through(&dir, &socket, &[]);
    assert_status_printed(&output);
}

#[test]
fn no_config_file_finds_the_derived_socket() {
    let dir = TempDir::new("no-file");
    let output = status_through(&dir, &dir.derived_socket(), &[]);
    assert_status_printed(&output);
}

#[test]
fn bad_daemon_only_keys_do_not_stop_a_client_command() {
    let dir = TempDir::new("daemon-keys");
    fs::write(
        dir.default_config_file(),
        "buffer_size = 0\nbuffer_siz = 64\n[ports]\nimput = \"x\"\n",
    )
    .unwrap();
    let output = status_through(&dir, &dir.derived_socket(), &[]);
    assert_status_printed(&output);
}

fn status_with_version(test: &str, version: u32) -> Output {
    let dir = TempDir::new(test);
    let reply = STATUS_REPLY.replace(
        "\"ok\":true",
        &format!("\"ok\":true,\"protocol_version\":{version}"),
    );
    status_answered(&dir, &dir.derived_socket(), &[], reply)
}

#[test]
fn status_prints_the_daemon_protocol_version() {
    let output = status_with_version("version", 1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\nprotocol_version: 1\n"), "{stdout}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[test]
fn a_newer_daemon_protocol_gets_one_warning_line() {
    let output = status_with_version("newer", 2);
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "warning: the Daemon speaks protocol 2, this xguitare knows 1; update xguitare\n"
    );
    assert_status_printed(&output);
}

#[test]
fn a_status_without_a_version_prints_no_version_line() {
    let dir = TempDir::new("unversioned");
    let output = status_through(&dir, &dir.derived_socket(), &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("protocol_version"), "{stdout}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

fn status_with_raw_version(test: &str, raw: &str) -> Output {
    let dir = TempDir::new(test);
    let reply = STATUS_REPLY.replace(
        "\"ok\":true",
        &format!("\"ok\":true,\"protocol_version\":{raw}"),
    );
    status_answered(&dir, &dir.derived_socket(), &[], reply)
}

#[test]
fn a_version_that_is_not_a_higher_whole_number_gives_no_warning_and_no_crash() {
    for (raw, shown) in [
        ("\"2\"", None),
        ("-1", None),
        ("2.5", None),
        ("null", None),
        ("100000000000000000000000", None),
        ("0", Some("0")),
        ("1", Some("1")),
    ] {
        let output = status_with_raw_version("odd-version", raw);
        assert_status_printed(&output);
        assert_eq!(String::from_utf8_lossy(&output.stderr), "", "{raw}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        match shown {
            Some(n) => assert!(
                stdout.contains(&format!("\nprotocol_version: {n}\n")),
                "{raw}: {stdout}"
            ),
            None => assert!(!stdout.contains("protocol_version"), "{raw}: {stdout}"),
        }
    }
}
