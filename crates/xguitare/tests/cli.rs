use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use xguitare::{CliEnv, run_cli, run_main};
use xguitare_client::{ClientError, Transport};
use xguitare_protocol::{
    BusRef, Position, RenderBits, RenderRange, Request, Response, SendLevel, TrackRef,
    TransportAction,
};

struct Canned {
    replies: VecDeque<Result<Response, ClientError>>,
    requests: Vec<Request>,
    stop_after_tuner_requests: Option<(usize, Arc<AtomicBool>)>,
}

impl Transport for Canned {
    fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        self.requests.push(req.clone());
        if let Some((count, stop)) = &self.stop_after_tuner_requests {
            let tuner_requests = self
                .requests
                .iter()
                .filter(|req| **req == Request::Tuner)
                .count();
            if tuner_requests == *count {
                stop.store(true, Ordering::Relaxed);
            }
        }
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
    let env = CliEnv {
        tuner_mute: false,
        is_tty: false,
        stop: Arc::new(AtomicBool::new(false)),
    };
    cli_with(argv, vec![reply], &env, None)
}

fn cli_with(
    argv: &[&str],
    replies: Vec<Result<Response, ClientError>>,
    env: &CliEnv,
    stop_after_tuner_requests: Option<usize>,
) -> Outcome {
    let argv: Vec<String> = std::iter::once("xguitare")
        .chain(argv.iter().copied())
        .map(String::from)
        .collect();
    let mut transport = Canned {
        replies: VecDeque::from(replies),
        requests: Vec::new(),
        stop_after_tuner_requests: stop_after_tuner_requests
            .map(|count| (count, Arc::clone(&env.stop))),
    };
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run_cli(&argv, &mut transport, &mut stdout, &mut stderr, env);
    Outcome {
        code,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
        requests: transport.requests,
    }
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "a canned transport reply is a Result"
)]
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
            r#"{"ok":true,"preset":"Clean","program":0,"modified":true,"sample_rate":48000,"buffer_size":128,"prepared_frames":128,"revision":7,"muted":false,"input_peak_db":-120.0,"output_peak_db":-3.5,"xruns":2,"callback_max_us":310,"callback_last_us":95,"protocol_version":1,"connections":[{"from":"Scarlett:capture_FL","to":"xguitare:in"},{"from":"xguitare:out_l","to":"Scarlett:playback_FL"}],"disk_overflows":0,"metronome":{"on":false,"bpm":120.0,"meter":[4,4],"volume_db":-12.0,"beats_played":0,"beat":0},"project":null}"#,
        ),
    );
    assert_eq!(out.requests, [Request::Status]);
    assert_eq!(
        out.stdout,
        "preset: Clean\nprogram: 0\nmodified: yes\nsample_rate: 48000\nbuffer_size: 128\nprepared_frames: 128\nrevision: 7\nmuted: false\ninput_peak_db: -120.0\noutput_peak_db: -3.5\nxruns: 2\ncallback_max_us: 310\ncallback_last_us: 95\nprotocol_version: 1\nconnections:\n  Scarlett:capture_FL -> xguitare:in\n  xguitare:out_l -> Scarlett:playback_FL\ndisk_overflows: 0\nmetronome: off, 120 bpm 4/4, volume -12 dB, beat 0\nproject: -\n"
    );
    let out = cli(
        &["status"],
        reply(
            r#"{"ok":true,"preset":null,"program":null,"modified":false,"sample_rate":48000,"buffer_size":128,"prepared_frames":128,"revision":1,"muted":false,"input_peak_db":-120.0,"output_peak_db":-120.0,"xruns":0,"callback_max_us":0,"callback_last_us":0,"protocol_version":1,"connections":[],"disk_overflows":0,"metronome":{"on":false,"bpm":120.0,"meter":[4,4],"volume_db":-12.0,"beats_played":0,"beat":0},"project":null}"#,
        ),
    );
    assert!(
        out.stdout
            .starts_with("preset: -\nprogram: -\nmodified: no\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .ends_with("connections:\ndisk_overflows: 0\nmetronome: off, 120 bpm 4/4, volume -12 dB, beat 0\nproject: -\n"),
        "{}",
        out.stdout
    );
}

