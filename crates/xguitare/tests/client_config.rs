use std::fs;
use std::path::PathBuf;

use xguitare_client::ClientConfig;
use xguitare_core::Config;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config_file(test: &str, contents: &str) -> (TempDir, PathBuf) {
    let dir = PathBuf::from(format!("/tmp/xguitare-cc-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    fs::write(&path, contents).unwrap();
    (TempDir(dir), path)
}

#[test]
fn client_config_agrees_with_the_daemon_config() {
    let cases = [
        (
            "explicit-socket",
            "socket = \"/tmp/elsewhere/x.sock\"\nbuffer_size = 64\n",
        ),
        ("default-socket", "[ports]\ninput = \"\"\n"),
        (
            "tuner-and-bank",
            "tuner_mute = false\nbank_kind = \"bass\"\n",
        ),
    ];
    for (test, contents) in cases {
        let (_dir, path) = config_file(test, contents);
        let daemon = Config::load(Some(&path)).unwrap();
        let client = ClientConfig::load(Some(&path)).unwrap();
        assert_eq!(client.socket, daemon.socket, "{test}");
        assert_eq!(
            client.socket_is_fallback, daemon.socket_is_fallback,
            "{test}"
        );
        assert_eq!(client.tuner_mute, daemon.tuner_mute, "{test}");
        assert_eq!(client.bank_kind, daemon.bank_kind, "{test}");
    }
}

#[test]
fn malformed_client_keys_give_the_daemon_config_error() {
    let cases = [
        ("syntax", "tuner_mute = true\n[ports\n"),
        ("tuner-mute-type", "tuner_mute = 3\n"),
        ("bank-kind-value", "bank_kind = \"ukulele\"\n"),
    ];
    for (test, contents) in cases {
        let (_dir, path) = config_file(test, contents);
        let daemon = Config::load(Some(&path)).unwrap_err().to_string();
        let client = ClientConfig::load(Some(&path)).unwrap_err().to_string();
        assert_eq!(client, daemon, "{test}");
    }
    let missing = PathBuf::from("/tmp/xguitare-cc-does-not-exist/config.toml");
    assert_eq!(
        ClientConfig::load(Some(&missing)).unwrap_err().to_string(),
        Config::load(Some(&missing)).unwrap_err().to_string()
    );
}
