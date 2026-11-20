use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_daemon_whose_stderr_reader_is_gone_still_logs_and_exits_cleanly() {
    let dir = TempDir(PathBuf::from(format!(
        "/tmp/xguitare-dse-{}",
        std::process::id()
    )));
    let _ = fs::remove_dir_all(&dir.0);
    fs::create_dir_all(&dir.0).unwrap();
    fs::write(dir.0.join("state.toml"), "not toml [").unwrap();
    fs::write(dir.0.join("not-a-dir"), "").unwrap();
    let config = dir.0.join("config.toml");
    fs::write(
        &config,
        format!(
            "socket = \"{}\"\n",
            dir.0.join("not-a-dir/xguitare.sock").display()
        ),
    )
    .unwrap();
    let (reader, writer) = io::pipe().unwrap();
    drop(reader);

    let status = Command::new(env!("CARGO_BIN_EXE_xguitare"))
        .arg("--config")
        .arg(&config)
        .arg("run")
        .stdout(std::process::Stdio::null())
        .stderr(writer)
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(1), "{status}");
}