#[test]
fn get_format() {
    let out = cli(
        &["get", "gain"],
        reply(
            r#"{"ok":true,"params":[{"module":"gain","type":"gain","param":"level","value":-3.5,"unit":"dB","min":-60.0,"max":20.0,"default":0.0,"integer":false,"labels":[],"bypass":false},{"module":"delay","type":"delay","param":"mix","value":25.0,"unit":"%","min":0.0,"max":100.0,"default":25.0,"integer":false,"labels":[],"bypass":false},{"module":"preamp","type":"preamp","param":"bright","value":1.0,"unit":"","min":0.0,"max":1.0,"default":0.0,"integer":true,"labels":[],"bypass":false}]}"#,
        ),
    );
    assert_eq!(
        out.requests,
        [Request::Get {
            param: Some("gain".to_string()),
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(
        out.stdout,
        "gain.level = -3.5 dB\ngain.bypass = no\ndelay.mix = 25 %\ndelay.bypass = no\npreamp.bright = 1\npreamp.bypass = no\n"
    );
    let out = cli(&["get"], reply(r#"{"ok":true,"params":[]}"#));
    assert_eq!(
        out.requests,
        [Request::Get {
            param: None,
            track: None,
            bus: None,
            master: false,
        }]
    );
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
            value: 50.0,
            track: None,
            bus: None,
            master: false,
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
            value: -6.0,
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(out.stdout, "gain.level = -6\n");
}

#[test]
fn set_non_numeric_value() {
    let out = cli(&["set", "gain.level", "loud"], reply(r#"{"ok":true}"#));
    assert_eq!(out.code, 1);
    assert_eq!(out.stderr, "error: value must be a number\n");
    assert_eq!(out.requests, []);
    assert_eq!(out.stdout, "");
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
            preset: "Loud".to_string(),
            track: None,
            bus: None,
            master: false,
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
    assert_eq!(out.stdout, "");
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
    assert_eq!(output.stdout, Vec::<u8>::new());
}

#[test]
fn save_prints_path() {
    let out = cli(
        &["save"],
        reply(r#"{"ok":true,"path":"/bank/01-clean.toml"}"#),
    );
    assert_eq!(out.requests, [Request::Save { name: None }]);
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "saved /bank/01-clean.toml\n");
}

#[test]
fn cli_save_as() {
    let out = cli(
        &["save", "--as", "My Tone"],
        reply(r#"{"ok":true,"path":"/bank/04-my-tone.toml","program":3}"#),
    );
    assert_eq!(
        out.requests,
        [Request::Save {
            name: Some("My Tone".to_string())
        }]
    );
    assert_eq!(
        out.requests[0].to_line(),
        r#"{"cmd":"save","as":"My Tone"}"#
    );
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "saved /bank/04-my-tone.toml\n");
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

#[test]
fn modules_prints_types() {
    let out = cli(
        &["modules"],
        reply(
            r#"{"ok":true,"modules":[{"type":"gain","channels":"mono","params":[{"name":"level","min":-60.0,"max":20.0,"default":0.0,"unit":"dB","integer":false}]},{"type":"cabinet","channels":"mono_to_stereo","params":[]},{"type":"delay","channels":"stereo","params":[{"name":"time"},{"name":"feedback"},{"name":"mix"}]},{"type":"autowah","channels":"mono","params":[{"name":"mode","labels":["lp","bp","hp"]},{"name":"direction","labels":["up","down"]},{"name":"q","labels":[]}]}]}"#,
        ),
    );
    assert_eq!(out.requests, [Request::Modules]);
    assert_eq!(
        out.stdout,
        "gain        mono            level\ncabinet     mono_to_stereo  \ndelay       stereo          time,feedback,mix\nautowah     mono            mode (lp|bp|hp),direction (up|down),q\n"
    );
    assert_eq!(out.code, 0);
}

#[test]
fn rack_add_prints() {
    let out = cli(
        &["rack", "add", "delay", "--after", "cabinet"],
        reply(r#"{"ok":true,"id":"delay-2"}"#),
    );
    assert_eq!(
        out.requests,
        [Request::RackAdd {
            r#type: "delay".to_string(),
            after: Some("cabinet".to_string()),
            at: None,
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(out.stdout, "added delay-2\n");
    let out = cli(
        &["rack", "add", "gate"],
        reply(r#"{"ok":true,"id":"gate"}"#),
    );
    assert_eq!(
        out.requests,
        [Request::RackAdd {
            r#type: "gate".to_string(),
            after: None,
            at: None,
            track: None,
            bus: None,
            master: false,
        }]
    );
    let out = cli(
        &["rack", "add", "gate", "--at", "0"],
        reply(r#"{"ok":true,"id":"gate"}"#),
    );
    assert_eq!(
        out.requests,
        [Request::RackAdd {
            r#type: "gate".to_string(),
            after: None,
            at: Some(0),
            track: None,
            bus: None,
            master: false,
        }]
    );
    let out = cli(
        &["rack", "add", "delay"],
        reply(r#"{"ok":false,"error":"stereo module 'delay' before the cabinet"}"#),
    );
    assert_eq!(
        out.stderr,
        "error: stereo module 'delay' before the cabinet\n"
    );
    assert_eq!(out.code, 1);
}

#[test]
fn rack_move_prints() {
    let out = cli(
        &["rack", "move", "delay", "6"],
        reply(r#"{"ok":true,"id":"delay","to":4}"#),
    );
    assert_eq!(
        out.requests,
        [Request::RackMove {
            id: "delay".to_string(),
            to: 6,
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(out.stdout, "moved delay to 4\n");
    assert_eq!(out.code, 0);
}

#[test]
fn rack_remove_prints() {
    let out = cli(
        &["rack", "remove", "delay"],
        reply(r#"{"ok":true,"id":"delay"}"#),
    );
    assert_eq!(
        out.requests,
        [Request::RackRemove {
            id: "delay".to_string(),
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(out.stdout, "removed delay\n");
    assert_eq!(out.code, 0);
}

fn watch_env(tuner_mute: bool) -> CliEnv {
    CliEnv {
        tuner_mute,
        is_tty: false,
        stop: Arc::new(AtomicBool::new(false)),
    }
}

const E1_READING: &str = r#"{"ok":true,"hz":41.2,"note":"E1","cents":-3,"confident":true}"#;

#[test]
fn tuner_prints_reading() {
    let out = cli(&["tuner"], reply(E1_READING));
    assert_eq!(out.requests, [Request::Tuner]);
    assert_eq!(out.stdout, "E1  41.2 Hz -3 ct\n");
    assert_eq!(out.code, 0);
    let out = cli(
        &["tuner"],
        reply(r#"{"ok":true,"hz":466.2,"note":"A#4","cents":0,"confident":true}"#),
    );
    assert_eq!(out.stdout, "A#4 466.2 Hz +0 ct\n");
}

#[test]
fn tuner_prints_no_signal() {
    let out = cli(
        &["tuner"],
        reply(r#"{"ok":true,"hz":null,"note":null,"cents":null,"confident":false}"#),
    );
    assert_eq!(out.stdout, "no signal\n");
    assert_eq!(out.code, 0);
}

#[test]
fn tuner_watch_two_readings_mutes_around() {
    let out = cli_with(
        &["tuner", "--watch"],
        vec![
            reply(r#"{"ok":true,"muted":true}"#),
            reply(E1_READING),
            reply(r#"{"ok":true,"hz":null,"note":null,"cents":null,"confident":false}"#),
            reply(r#"{"ok":true,"muted":false}"#),
        ],
        &watch_env(true),
        Some(2),
    );
    assert_eq!(
        out.requests,
        [
            Request::Mute { on: true },
            Request::Tuner,
            Request::Tuner,
            Request::Mute { on: false },
        ]
    );
    assert_eq!(out.stdout, "E1  41.2 Hz -3 ct\nno signal\n");
    assert_eq!(out.code, 0);
}

#[test]
fn tuner_watch_without_tuner_mute_sends_no_mute() {
    let out = cli_with(
        &["tuner", "--watch"],
        vec![reply(E1_READING), reply(E1_READING)],
        &watch_env(false),
        Some(2),
    );
    assert_eq!(out.requests, [Request::Tuner, Request::Tuner]);
    assert_eq!(out.stdout, "E1  41.2 Hz -3 ct\nE1  41.2 Hz -3 ct\n");
    assert_eq!(out.code, 0);
}

#[test]
fn mute_on_off_prints() {
    let out = cli(&["mute", "on"], reply(r#"{"ok":true,"muted":true}"#));
    assert_eq!(out.requests, [Request::Mute { on: true }]);
    assert_eq!(out.stdout, "muted\n");
    assert_eq!(out.code, 0);
    let out = cli(&["mute", "off"], reply(r#"{"ok":true,"muted":false}"#));
    assert_eq!(out.requests, [Request::Mute { on: false }]);
    assert_eq!(out.stdout, "unmuted\n");
    assert_eq!(out.code, 0);
}

#[test]
fn mute_bad_value_is_usage_error() {
    let out = cli(&["mute", "maybe"], reply(r#"{"ok":true}"#));
    assert_eq!(out.code, 1);
    assert_eq!(out.requests, []);
    assert_eq!(out.stderr.lines().count(), 1, "{}", out.stderr);
    assert!(out.stderr.starts_with("error:"), "{}", out.stderr);
}

#[test]
fn status_shows_muted() {
    let out = cli(
        &["status"],
        reply(
            r#"{"ok":true,"preset":"Clean","program":0,"sample_rate":48000,"buffer_size":128,"prepared_frames":128,"revision":3,"muted":true,"input_peak_db":-120.0,"output_peak_db":-120.0,"xruns":0,"callback_max_us":0,"callback_last_us":0,"connections":[]}"#,
        ),
    );
    assert!(
        out.stdout
            .contains("revision: 3\nmuted: true\ninput_peak_db"),
        "{}",
        out.stdout
    );
}

#[test]
fn tuner_watch_unmutes_after_error() {
    let out = cli_with(
        &["tuner", "--watch"],
        vec![
            reply(r#"{"ok":true,"muted":true}"#),
            reply(E1_READING),
            Err(ClientError::Protocol("boom".into())),
            reply(r#"{"ok":true,"muted":false}"#),
        ],
        &watch_env(true),
        None,
    );
    assert_eq!(out.requests.last(), Some(&Request::Mute { on: false }));
    assert_eq!(out.code, 1);
}

#[test]
fn ir_list_prints_names() {
    let out = cli(
        &["ir", "list"],
        reply(r#"{"ok":true,"ir_dir":"/srv/irs","irs":["bad.wav","cabs/x.wav"]}"#),
    );
    assert_eq!(out.requests, [Request::IrList]);
    assert_eq!(out.stdout, "bad.wav\ncabs/x.wav\n");
    assert_eq!(out.code, 0);
}

#[test]
fn ir_list_empty_prints_nothing() {
    let out = cli(
        &["ir", "list"],
        reply(r#"{"ok":true,"ir_dir":"/srv/irs","irs":[]}"#),
    );
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, "");
    assert_eq!(out.code, 0);
}

#[test]
fn ir_set_prints() {
    let out = cli(
        &["ir", "set", "cabs/x.wav"],
        reply(r#"{"ok":true,"id":"cabinet","ir":"cabs/x.wav"}"#),
    );
    assert_eq!(
        out.requests,
        [Request::RackIr {
            ir: "cabs/x.wav".to_string(),
            id: None,
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(out.stdout, "ir cabinet cabs/x.wav\n");
    assert_eq!(out.code, 0);
    let out = cli(
        &["ir", "set", "cabs/x.wav", "--id", "cab-2"],
        reply(r#"{"ok":true,"id":"cab-2","ir":"cabs/x.wav"}"#),
    );
    assert_eq!(
        out.requests,
        [Request::RackIr {
            ir: "cabs/x.wav".to_string(),
            id: Some("cab-2".to_string()),
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(out.stdout, "ir cab-2 cabs/x.wav\n");
}

#[test]
fn ir_set_error() {
    let out = cli(
        &["ir", "set", "bad.wav"],
        reply(r#"{"ok":false,"error":"/srv/irs/bad.wav: unsupported wav format: 8-bit int"}"#),
    );
    assert_eq!(
        out.stderr,
        "error: /srv/irs/bad.wav: unsupported wav format: 8-bit int\n"
    );
    assert_eq!(out.stdout, "");
    assert_eq!(out.code, 1);
}

#[test]
fn ir_alone_is_usage_error() {
    let out = cli(&["ir"], reply(r#"{"ok":true}"#));
    assert_eq!(out.code, 1);
    assert_eq!(out.stderr.lines().count(), 1, "{}", out.stderr);
    assert!(out.stderr.starts_with("error:"), "{}", out.stderr);
    assert_eq!(out.requests, []);
    assert_eq!(out.stdout, "");
}

#[test]
fn get_cabinet_prints_ir() {
    let out = cli(
        &["get", "cabinet"],
        reply(
            r#"{"ok":true,"params":[{"module":"cabinet","type":"cabinet","param":"level","value":0.0,"unit":"dB","min":-20.0,"max":20.0,"default":0.0,"integer":false,"labels":[],"bypass":false,"ir":"celestion/v30.wav"},{"module":"cabinet","type":"cabinet","param":"low_cut","value":80.0,"unit":"Hz","min":20.0,"max":500.0,"default":80.0,"integer":false,"labels":[],"bypass":false,"ir":"celestion/v30.wav"},{"module":"cab-2","type":"cabinet","param":"level","value":-1.0,"unit":"dB","min":-20.0,"max":20.0,"default":0.0,"integer":false,"labels":[],"bypass":false,"ir":null}]}"#,
        ),
    );
    assert_eq!(
        out.stdout,
        "cabinet.level = 0 dB\ncabinet.low_cut = 80 Hz\ncabinet.ir = celestion/v30.wav\ncabinet.bypass = no\ncab-2.level = -1 dB\ncab-2.ir = none\ncab-2.bypass = no\n"
    );
}

#[test]
fn get_prints_bypass_after_each_module() {
    let out = cli(
        &["get"],
        reply(
            r#"{"ok":true,"params":[{"module":"gain","param":"level","value":-3.5,"unit":"dB","bypass":false},{"module":"cabinet","type":"cabinet","param":"level","value":0.0,"unit":"dB","bypass":true,"ir":null}]}"#,
        ),
    );
    assert_eq!(
        out.stdout,
        "gain.level = -3.5 dB\ngain.bypass = no\ncabinet.level = 0 dB\ncabinet.ir = none\ncabinet.bypass = yes\n"
    );
    let out = cli(
        &["get", "gain.level"],
        reply(
            r#"{"ok":true,"params":[{"module":"gain","param":"level","value":-3.5,"unit":"dB","bypass":false}]}"#,
        ),
    );
    assert_eq!(out.stdout, "gain.level = -3.5 dB\n");
}

#[test]
fn get_cabinet_param_prints_no_ir() {
    let out = cli(
        &["get", "cabinet.level"],
        reply(
            r#"{"ok":true,"params":[{"module":"cabinet","type":"cabinet","param":"level","value":0.0,"unit":"dB","ir":"celestion/v30.wav"}]}"#,
        ),
    );
    assert_eq!(out.stdout, "cabinet.level = 0 dB\n");
}

#[test]
fn rack_alone_is_error_line() {
    let out = cli(&["rack"], reply(r#"{"ok":true}"#));
    assert_eq!(out.code, 1);
    assert_eq!(out.stderr.lines().count(), 1, "{}", out.stderr);
    assert!(out.stderr.starts_with("error:"), "{}", out.stderr);
    assert_eq!(out.requests, []);
    assert_eq!(out.stdout, "");
}

#[test]
fn bypass_on_off_prints() {
    let out = cli(
        &["bypass", "preamp", "on"],
        reply(r#"{"ok":true,"bypass":true}"#),
    );
    assert_eq!(
        out.requests,
        [Request::Bypass {
            id: "preamp".to_string(),
            on: true,
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(out.stdout, "preamp: bypassed\n");
    assert_eq!(out.code, 0);
    let out = cli(
        &["bypass", "preamp", "off"],
        reply(r#"{"ok":true,"bypass":false}"#),
    );
    assert_eq!(
        out.requests,
        [Request::Bypass {
            id: "preamp".to_string(),
            on: false,
            track: None,
            bus: None,
            master: false,
        }]
    );
    assert_eq!(out.stdout, "preamp: on\n");
    assert_eq!(out.code, 0);
}

#[test]
fn bypass_unknown_module_prints_error() {
    let out = cli(
        &["bypass", "wah", "on"],
        reply(r#"{"ok":false,"error":"no module 'wah'"}"#),
    );
    assert_eq!(out.stderr, "error: no module 'wah'\n");
    assert_eq!(out.code, 1);
}

#[test]
fn status_prints_the_open_project() {
    let out = cli(
        &["status"],
        reply(
            r#"{"ok":true,"preset":"Clean","protocol_version":1,"connections":[],"disk_overflows":1,"metronome":{"on":false,"bpm":120.0,"meter":[4,4],"volume_db":-12.0,"beats_played":0,"beat":0},"project":"Song","transport":"recording","position":{"frames":216000,"bars_beats":"3.2"},"tempo":{"bpm":120.0,"meter":[3,4]},"loop":{"start":0,"end":96000,"on":true},"click":{"on":true,"count_in":1},"selected":1,"undo":"trim","redo":null,"tracks":[{"id":0,"name":"Guitar","input":"card:capture_1","volume_db":-6.0,"pan":0.0,"mute":false,"solo":true,"armed":true,"input_peak_db":-42.04,"to_master":true,"sends":[{"bus":1,"level_db":-6.0,"pre":false},{"bus":0,"level_db":-12.5,"pre":true}],"peak_db":[-12.04,-12.0],"clip":false},{"id":1,"name":"Bass","input":null,"volume_db":0.0,"pan":-0.5,"mute":true,"solo":false,"armed":false,"input_peak_db":-120.0,"to_master":false,"sends":[],"peak_db":[-120.0,-120.0],"clip":true}],"buses":[{"id":3,"name":"Verb","volume_db":-3.0,"pan":0.0,"mute":false,"solo":true,"peak_db":[-20.0,-21.5],"clip":false},{"id":1,"name":"Delay","volume_db":0.0,"pan":0.25,"mute":false,"solo":false,"peak_db":[0.0,-0.4],"clip":true}],"master":{"volume_db":-1.5,"peak_db":[-9.0,-9.24],"clip":false}}"#,
        ),
    );
    assert!(
        out.stdout.ends_with(
            "connections:\ndisk_overflows: 1\nmetronome: off, 120 bpm 4/4, volume -12 dB, beat 0\nproject: Song\ntransport: recording\nposition: 3.2 (frame 216000)\ntempo: 120 bpm 3/4\nloop: on 0..96000\nclick on, count-in 1 bars\nundo: trim, redo: -\ntracks:\n  0  Guitar: armed, solo; input card:capture_1; -6 dB, pan 0; peak -42.0 dB; out -12.0/-12.0 dB\n      send Delay: -6 dB, post\n      send Verb: -12.5 dB, pre\n  1* Bass: muted, not to master; input -; 0 dB, pan -0.5; peak -120.0 dB; out -120.0/-120.0 dB, clip\nbuses:\n  0 Verb: solo; -3 dB, pan 0; out -20.0/-21.5 dB\n  1 Delay: -; 0 dB, pan 0.25; out 0.0/-0.4 dB, clip\nmaster: -1.5 dB; out -9.0/-9.2 dB\n"
        ),
        "{}",
        out.stdout
    );
}

#[test]
fn project_commands_send_their_requests_and_print_the_answer() {
    let cases: [(&[&str], Request, &str, &str); 8] = [
        (
            &["project", "new", "My Song"],
            Request::ProjectNew {
                name: "My Song".to_string(),
            },
            r#"{"ok":true,"name":"My Song","path":"/p/my-song"}"#,
            "created My Song at /p/my-song\n",
        ),
        (
            &["project", "open", "song"],
            Request::ProjectOpen {
                name: "song".to_string(),
                restore: false,
            },
            r#"{"ok":true,"name":"Song","path":"/p/song","autosave_newer":true,"restored":false}"#,
            "opened Song\nits autosave is newer; open it with --restore to load the autosave\n",
        ),
        (
            &["project", "open", "song", "--restore"],
            Request::ProjectOpen {
                name: "song".to_string(),
                restore: true,
            },
            r#"{"ok":true,"name":"Song","path":"/p/song","autosave_newer":true,"restored":true}"#,
            "opened Song from its autosave\n",
        ),
        (
            &["project", "save"],
            Request::ProjectSave,
            r#"{"ok":true,"path":"/p/song/project.toml"}"#,
            "saved /p/song/project.toml\n",
        ),
        (
            &["project", "close"],
            Request::ProjectClose,
            r#"{"ok":true,"name":"Song","path":"/p/song"}"#,
            "closed Song\n",
        ),
        (
            &["project", "list"],
            Request::ProjectList,
            r#"{"ok":true,"projects":[{"name":"A Song","folder":"a-song","valid":true},{"name":"bad","folder":"bad","valid":false,"error":"line 1:8: invalid string"}]}"#,
            "A Song\nbad! (line 1:8: invalid string)\n",
        ),
        (
            &["project", "undo"],
            Request::ProjectUndo,
            r#"{"ok":true,"label":"move 2 Items"}"#,
            "undid move 2 Items\n",
        ),
        (
            &["project", "redo"],
            Request::ProjectRedo,
            r#"{"ok":true,"label":"record Bass DI"}"#,
            "redid record Bass DI\n",
        ),
    ];
    for (argv, request, answer, printed) in cases {
        let out = cli(argv, reply(answer));
        assert_eq!(out.requests, [request], "{argv:?}");
        assert_eq!(out.stdout, printed, "{argv:?}");
        assert_eq!(out.code, 0);
    }
}

#[test]
fn project_open_prints_each_load_warning_on_stderr() {
    let out = cli(
        &["project", "open", "song"],
        reply(
            r#"{"ok":true,"name":"Song","path":"/p/song","autosave_newer":false,"restored":false,"warnings":["track 'Gtr': item 1 now plays takes/b.wav: takes/c.wav ends before the item starts","track 'Gtr': dropped item 4 (takes/d.wav): starts past the end of its take"]}"#,
        ),
    );
    assert_eq!(out.stdout, "opened Song\n");
    assert_eq!(
        out.stderr,
        "warning: track 'Gtr': item 1 now plays takes/b.wav: takes/c.wav ends before the item starts\nwarning: track 'Gtr': dropped item 4 (takes/d.wav): starts past the end of its take\n"
    );
    assert_eq!(out.code, 0);
}

#[test]
fn track_commands_take_an_index_or_a_name() {
    let cases: [(&[&str], Request, &str, &str); 5] = [
        (
            &["track", "add", "Guitar"],
            Request::TrackAdd {
                name: Some("Guitar".to_string()),
            },
            r#"{"ok":true,"index":0,"name":"Guitar"}"#,
            "added track 0 Guitar\n",
        ),
        (
            &["track", "remove", "1"],
            Request::TrackRemove {
                track: TrackRef::Index(1),
            },
            r#"{"ok":true,"index":1,"name":"Bass"}"#,
            "removed track 1 Bass\n",
        ),
        (
            &["track", "select", "Bass"],
            Request::TrackSelect {
                track: TrackRef::Name("Bass".to_string()),
            },
            r#"{"ok":true,"index":1,"name":"Bass"}"#,
            "selected track 1 Bass\n",
        ),
        (
            &[
                "track",
                "edit",
                "Guitar",
                "--input",
                "card:capture_1",
                "--volume",
                "-6",
                "--pan",
                "-0.5",
                "--mute",
                "off",
                "--solo",
                "on",
                "--arm",
                "on",
                "--name",
                "Lead",
                "--to-master",
                "off",
            ],
            Request::TrackSet {
                track: TrackRef::Name("Guitar".to_string()),
                input: Some("card:capture_1".to_string()),
                volume: Some(-6.0),
                pan: Some(-0.5),
                mute: Some(false),
                solo: Some(true),
                arm: Some(true),
                name: Some("Lead".to_string()),
                to_master: Some(false),
            },
            r#"{"ok":true,"index":0,"name":"Lead","input":"card:capture_1","volume_db":-6.0,"pan":-0.5,"mute":false,"solo":true,"arm":true,"to_master":false}"#,
            "track 0 Lead: input card:capture_1, volume -6 dB, pan -0.5, mute no, solo yes, arm yes, to master no\n",
        ),
        (
            &["track", "edit", "0", "--input", ""],
            Request::TrackSet {
                track: TrackRef::Index(0),
                input: Some(String::new()),
                volume: None,
                pan: None,
                mute: None,
                solo: None,
                arm: None,
                name: None,
                to_master: None,
            },
            r#"{"ok":true,"index":0,"name":"Lead","input":null,"volume_db":0.0,"pan":0.0,"mute":false,"solo":false,"arm":false,"to_master":true}"#,
            "track 0 Lead: input -, volume 0 dB, pan 0, mute no, solo no, arm no, to master yes\n",
        ),
    ];
    for (argv, request, answer, printed) in cases {
        let out = cli(argv, reply(answer));
        assert_eq!(out.requests, [request], "{argv:?}");
        assert_eq!(out.stdout, printed, "{argv:?}");
    }
}

#[test]
fn transport_tempo_and_click_commands() {
    let transport = |action, position: Option<&str>| Request::Transport {
        action,
        position: position.map(|text| Position::Text(text.to_string())),
    };
    let cases: [(&[&str], Request, &str, &str); 7] = [
        (
            &["play"],
            transport(TransportAction::Play, None),
            r#"{"ok":true,"action":"play"}"#,
            "playing\n",
        ),
        (
            &["stop"],
            transport(TransportAction::Stop, None),
            r#"{"ok":true,"action":"stop"}"#,
            "stopped\n",
        ),
        (
            &["record"],
            transport(TransportAction::Record, None),
            r#"{"ok":true,"action":"record"}"#,
            "recording\n",
        ),
        (
            &["locate", "3.2"],
            transport(TransportAction::Locate, Some("3.2")),
            r#"{"ok":true,"action":"locate","frames":216000}"#,
            "located at frame 216000\n",
        ),
        (
            &["tempo", "90.5", "3/4"],
            Request::Tempo {
                bpm: Some(90.5),
                meter: Some((3, 4)),
            },
            r#"{"ok":true,"bpm":90.5,"meter":[3,4]}"#,
            "tempo 90.5 bpm 3/4\n",
        ),
        (
            &["click", "on", "--count-in", "2"],
            Request::Click {
                on: Some(true),
                count_in: Some(2),
            },
            r#"{"ok":true,"on":true,"count_in":2}"#,
            "click on, count-in 2 bars\n",
        ),
        (
            &["click", "off"],
            Request::Click {
                on: Some(false),
                count_in: None,
            },
            r#"{"ok":true,"on":false,"count_in":0}"#,
            "click off, count-in 0 bars\n",
        ),
    ];
    for (argv, request, answer, printed) in cases {
        let out = cli(argv, reply(answer));
        assert_eq!(out.requests, [request], "{argv:?}");
        assert_eq!(out.stdout, printed, "{argv:?}");
    }
}

#[test]
fn project_errors_print_one_line() {
    let out = cli(
        &["track", "remove", "0"],
        reply(r#"{"ok":false,"error":"cannot remove a track while recording"}"#),
    );
    assert_eq!(out.stderr, "error: cannot remove a track while recording\n");
    assert_eq!(out.code, 1);
    let out = cli(
        &["project", "undo"],
        reply(r#"{"ok":false,"error":"nothing to undo"}"#),
    );
    assert_eq!(out.stderr, "error: nothing to undo\n");
    assert_eq!(out.code, 1);
    let out = cli_with(
        &["tempo", "120", "3-4"],
        Vec::new(),
        &CliEnv {
            tuner_mute: false,
            is_tty: false,
            stop: Arc::new(AtomicBool::new(false)),
        },
        None,
    );
    assert_eq!(out.stderr, "error: meter must be <beats>/<note>\n");
    assert_eq!(out.code, 1);
    assert_eq!(out.requests, Vec::<Request>::new());
}

#[test]
fn record_check_is_gone() {
    assert_eq!(run_main(["xguitare", "record-check", "/tmp/x"]), 1);
}

#[test]
fn project_items_without_sample_rate_is_an_error_line() {
    let out = cli(
        &["project", "items"],
        reply(
            r#"{"ok":true,"tracks":[{"id":0,"name":"Guitar","items":[{"id":1,"position":0,"bars_beats":"1.1","offset":0,"length":96000,"take":0,"takes":["takes/guitar-1.wav"]}]}]}"#,
        ),
    );
    assert_eq!(out.stderr, "error: the items answer has no sample_rate\n");
    assert_eq!(out.stdout, "");
    assert_eq!(out.code, 1);
}

#[test]
fn project_items_prints_one_line_per_item() {
    let out = cli(
        &["project", "items"],
        reply(
            r#"{"ok":true,"sample_rate":48000,"tracks":[{"id":0,"name":"Guitar","items":[{"id":4,"position":0,"bars_beats":"1.1","offset":50,"length":96000,"take":0,"takes":["takes/guitar-1.wav"]},{"id":9,"position":192000,"bars_beats":"3.1","offset":0,"length":24000,"take":1,"takes":["takes/guitar-2.wav","takes/guitar-3.wav"]}]},{"id":2,"name":"Bass","items":[]}]}"#,
        ),
    );
    assert_eq!(out.requests, [Request::Items]);
    assert_eq!(
        out.stdout,
        "4 Guitar 1.1 2.00s take 1/1\n9 Guitar 3.1 0.50s take 2/2\n"
    );
    assert_eq!(out.code, 0);
}

#[test]
fn bus_commands_send_their_requests_and_print_the_answer() {
    let bus = |name: &str| BusRef::Name(name.to_string());
    let cases: Vec<(&[&str], Request, &str, &str)> = vec![
        (
            &["bus", "add", "Verb"],
            Request::BusAdd {
                name: Some("Verb".to_string()),
            },
            r#"{"ok":true,"bus":{"id":0,"index":0,"name":"Verb"}}"#,
            "added bus 0 Verb\n",
        ),
        (
            &["bus", "add"],
            Request::BusAdd { name: None },
            r#"{"ok":true,"bus":{"id":1,"index":1,"name":"Bus 1"}}"#,
            "added bus 1 Bus 1\n",
        ),
        (
            &["bus", "remove", "1"],
            Request::BusRemove {
                bus: BusRef::Index(1),
            },
            r#"{"ok":true,"bus":{"id":1,"index":1,"name":"Bus 1"}}"#,
            "removed bus 1 Bus 1\n",
        ),
        (
            &[
                "bus", "edit", "Verb", "--name", "Room", "--volume", "-3", "--pan", "-0.5",
                "--mute", "on", "--solo", "off",
            ],
            Request::BusSet {
                bus: bus("Verb"),
                name: Some("Room".to_string()),
                volume: Some(-3.0),
                pan: Some(-0.5),
                mute: Some(true),
                solo: Some(false),
            },
            r#"{"ok":true,"bus":{"id":0,"index":0,"name":"Room","volume_db":-3.0,"pan":-0.5,"mute":true,"solo":false}}"#,
            "bus 0 Room: volume -3 dB, pan -0.5, mute yes, solo no\n",
        ),
    ];
    assert_cases(cases);
}

#[test]
fn send_master_and_clip_commands_send_their_requests_and_print_the_answer() {
    let bus = |name: &str| BusRef::Name(name.to_string());
    let cases: Vec<(&[&str], Request, &str, &str)> = vec![
        (
            &["send", "0", "Verb", "-6"],
            Request::SendSet {
                track: TrackRef::Index(0),
                bus: bus("Verb"),
                level: Some(SendLevel::Db(-6.0)),
                pre: None,
            },
            r#"{"ok":true,"track":0,"bus":0,"level":-6.0,"pre":false}"#,
            "send track 0 -> bus 0: -6 dB, post\n",
        ),
        (
            &["send", "Guitar", "1", "off", "--pre", "on"],
            Request::SendSet {
                track: TrackRef::Name("Guitar".to_string()),
                bus: BusRef::Index(1),
                level: Some(SendLevel::Off),
                pre: Some(true),
            },
            r#"{"ok":true,"track":0,"bus":1,"level":"off","pre":true}"#,
            "send track 0 -> bus 1: off, pre\n",
        ),
        (
            &["master", "-3"],
            Request::MasterSet {
                volume: SendLevel::Db(-3.0),
            },
            r#"{"ok":true,"volume_db":-3.0}"#,
            "master -3 dB\n",
        ),
        (
            &["master", "off"],
            Request::MasterSet {
                volume: SendLevel::Off,
            },
            r#"{"ok":true,"volume_db":null}"#,
            "master off\n",
        ),
        (
            &["clip", "reset"],
            Request::ClipReset {
                track: None,
                bus: None,
                master: false,
            },
            r#"{"ok":true}"#,
            "clip reset\n",
        ),
        (
            &["clip", "reset", "track", "Guitar"],
            Request::ClipReset {
                track: Some(TrackRef::Name("Guitar".to_string())),
                bus: None,
                master: false,
            },
            r#"{"ok":true,"track":0}"#,
            "clip reset\n",
        ),
        (
            &["clip", "reset", "bus", "0"],
            Request::ClipReset {
                track: None,
                bus: Some(BusRef::Index(0)),
                master: false,
            },
            r#"{"ok":true,"bus":0}"#,
            "clip reset\n",
        ),
        (
            &["clip", "reset", "master"],
            Request::ClipReset {
                track: None,
                bus: None,
                master: true,
            },
            r#"{"ok":true,"master":true}"#,
            "clip reset\n",
        ),
    ];
    assert_cases(cases);
}

fn assert_cases(cases: Vec<(&[&str], Request, &str, &str)>) {
    for (argv, request, answer, printed) in cases {
        let out = cli(argv, reply(answer));
        assert_eq!(out.requests, [request], "{argv:?}");
        assert_eq!(out.stdout, printed, "{argv:?}");
        assert_eq!(out.code, 0, "{argv:?}");
    }
}

#[test]
fn rack_commands_take_a_bus_or_master() {
    let verb = Some(BusRef::Name("Verb".to_string()));
    let cases: Vec<(&[&str], Request, &str)> = vec![
        (
            &["rack", "add", "reverb", "--bus", "Verb"],
            Request::RackAdd {
                r#type: "reverb".to_string(),
                after: None,
                at: None,
                track: None,
                bus: verb.clone(),
                master: false,
            },
            r#"{"ok":true,"id":"reverb"}"#,
        ),
        (
            &["rack", "remove", "gain", "--master"],
            Request::RackRemove {
                id: "gain".to_string(),
                track: None,
                bus: None,
                master: true,
            },
            r#"{"ok":true,"id":"gain"}"#,
        ),
        (
            &["rack", "move", "gain", "0", "--bus", "1"],
            Request::RackMove {
                id: "gain".to_string(),
                to: 0,
                track: None,
                bus: Some(BusRef::Index(1)),
                master: false,
            },
            r#"{"ok":true,"id":"gain","to":0}"#,
        ),
        (
            &["set", "reverb.mix", "-0.5", "--bus", "Verb"],
            Request::Set {
                param: "reverb.mix".to_string(),
                value: -0.5,
                track: None,
                bus: verb.clone(),
                master: false,
            },
            r#"{"ok":true,"param":"reverb.mix","value":0.0,"clamped":true}"#,
        ),
        (
            &["get", "--master"],
            Request::Get {
                param: None,
                track: None,
                bus: None,
                master: true,
            },
            r#"{"ok":true,"params":[]}"#,
        ),
        (
            &["bypass", "reverb", "on", "--bus", "Verb"],
            Request::Bypass {
                id: "reverb".to_string(),
                on: true,
                track: None,
                bus: verb,
                master: false,
            },
            r#"{"ok":true,"bypass":true}"#,
        ),
        (
            &["ir", "set", "cab.wav", "--master"],
            Request::RackIr {
                ir: "cab.wav".to_string(),
                id: None,
                track: None,
                bus: None,
                master: true,
            },
            r#"{"ok":false,"error":"a cabinet is not allowed on a Bus or Master"}"#,
        ),
    ];
    for (argv, request, answer) in cases {
        let out = cli(argv, reply(answer));
        assert_eq!(out.requests, [request], "{argv:?}");
    }
    let out = cli(
        &["ir", "set", "cab.wav", "--master"],
        reply(r#"{"ok":false,"error":"a cabinet is not allowed on a Bus or Master"}"#),
    );
    assert_eq!(
        out.stderr,
        "error: a cabinet is not allowed on a Bus or Master\n"
    );
}

#[test]
fn mixer_volume_and_pan_must_be_finite() {
    for (argv, message) in [
        (
            &["bus", "edit", "0", "--volume", "nan"][..],
            "volume must be between -60 and 6 dB",
        ),
        (
            &["bus", "edit", "0", "--pan", "inf"][..],
            "pan must be between -1 and 1",
        ),
        (
            &["track", "edit", "0", "--volume", "inf"][..],
            "volume must be between -60 and 6 dB",
        ),
        (
            &["track", "edit", "0", "--pan", "NaN"][..],
            "pan must be between -1 and 1",
        ),
    ] {
        let out = cli(argv, reply(r#"{"ok":true}"#));
        assert_eq!(out.code, 1, "{argv:?}");
        assert!(out.requests.is_empty(), "{argv:?}");
        assert!(
            out.stderr.starts_with("error: invalid value"),
            "{}",
            out.stderr
        );
        assert!(
            out.stderr.contains(&format!("': {message};")),
            "{}",
            out.stderr
        );
    }
}

#[test]
fn mixer_usage_errors_print_one_line() {
    for (argv, stderr) in [
        (
            &["send", "0", "0", "loud"][..],
            "error: level must be a number in dB or off\n",
        ),
        (
            &["clip", "reset", "master", "0"][..],
            "error: clip reset master takes no name\n",
        ),
        (
            &["master", "inf"][..],
            "error: master volume must be off or between -60 and 6 dB\n",
        ),
        (
            &["master", "loud"][..],
            "error: master volume must be off or between -60 and 6 dB\n",
        ),
        (
            &["clip", "reset", "bus"][..],
            "error: clip reset track and bus need an index or a name\n",
        ),
        (
            &["rack", "add", "gain", "--bus", "0", "--master"][..],
            "error: the argument '--bus <BUS>' cannot be used with '--master'\n",
        ),
    ] {
        let out = cli(argv, reply(r#"{"ok":true}"#));
        assert_eq!(out.stderr, stderr, "{argv:?}");
        assert_eq!(out.code, 1, "{argv:?}");
        assert!(out.requests.is_empty(), "{argv:?}");
    }
}

fn render_reply(state: &str, progress: f64, extra: &str) -> Result<Response, ClientError> {
    reply(&format!(
        r#"{{"ok":true,"render":{{"name":"song","range":"project","progress":{progress},"state":"{state}","file":"/p/song/renders/song.wav"{extra}}}}}"#
    ))
}

fn quiet_env() -> CliEnv {
    CliEnv {
        tuner_mute: false,
        is_tty: false,
        stop: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn render_waits_prints_each_ten_percent_and_the_file() {
    let out = cli_with(
        &["render", "--loop", "--bits", "32f", "--name", "song"],
        vec![
            reply(r#"{"ok":true,"file":"/p/song/renders/song.wav"}"#),
            render_reply("running", 0.05, ""),
            render_reply("running", 0.25, ""),
            render_reply("running", 0.29, ""),
            render_reply("running", 0.7, ""),
            render_reply("done", 1.0, ""),
        ],
        &quiet_env(),
        None,
    );
    assert_eq!(
        out.requests[0],
        Request::Render {
            range: Some(RenderRange::Loop),
            bits: Some(RenderBits::Float32),
            name: Some("song".to_string()),
        }
    );
    assert!(out.requests[1..].iter().all(|req| *req == Request::Status));
    assert_eq!(
        out.stdout,
        "rendering /p/song/renders/song.wav\n20%\n70%\nrendered /p/song/renders/song.wav\n"
    );
    assert_eq!(out.code, 0);
}

#[test]
fn render_failure_and_cancel_end_with_an_error_line() {
    for (state, extra, message) in [
        (
            "failed",
            r#","error":"cannot write x: disk full""#,
            "error: render failed: cannot write x: disk full\n",
        ),
        ("cancelled", "", "error: render cancelled\n"),
    ] {
        let out = cli_with(
            &["render"],
            vec![
                reply(r#"{"ok":true,"file":"/p/song/renders/song.wav"}"#),
                render_reply(state, 0.5, extra),
            ],
            &quiet_env(),
            None,
        );
        assert_eq!(
            out.requests[0],
            Request::Render {
                range: None,
                bits: None,
                name: None
            }
        );
        assert_eq!(out.stderr, message);
        assert_eq!(out.code, 1);
    }
    let refused = cli(
        &["render", "--bits", "16"],
        reply(r#"{"ok":false,"error":"nothing to render"}"#),
    );
    assert_eq!(
        refused.requests,
        [Request::Render {
            range: None,
            bits: Some(RenderBits::Pcm16),
            name: None
        }]
    );
    assert_eq!(refused.stderr, "error: nothing to render\n");
    assert_eq!(refused.code, 1);
    let cancel = cli(&["render", "cancel"], reply(r#"{"ok":true}"#));
    assert_eq!(cancel.requests, [Request::RenderCancel]);
    assert_eq!(cancel.stdout, "render cancelling\n");
    let usage = cli(&["render", "--bits", "8"], reply(r#"{"ok":true}"#));
    assert_eq!(usage.code, 1);
    assert!(
        usage
            .stderr
            .starts_with("error: invalid value '8' for '--bits <BITS>'"),
        "{}",
        usage.stderr
    );
}

#[test]
fn status_prints_the_last_render() {
    let out = cli(
        &["status"],
        reply(
            r#"{"ok":true,"project":null,"render":{"name":"song","range":"loop","progress":0.4,"state":"running","file":"/p/song/renders/song.wav"}}"#,
        ),
    );
    assert!(
        out.stdout
            .ends_with("project: -\nrender: song (loop) running 40% /p/song/renders/song.wav\n"),
        "{}",
        out.stdout
    );
}

fn metronome_request(
    on: Option<bool>,
    bpm: Option<f64>,
    meter: Option<(u32, u32)>,
    volume: Option<f64>,
) -> Request {
    Request::Metronome {
        on,
        bpm,
        meter,
        volume,
    }
}

#[test]
fn metronome_commands() {
    let state = r#"{"ok":true,"on":true,"bpm":90,"meter":[3,4],"volume_db":-12.0,"beat":0}"#;
    let cases: [(&[&str], Request, &str, &str); 6] = [
        (
            &["metronome", "on"],
            metronome_request(Some(true), None, None, None),
            state,
            "metronome on, 90 bpm 3/4, volume -12 dB\n",
        ),
        (
            &["metronome", "off"],
            metronome_request(Some(false), None, None, None),
            r#"{"ok":true,"on":false,"bpm":120.0,"meter":[4,4],"volume_db":-12.0,"beat":0}"#,
            "metronome off, 120 bpm 4/4, volume -12 dB\n",
        ),
        (
            &["metronome", "tempo", "90", "3/4"],
            metronome_request(None, Some(90.0), Some((3, 4)), None),
            state,
            "metronome on, 90 bpm 3/4, volume -12 dB\n",
        ),
        (
            &["metronome", "tempo", "97.5"],
            metronome_request(None, Some(97.5), None, None),
            r#"{"ok":true,"on":true,"bpm":97.5,"meter":[3,4],"volume_db":-12.0,"beat":2}"#,
            "metronome on, 97.5 bpm 3/4, volume -12 dB\n",
        ),
        (
            &["metronome", "volume", "-12"],
            metronome_request(None, None, None, Some(-12.0)),
            state,
            "metronome on, 90 bpm 3/4, volume -12 dB\n",
        ),
        (
            &["metronome", "tap"],
            Request::MetronomeTap,
            r#"{"ok":true,"bpm":null}"#,
            "tap\n",
        ),
    ];
    for (argv, request, line, expected) in cases {
        let out = cli(argv, reply(line));
        assert_eq!(out.requests, [request], "{argv:?}");
        assert_eq!(out.stdout, expected, "{argv:?}");
        assert_eq!(out.code, 0);
    }
    let out = cli(&["metronome", "tap"], reply(r#"{"ok":true,"bpm":120.0}"#));
    assert_eq!(out.stdout, "tap 120 bpm\n");
}

#[test]
fn metronome_errors_print_one_line() {
    let out = cli(
        &["metronome", "tempo", "500"],
        reply(r#"{"ok":false,"error":"bpm must be between 20 and 300"}"#),
    );
    assert_eq!(out.stderr, "error: bpm must be between 20 and 300\n");
    assert_eq!(out.code, 1);
    let out = cli(
        &["metronome", "tempo", "90", "3-4"],
        reply(r#"{"ok":true}"#),
    );
    assert_eq!(out.stderr, "error: meter must be <beats>/<note>\n");
    assert_eq!(out.requests, []);
    assert_eq!(out.code, 1);
}

#[test]
fn status_prints_the_metronome() {
    let out = cli(
        &["status"],
        reply(
            r#"{"ok":true,"preset":null,"program":null,"modified":false,"connections":[],"project":null,"metronome":{"on":true,"bpm":90,"meter":[3,4],"volume_db":-12.0,"beat":2}}"#,
        ),
    );
    assert!(
        out.stdout
            .ends_with("metronome: on, 90 bpm 3/4, volume -12 dB, beat 2\nproject: -\n"),
        "{}",
        out.stdout
    );
}

fn item_cli(argv: &[&str], replies: Vec<Result<Response, ClientError>>) -> Outcome {
    let env = CliEnv {
        tuner_mute: false,
        is_tty: false,
        stop: Arc::new(AtomicBool::new(false)),
    };
    cli_with(argv, replies, &env, None)
}

/// The `items` and `status` answers an Item command reads its timing from: 48 kHz, 120 bpm in 4/4, so a beat is 24 000 frames.
fn timing_replies() -> Vec<Result<Response, ClientError>> {
    vec![
        reply(r#"{"ok":true,"sample_rate":48000,"tracks":[]}"#),
        reply(r#"{"ok":true,"tempo":{"bpm":120.0,"meter":[4,4]}}"#),
    ]
}

const MOVED: &str = r#"{"ok":true,"items":[{"track":1,"id":4,"position":24000,"bars_beats":"1.2","offset":0,"length":96000,"take":0,"takes":["takes/a.wav","takes/b.wav"]}]}"#;

#[test]
fn item_list_prints_one_line_per_item() {
    let out = cli(
        &["item", "list"],
        reply(
            r#"{"ok":true,"sample_rate":48000,"tracks":[{"id":0,"name":"Guitar","items":[{"id":4,"position":0,"bars_beats":"1.1","offset":50,"length":96000,"take":0,"takes":["takes/guitar-1.wav"]}]}]}"#,
        ),
    );
    assert_eq!(out.requests, [Request::Items]);
    assert_eq!(out.stdout, "4 Guitar 1.1 2.00s take 1/1\n");
}

#[test]
fn item_move_sends_the_delta_in_frames_and_prints_the_moved_items() {
    let cases: [(&[&str], i64, Option<TrackRef>); 3] = [
        (
            &["item", "move", "--track", "Bass", "4", "9", "-1.5s"],
            -72_000,
            Some(TrackRef::Name("Bass".to_string())),
        ),
        (&["item", "move", "4", "9", "+1.2"], 144_000, None),
        (
            &["item", "move", "--track", "1", "4", "9", "0.1"],
            24_000,
            Some(TrackRef::Index(1)),
        ),
    ];
    for (argv, delta, track) in cases {
        let mut replies = timing_replies();
        replies.push(reply(MOVED));
        let out = item_cli(argv, replies);
        assert_eq!(out.stderr, "", "{argv:?}");
        assert_eq!(
            out.requests,
            [
                Request::Items,
                Request::Status,
                Request::ItemMove {
                    ids: vec![4, 9],
                    delta,
                    track
                }
            ]
        );
        assert_eq!(
            out.stdout,
            "4 track 1 at 1.2 (frame 24000), 96000 frames, take 1/2\n"
        );
        assert_eq!(out.code, 0);
    }
}

#[test]
fn item_trim_and_split_send_times_in_frames() {
    let cases: [(&[&str], Request); 3] = [
        (
            &["item", "trim", "4", "--start", "2.1", "--end", "3s"],
            Request::ItemTrim {
                id: 4,
                start: Some(96_000),
                end: Some(144_000),
            },
        ),
        (
            &["item", "split", "--at", "3.1"],
            Request::ItemSplit {
                ids: None,
                at: 192_000,
            },
        ),
        (
            &["item", "split", "4", "9", "--at", "1s"],
            Request::ItemSplit {
                ids: Some(vec![4, 9]),
                at: 48_000,
            },
        ),
    ];
    for (argv, edit) in cases {
        let mut replies = timing_replies();
        replies.push(reply(MOVED));
        let out = item_cli(argv, replies);
        assert_eq!(out.requests, [Request::Items, Request::Status, edit]);
        assert_eq!(
            out.stdout,
            "4 track 1 at 1.2 (frame 24000), 96000 frames, take 1/2\n"
        );
    }
}

#[test]
fn item_delete_and_take_need_no_timing() {
    let out = cli(
        &["item", "delete", "4", "9"],
        reply(r#"{"ok":true,"deleted":[4,9]}"#),
    );
    assert_eq!(out.requests, [Request::ItemDelete { ids: vec![4, 9] }]);
    assert_eq!(out.stdout, "deleted 4 9\n");
    let out = cli(&["item", "take", "4", "2"], reply(MOVED));
    assert_eq!(out.requests, [Request::ItemTake { id: 4, take: 1 }]);
    assert_eq!(
        out.stdout,
        "4 track 1 at 1.2 (frame 24000), 96000 frames, take 1/2\n"
    );
}

#[test]
fn item_duplicate_and_copy_need_no_timing() {
    let out = cli(&["item", "duplicate", "4", "9"], reply(MOVED));
    assert_eq!(out.requests, [Request::ItemDuplicate { ids: vec![4, 9] }]);
    assert_eq!(
        out.stdout,
        "4 track 1 at 1.2 (frame 24000), 96000 frames, take 1/2\n"
    );
    let out = cli(
        &["item", "copy", "4", "9"],
        reply(r#"{"ok":true,"copied":[4,9]}"#),
    );
    assert_eq!(out.requests, [Request::ItemCopy { ids: vec![4, 9] }]);
    assert_eq!(out.stdout, "copied 4 9\n");
}

#[test]
fn item_paste_sends_the_time_in_frames_and_says_what_it_dropped() {
    let pasted = MOVED.replace(r#""ok":true,"#, r#""ok":true,"dropped":0,"#);
    let cases: [(&[&str], &str, &str); 2] = [
        (
            &["item", "paste", "--track", "Bass", "--at", "2.1"],
            &pasted,
            "4 track 1 at 1.2 (frame 24000), 96000 frames, take 1/2\n",
        ),
        (
            &["item", "paste", "--track", "Bass", "--at", "2.1"],
            r#"{"ok":true,"items":[],"dropped":2}"#,
            "dropped 2 Items: no Track to paste them on\n",
        ),
    ];
    for (argv, answer, stdout) in cases {
        let mut replies = timing_replies();
        replies.push(reply(answer));
        let out = item_cli(argv, replies);
        assert_eq!(
            out.requests,
            [
                Request::Items,
                Request::Status,
                Request::ItemPaste {
                    track: TrackRef::Name("Bass".to_string()),
                    at: 96_000
                }
            ]
        );
        assert_eq!(out.stdout, stdout);
    }
    let mut replies = timing_replies();
    replies.push(reply(r#"{"ok":false,"error":"clipboard is empty"}"#));
    let out = item_cli(&["item", "paste", "--track", "0", "--at", "1.1"], replies);
    assert_eq!(out.stderr, "error: clipboard is empty\n");
    assert_eq!(out.code, 1);
}

#[test]
fn item_command_errors_are_one_line() {
    type Case<'a> = (&'a [&'a str], Vec<Result<Response, ClientError>>, &'a str);
    let cases: [Case; 7] = [
        (
            &["item", "move", "4", "-1s", "--track", "1"],
            Vec::new(),
            "error: give --track before the item ids\n",
        ),
        (
            &["item", "move", "x", "-1s"],
            Vec::new(),
            "error: item id 'x' is not a number\n",
        ),
        (
            &["item", "take", "4", "0"],
            Vec::new(),
            "error: takes are numbered from 1\n",
        ),
        (
            &["item", "move", "4", "1.5x"],
            timing_replies(),
            "error: move needs <±seconds>s or <±bars>[.<beats>]\n",
        ),
        (
            &["item", "split", "--at", "0.1"],
            timing_replies(),
            "error: position must be <bar>.<beat> or <seconds>s\n",
        ),
        (
            &["item", "trim", "4"],
            Vec::new(),
            "error: item trim needs --start, --end or both\n",
        ),
        (
            &["item", "move", "4", "+1s"],
            vec![reply(r#"{"ok":false,"error":"no project open"}"#)],
            "error: no project open\n",
        ),
    ];
    for (argv, replies, stderr) in cases {
        let out = item_cli(argv, replies);
        assert_eq!(out.stderr, stderr, "{argv:?}");
        assert_eq!(out.code, 1);
    }
}

const UNUSED: &str = r#"{"ok":true,"files":[{"file":"takes/guitar-2.wav","bytes":1234567},{"file":"takes/guitar-3.wav","bytes":800}],"bytes":1235367}"#;

#[test]
fn takes_unused_prints_one_line_per_file_and_the_total() {
    let out = cli(&["takes", "unused"], reply(UNUSED));
    assert_eq!(out.requests, [Request::TakesUnused]);
    assert_eq!(
        out.stdout,
        "takes/guitar-2.wav 1.2 MB\ntakes/guitar-3.wav 800 B\n2 unused takes, 1.2 MB\n"
    );
}

#[test]
fn takes_clean_without_yes_lists_and_deletes_nothing() {
    let out = item_cli(&["takes", "clean"], vec![reply(UNUSED)]);
    assert_eq!(out.requests, [Request::TakesUnused]);
    assert_eq!(
        out.stdout,
        "takes/guitar-2.wav 1.2 MB\ntakes/guitar-3.wav 800 B\n2 unused takes, 1.2 MB\nnothing deleted; pass --yes to delete\n"
    );
}

#[test]
fn takes_clean_yes_deletes_every_unused_take() {
    let out = item_cli(
        &["takes", "clean", "--yes"],
        vec![
            reply(UNUSED),
            reply(r#"{"ok":true,"deleted":2,"bytes":1235367,"failed":[]}"#),
        ],
    );
    assert_eq!(
        out.requests,
        [
            Request::TakesUnused,
            Request::TakesClean {
                files: vec![
                    "takes/guitar-2.wav".to_string(),
                    "takes/guitar-3.wav".to_string()
                ],
            },
        ]
    );
    assert_eq!(out.stdout, "deleted 2 takes, 1.2 MB freed\n");
}

#[test]
fn takes_clean_yes_with_nothing_unused_sends_no_clean() {
    let out = item_cli(
        &["takes", "clean", "--yes"],
        vec![reply(r#"{"ok":true,"files":[],"bytes":0}"#)],
    );
    assert_eq!(out.requests, [Request::TakesUnused]);
    assert_eq!(out.stdout, "0 unused takes, 0 B\n");
}

#[test]
fn takes_lines_count_one_take_in_the_singular() {
    let one = r#"{"ok":true,"files":[{"file":"takes/guitar-3.wav","bytes":800}],"bytes":800}"#;
    let out = cli(&["takes", "unused"], reply(one));
    assert_eq!(
        out.stdout,
        "takes/guitar-3.wav 800 B\n1 unused take, 800 B\n"
    );
    let out = item_cli(
        &["takes", "clean", "--yes"],
        vec![
            reply(one),
            reply(r#"{"ok":true,"deleted":1,"bytes":800,"failed":[]}"#),
        ],
    );
    assert_eq!(out.stdout, "deleted 1 take, 800 B freed\n");
    assert_eq!(out.code, 0);
}

#[test]
fn takes_clean_reports_each_file_it_could_not_delete_and_fails() {
    let out = item_cli(
        &["takes", "clean", "--yes"],
        vec![
            reply(UNUSED),
            reply(
                r#"{"ok":true,"deleted":1,"bytes":800,"failed":[{"file":"takes/guitar-2.wav","error":"Permission denied (os error 13)"}]}"#,
            ),
        ],
    );
    assert_eq!(out.stdout, "deleted 1 take, 800 B freed\n");
    assert_eq!(
        out.stderr,
        "error: could not delete takes/guitar-2.wav: Permission denied (os error 13)\n"
    );
    assert_eq!(out.code, 1);
}

#[test]
fn takes_unused_escapes_hidden_characters_in_file_names() {
    let out = cli(
        &["takes", "unused"],
        reply(
            r#"{"ok":true,"files":[{"file":"takes/x\u202evaw.wav","bytes":8},{"file":"takes/y\u001b.wav","bytes":8}],"bytes":16}"#,
        ),
    );
    assert_eq!(
        out.stdout,
        "takes/x\\u{202e}vaw.wav 8 B\ntakes/y\\u{1b}.wav 8 B\n2 unused takes, 16 B\n"
    );
}

/// The Daemon's request line limit, `MAX_LINE` in `socket_server.rs`.
const MAX_LINE: usize = 64 * 1024;

/// Lists `unused` Take files of 1000 bytes each and deletes them as the Daemon does, refusing a request line over [`MAX_LINE`]; in the `takes_clean` batches numbered in `failing` (from 1) it reports the first file in `failed`, and the batch numbered `refuse_at` is refused.
struct CleaningDaemon {
    unused: Vec<String>,
    failing: Vec<usize>,
    refuse_at: Option<usize>,
    cleans: Vec<Vec<String>>,
}

impl Transport for CleaningDaemon {
    fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        if req.to_line().len() > MAX_LINE {
            return reply(r#"{"ok":false,"error":"request too long"}"#);
        }
        match req {
            Request::TakesUnused => {
                let files: Vec<String> = self
                    .unused
                    .iter()
                    .map(|file| format!(r#"{{"file":"{file}","bytes":1000}}"#))
                    .collect();
                reply(&format!(
                    r#"{{"ok":true,"files":[{}],"bytes":{}}}"#,
                    files.join(","),
                    self.unused.len() * 1000
                ))
            }
            Request::TakesClean { files } => {
                self.cleans.push(files.clone());
                if self.refuse_at == Some(self.cleans.len()) {
                    return reply(r#"{"ok":false,"error":"cannot clean takes while recording"}"#);
                }
                if !self.failing.contains(&self.cleans.len()) {
                    return reply(&format!(
                        r#"{{"ok":true,"deleted":{},"bytes":{}}}"#,
                        files.len(),
                        files.len() * 1000
                    ));
                }
                reply(&format!(
                    r#"{{"ok":true,"deleted":{},"bytes":{},"failed":[{{"file":"{}","error":"Permission denied (os error 13)"}}]}}"#,
                    files.len() - 1,
                    (files.len() - 1) * 1000,
                    files[0]
                ))
            }
            req => panic!("unexpected request {req:?}"),
        }
    }
}

fn clean_unused(
    count: usize,
    failing: &[usize],
    refuse_at: Option<usize>,
) -> (Outcome, Vec<Vec<String>>) {
    let mut daemon = CleaningDaemon {
        unused: (0..count)
            .map(|n| format!("takes/guitar-{n}.wav"))
            .collect(),
        failing: failing.to_vec(),
        refuse_at,
        cleans: Vec::new(),
    };
    let argv = ["xguitare", "takes", "clean", "--yes"].map(String::from);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run_cli(&argv, &mut daemon, &mut stdout, &mut stderr, &quiet_env());
    let outcome = Outcome {
        code,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
        requests: Vec::new(),
    };
    (outcome, daemon.cleans)
}

#[test]
fn takes_clean_yes_deletes_thousands_of_takes_in_batches_and_sums_them() {
    let (out, cleans) = clean_unused(5000, &[], None);
    assert_eq!(cleans.concat().len(), 5000);
    assert!(cleans.len() > 1, "one batch only");
    assert_eq!(
        (out.stdout, out.stderr, out.code),
        (
            "deleted 5000 takes, 5.0 MB freed\n".to_string(),
            String::new(),
            0
        )
    );
}

#[test]
fn takes_clean_yes_lists_the_files_of_every_batch_it_could_not_delete_and_exits_1() {
    let (out, cleans) = clean_unused(5000, &[1, 3], None);
    assert!(cleans.len() > 3, "too few batches");
    assert_eq!(
        (out.stdout, out.stderr, out.code),
        (
            "deleted 4998 takes, 5.0 MB freed\n".to_string(),
            format!(
                "error: could not delete {}: Permission denied (os error 13)\n\
                 error: could not delete {}: Permission denied (os error 13)\n",
                cleans[0][0], cleans[2][0]
            ),
            1
        )
    );
}

#[test]
fn takes_clean_yes_stopped_by_an_error_reports_what_went_before() {
    let (out, cleans) = clean_unused(5000, &[1], Some(2));
    assert_eq!(cleans.len(), 2, "the run stops at the refused batch");
    let deleted = format!("deleted {} takes, ", cleans[0].len() - 1);
    assert!(out.stdout.starts_with(&deleted), "{}", out.stdout);
    assert_eq!(
        (out.stderr, out.code),
        (
            format!(
                "error: could not delete {}: Permission denied (os error 13)\n\
                 error: cannot clean takes while recording\n",
                cleans[0][0]
            ),
            1
        )
    );
}

macro_rules! reply_fixture {
    ($name:literal) => {
        include_str!(concat!(
            "../../xguitare-protocol/tests/fixtures/replies/",
            $name,
            ".json"
        ))
    };
}

const NO_PROJECT_STATUS: &str = "preset: -\nprogram: -\nmodified: no\nsample_rate: 48000\nbuffer_size: 128\nprepared_frames: 128\nrevision: 1\nmuted: false\ninput_peak_db: -120.0\noutput_peak_db: -120.0\nxruns: 0\ncallback_max_us: 0\ncallback_last_us: 0\nprotocol_version: 1\nconnections:\ndisk_overflows: 0\nmetronome: off, 120 bpm 4/4, volume -12 dB, beat 0\nproject: -\n";
const PROJECT_STATUS_HEAD: &str = "preset: -\nprogram: -\nmodified: no\nsample_rate: 48000\nbuffer_size: 128\nprepared_frames: 128\nrevision: {revision}\nmuted: false\ninput_peak_db: -120.0\noutput_peak_db: -120.0\nxruns: 0\ncallback_max_us: 0\ncallback_last_us: 0\nprotocol_version: 1\nconnections:\ndisk_overflows: 0\nmetronome: off, 120 bpm 4/4, volume -12 dB, beat 0\nproject: Song\n";
const PROJECT_STATUS_TAIL: &str = "transport: stopped\nposition: 1.1 (frame 0)\ntempo: 120 bpm 4/4\nloop: off 0..0\nclick off, count-in 0 bars\nundo: -, redo: -\ntracks:\n  0* Guitar: -; input -; 0 dB, pan 0; peak -120.0 dB; out -120.0/-120.0 dB\n      send Verb: -6 dB, pre\n  1  Bass: -; input -; 0 dB, pan 0; peak -120.0 dB; out -120.0/-120.0 dB\nbuses:\n  0 Verb: -; 0 dB, pan 0; out -120.0/-120.0 dB\nmaster: {master} dB; out -120.0/-120.0 dB\n";

fn project_status(revision: &str, render: &str, master: &str) -> String {
    PROJECT_STATUS_HEAD.replace("{revision}", revision)
        + render
        + &PROJECT_STATUS_TAIL.replace("{master}", master)
}

#[test]
fn real_daemon_replies_print_their_pinned_text() {
    let cases: [(&[&str], &str, String); 8] = [
        (
            &["status"],
            reply_fixture!("status-no-project"),
            NO_PROJECT_STATUS.to_string(),
        ),
        (
            &["status"],
            reply_fixture!("status-project-render-running"),
            project_status(
                "10",
                "render: song (project) running 0% <temp>/projects/song/renders/song.wav\n",
                "-3",
            ),
        ),
        (
            &["status"],
            reply_fixture!("status-project-render-failed"),
            project_status(
                "11",
                "render: song (project) failed 0% <temp>/projects/song/renders/song.wav: disk full\n",
                "-",
            ),
        ),
        (
            &["item", "list"],
            reply_fixture!("items"),
            "1 Guitar 2.1 0.04s take 1/1\n".to_string(),
        ),
        (
            &["get"],
            reply_fixture!("get-plain"),
            "gain.level = -3 dB\ngain.bypass = no\n".to_string(),
        ),
        (
            &["get"],
            reply_fixture!("get-cabinet-ir"),
            "cabinet.level = 0 dB\ncabinet.ir = ir.wav\ncabinet.bypass = no\n".to_string(),
        ),
        (
            &["get"],
            reply_fixture!("get-choice"),
            "autowah.mode = 0\nautowah.bypass = no\n".to_string(),
        ),
        (
            &["get"],
            reply_fixture!("get-bypassed"),
            "gate.threshold = -60 dB\ngate.attack = 1 ms\ngate.release = 100 ms\ngate.hysteresis = 6 dB\ngate.bypass = yes\n".to_string(),
        ),
    ];
    for (argv, fixture, expected) in cases {
        let out = cli(argv, reply(fixture.trim_end()));
        assert_eq!(
            (out.code, out.stderr.as_str(), out.stdout.as_str()),
            (0, "", expected.as_str()),
            "{argv:?}"
        );
    }
}

#[test]
fn status_prints_volumes_pans_and_progress_as_the_wire_wrote_them() {
    let fixture = reply_fixture!("status-project-render-running")
        .replacen(r#""volume_db":0.0"#, r#""volume_db":-3.3"#, 1)
        .replacen(
            r#""pan":0.0,"peak_db":[-120.0,-120.0],"sends""#,
            r#""pan":0.1,"peak_db":[-120.0,-120.0],"sends""#,
            1,
        )
        .replace(r#""level_db":-6.0"#, r#""level_db":-6.05"#)
        .replace(r#""progress":0.0"#, r#""progress":0.7"#);
    let expected = project_status(
        "10",
        "render: song (project) running 70% <temp>/projects/song/renders/song.wav\n",
        "-3",
    )
    .replace(
        "0* Guitar: -; input -; 0 dB, pan 0;",
        "0* Guitar: -; input -; 0 dB, pan 0.1;",
    )
    .replace("send Verb: -6 dB", "send Verb: -6.05 dB")
    .replace("0 Verb: -; 0 dB", "0 Verb: -; -3.3 dB");
    let out = cli(&["status"], reply(fixture.trim_end()));
    assert_eq!(
        (out.code, out.stderr.as_str(), out.stdout.as_str()),
        (0, "", expected.as_str())
    );
}

#[test]
fn a_status_reply_with_a_wrong_typed_key_is_an_error() {
    let out = cli(
        &["status"],
        reply(&reply_fixture!("status-no-project").replace(r#""xruns":0"#, r#""xruns":"many""#)),
    );
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    assert_eq!(
        out.stderr,
        "error: the status answer is malformed: invalid type: string \"many\", expected u64\n"
    );
}

#[test]
fn a_newer_protocol_still_warns() {
    let out = cli(
        &["status"],
        reply(&reply_fixture!("status-no-project").replace(
            r#""protocol_version":1"#,
            r#""protocol_version":2,"spectrum":[1,2]"#,
        )),
    );
    assert_eq!(out.code, 0);
    assert_eq!(
        out.stderr,
        "warning: the Daemon speaks protocol 2, this xguitare knows 1; update xguitare\n"
    );
    assert_eq!(
        out.stdout,
        NO_PROJECT_STATUS.replace("protocol_version: 1", "protocol_version: 2")
    );
    let unreadable = cli(
        &["status"],
        reply(
            &reply_fixture!("status-no-project")
                .replace(r#""protocol_version":1"#, r#""protocol_version":2"#)
                .replace(r#""xruns":0"#, r#""xruns":"many""#),
        ),
    );
    assert_eq!(unreadable.code, 1);
    assert_eq!(
        unreadable.stderr,
        "warning: the Daemon speaks protocol 2, this xguitare knows 1; update xguitare\nerror: the status answer is malformed: invalid type: string \"many\", expected u64\n"
    );
}
