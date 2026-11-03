use std::collections::VecDeque;
use std::path::PathBuf;

use xguitare::{run_cli, run_main};
use xguitare_client::{ClientError, Transport};
use xguitare_core::control::{Request, Response};

struct Canned {
    replies: VecDeque<Result<Response, ClientError>>,
    requests: Vec<Request>,
}

impl Transport for Canned {
    fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        self.requests.push(req.clone());
        self.replies
            .pop_front()
            .expect("one canned reply per request")
    }
}

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
    requests: Vec<Request>,
}

fn cli(argv: &[&str], reply: Result<Response, ClientError>) -> Outcome {
    let argv: Vec<String> = std::iter::once("xguitare")
        .chain(argv.iter().copied())
        .map(String::from)
        .collect();
    let mut transport = Canned {
        replies: VecDeque::from([reply]),
        requests: Vec::new(),
    };
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run_cli(&argv, &mut transport, &mut stdout, &mut stderr);
    Outcome {
        code,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
        requests: transport.requests,
    }
}

fn reply(line: &str) -> Result<Response, ClientError> {
    Ok(Response::from_line(line).unwrap())
}

#[test]
fn list_format() {
    let out = cli(
        &["list"],
        reply(
            r#"{"ok":true,"active":0,"entries":[{"program":0,"name":"Clean","valid":true},{"program":1,"name":"Loud","valid":true},{"program":12,"name":"13-broken","valid":false,"error":"line 1:8: invalid string"}]}"#,
        ),
    );
    assert_eq!(out.requests, [Request::List]);
    assert_eq!(
        out.stdout,
        " 0* Clean\n 1  Loud\n12! 13-broken (line 1:8: invalid string)\n"
    );
    assert_eq!(out.code, 0);
}

#[test]
fn status_format() {
    let out = cli(
        &["status"],
        reply(
            r#"{"ok":true,"preset":"Clean","program":0,"sample_rate":48000,"buffer_size":128,"prepared_frames":128,"revision":7,"input_peak_db":-120.0,"output_peak_db":-3.5,"xruns":2,"callback_max_us":310,"callback_last_us":95,"connections":[{"from":"Scarlett:capture_FL","to":"xguitare:in"},{"from":"xguitare:out_l","to":"Scarlett:playback_FL"}]}"#,
        ),
    );
    assert_eq!(out.requests, [Request::Status]);
    assert_eq!(
        out.stdout,
        "preset: Clean\nprogram: 0\nsample_rate: 48000\nbuffer_size: 128\nprepared_frames: 128\nrevision: 7\ninput_peak_db: -120.0\noutput_peak_db: -3.5\nxruns: 2\ncallback_max_us: 310\ncallback_last_us: 95\nconnections:\n  Scarlett:capture_FL -> xguitare:in\n  xguitare:out_l -> Scarlett:playback_FL\n"
    );
    let out = cli(
        &["status"],
        reply(
            r#"{"ok":true,"preset":null,"program":null,"sample_rate":48000,"buffer_size":128,"xruns":0,"callback_max_us":0,"callback_last_us":0,"connections":[]}"#,
        ),
    );
    assert!(
        out.stdout.starts_with("preset: -\nprogram: -\n"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.ends_with("connections:\n"), "{}", out.stdout);
}

#[test]
fn get_format() {
    let out = cli(
        &["get", "gain"],
        reply(
            r#"{"ok":true,"params":[{"module":"gain","param":"level","value":-3.5,"unit":"dB","min":-60.0,"max":20.0},{"module":"delay","param":"mix","value":25.0,"unit":"%","min":0.0,"max":100.0},{"module":"preamp","param":"bright","value":1.0,"unit":"","min":0.0,"max":1.0}]}"#,
        ),
    );
    assert_eq!(
        out.requests,
        [Request::Get {
            param: Some("gain".to_string())
        }]
    );
    assert_eq!(
        out.stdout,
        "gain.level = -3.5 dB\ndelay.mix = 25 %\npreamp.bright = 1\n"
    );
    let out = cli(&["get"], reply(r#"{"ok":true,"params":[]}"#));
    assert_eq!(out.requests, [Request::Get { param: None }]);
}

#[test]
fn set_prints_clamped() {
    let out = cli(
        &["set", "gain.level", "50"],
        reply(r#"{"ok":true,"param":"gain.level","value":20.0,"clamped":true}"#),
    );
    assert_eq!(
        out.requests,
        [Request::Set {
            param: "gain.level".to_string(),
            value: 50.0
        }]
    );
    assert_eq!(out.stdout, "gain.level = 20 (clamped)\n");
    let out = cli(
        &["set", "gain.level", "-6"],
        reply(r#"{"ok":true,"param":"gain.level","value":-6.0,"clamped":false}"#),
    );
    assert_eq!(
        out.requests,
        [Request::Set {
            param: "gain.level".to_string(),
            value: -6.0
        }]
    );
    assert_eq!(out.stdout, "gain.level = -6\n");
}

#[test]
fn set_non_numeric_value() {
    let out = cli(&["set", "gain.level", "loud"], reply(r#"{"ok":true}"#));
    assert_eq!(out.code, 1);
    assert_eq!(out.stderr, "error: value must be a number\n");
    assert!(out.requests.is_empty());
    assert!(out.stdout.is_empty());
}

#[test]
fn load_prints() {
    let out = cli(
        &["load", "Loud"],
        reply(r#"{"ok":true,"program":1,"name":"Loud"}"#),
    );
    assert_eq!(
        out.requests,
        [Request::Load {
            preset: "Loud".to_string()
        }]
    );
    assert_eq!(out.stdout, "loaded 1 Loud\n");
    assert_eq!(out.code, 0);
}

#[test]
fn error_response_exit_1() {
    let out = cli(
        &["set", "drive.gain", "1"],
        reply(r#"{"ok":false,"error":"no module 'drive'"}"#),
    );
    assert_eq!(out.code, 1);
    assert_eq!(out.stderr, "error: no module 'drive'\n");
    assert!(out.stdout.is_empty());
}

#[test]
fn daemon_not_running() {
    let out = cli(
        &["status"],
        Err(ClientError::NotRunning(PathBuf::from(
            "/run/user/1000/xguitare.sock",
        ))),
    );
    assert_eq!(out.code, 1);
    assert_eq!(
        out.stderr,
        "error: daemon not running (socket /run/user/1000/xguitare.sock)\n"
    );
}

#[test]
fn run_main_usage_error_exit_1() {
    assert_eq!(run_main(["xguitare", "bogus"]), 1);
}

#[test]
fn run_main_missing_config_is_error() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_xguitare"))
        .args(["--config", "/nonexistent/x.toml", "status"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: /nonexistent/x.toml: not found\n"
    );
}

#[test]
fn run_main_help_exit_0() {
    assert_eq!(run_main(["xguitare", "--help"]), 0);
}

#[test]
fn run_main_no_args_is_usage_error() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_xguitare"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr
            .lines()
            .next()
            .unwrap_or_default()
            .starts_with("error:"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn save_prints_path() {
    let out = cli(
        &["save"],
        reply(r#"{"ok":true,"path":"/bank/01-clean.toml"}"#),
    );
    assert_eq!(out.requests, [Request::Save]);
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "saved /bank/01-clean.toml\n");
}

#[test]
fn control_chars_in_names_are_replaced() {
    let out = cli(
        &["list"],
        reply(
            r#"{"ok":true,"active":0,"entries":[{"program":0,"name":"Evil\u001b]0;x\u0007","valid":true}]}"#,
        ),
    );
    assert_eq!(out.stdout, " 0* Evil\u{fffd}]0;x\u{fffd}\n");
    let out = cli(
        &["load", "x"],
        reply(r#"{"ok":false,"error":"no preset 'x\u001b[2J\u007f'"}"#),
    );
    assert_eq!(out.stderr, "error: no preset 'x\u{fffd}[2J\u{fffd}'\n");
}

#[test]
fn run_main_missing_args_names_them() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_xguitare"))
        .arg("set")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.contains("<PARAM>") && stderr.contains("<VALUE>"),
        "{stderr}"
    );
}
