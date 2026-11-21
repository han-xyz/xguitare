use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use xguitare_core::control::{EngineState, handle};
use xguitare_core::engine::{EngineShared, engine_queues};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, Config};
use xguitare_gui::metronome_model::{
    BeatDot, BeatLight, MetronomeView, beat_dots, beat_light, bpm_text, metronome_key_to_command,
    metronome_view,
};
use xguitare_gui::model::{Command, KeyEvent, Outgoing, requests_until_quit};
use xguitare_gui::project_model::{KeyFocus, key_active, project_key_to_command};
use xguitare_protocol::{Request, Response};

fn status(on: bool, beat: u32) -> Value {
    json!({
        "project": null,
        "metronome": {"on": on, "bpm": 90.0, "meter": [3, 4], "volume_db": -12.0, "beat": beat, "beats_played": u64::from(beat)},
    })
}

fn project_status(on: bool, beat: u32) -> Value {
    let mut status = status(on, beat);
    status["project"] = json!("Demo Song");
    status["transport"] = json!("stopped");
    status
}

fn toggle(on: bool) -> Command {
    Command::Metronome {
        on: Some(on),
        bpm: None,
        meter: None,
        volume: None,
    }
}

#[test]
fn status_gives_the_metronome_view() {
    assert_eq!(
        metronome_view(&status(true, 2)),
        Some(MetronomeView {
            on: true,
            bpm: 90.0,
            meter: (3, 4),
            volume_db: -12.0,
            beat: 2,
            beats_played: 2,
        })
    );
    assert_eq!(metronome_view(&json!({"preset": "Funk"})), None);
}

#[test]
fn a_new_beat_flashes_and_the_same_beat_does_not() {
    assert_eq!(
        beat_light(Some(&status(true, 1)), Some(&status(true, 2))),
        BeatLight::Beat {
            beat: 2,
            accent: false,
            flash: true,
        }
    );
    assert_eq!(
        beat_light(Some(&status(true, 2)), Some(&status(true, 2))),
        BeatLight::Beat {
            beat: 2,
            accent: false,
            flash: false,
        }
    );
}

#[test]
fn a_one_beat_meter_flashes_on_every_beat() {
    let one_four = |beats_played: u64| {
        let mut status = status(true, 1);
        status["metronome"]["meter"] = json!([1, 4]);
        status["metronome"]["beats_played"] = json!(beats_played);
        status
    };
    let light = |flash| BeatLight::Beat {
        beat: 1,
        accent: true,
        flash,
    };
    assert_eq!(
        beat_light(Some(&one_four(7)), Some(&one_four(8))),
        light(true)
    );
    assert_eq!(
        beat_light(Some(&one_four(8)), Some(&one_four(8))),
        light(false)
    );
}

#[test]
fn beat_one_is_accented() {
    assert_eq!(
        beat_light(Some(&status(true, 3)), Some(&status(true, 1))),
        BeatLight::Beat {
            beat: 1,
            accent: true,
            flash: true,
        }
    );
}

#[test]
fn the_first_beat_after_switching_on_flashes() {
    assert_eq!(
        beat_light(Some(&status(false, 0)), Some(&status(true, 1))),
        BeatLight::Beat {
            beat: 1,
            accent: true,
            flash: true,
        }
    );
    assert_eq!(
        beat_light(None, Some(&status(true, 1))),
        BeatLight::Beat {
            beat: 1,
            accent: true,
            flash: true,
        }
    );
}

#[test]
fn off_or_no_status_is_dark() {
    assert_eq!(
        beat_light(Some(&status(true, 2)), Some(&status(false, 0))),
        BeatLight::Dark
    );
    assert_eq!(
        beat_light(Some(&status(true, 2)), Some(&status(false, 3))),
        BeatLight::Dark
    );
    assert_eq!(beat_light(Some(&status(true, 2)), None), BeatLight::Dark);
}

#[test]
fn m_toggles_and_t_taps_with_no_project() {
    assert_eq!(
        metronome_key_to_command(KeyEvent::M, &status(false, 0)),
        Some(toggle(true))
    );
    assert_eq!(
        metronome_key_to_command(KeyEvent::M, &status(true, 2)),
        Some(toggle(false))
    );
    assert_eq!(
        metronome_key_to_command(KeyEvent::T, &status(false, 0)),
        Some(Command::MetronomeTap)
    );
    assert_eq!(
        metronome_key_to_command(KeyEvent::Space, &status(false, 0)),
        None
    );
}

#[test]
fn m_and_t_do_nothing_with_a_project_open() {
    for key in [KeyEvent::M, KeyEvent::T] {
        assert_eq!(
            metronome_key_to_command(key, &project_status(true, 1)),
            None
        );
        assert_eq!(
            project_key_to_command(key, &project_status(true, 1), &[]),
            None
        );
    }
}

#[test]
fn m_and_t_wait_while_a_text_field_has_focus() {
    for key in [KeyEvent::M, KeyEvent::T] {
        assert!(key_active(key, KeyFocus::Nothing), "{key:?}");
        assert!(key_active(key, KeyFocus::Widget), "{key:?}");
        assert!(!key_active(key, KeyFocus::TextField), "{key:?}");
    }
}

#[test]
fn metronome_commands_send_their_requests() {
    let queued = vec![
        Command::Metronome {
            on: None,
            bpm: Some(96.5),
            meter: Some((7, 8)),
            volume: Some(-20.0),
        },
        Command::MetronomeTap,
    ];
    let (outgoing, quit) = requests_until_quit(queued, None, None);
    assert!(!quit);
    assert_eq!(
        outgoing,
        [
            Outgoing::Request(Request::Metronome {
                on: None,
                bpm: Some(96.5),
                meter: Some((7, 8)),
                volume: Some(-20.0),
            }),
            Outgoing::Request(Request::MetronomeTap),
        ]
    );
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ok(request: Request, state: &mut EngineState) -> Value {
    match handle(state, request).0 {
        Response::Ok(payload) => payload,
        Response::Err { error, .. } => panic!("expected ok, got error {error}"),
    }
}

#[test]
fn the_daemon_status_gives_the_metronome_view() {
    let dir = TempDir(
        std::env::temp_dir().join(format!("xguitare-gui-metronome-{}", std::process::id())),
    );
    let _ = fs::remove_dir_all(&dir.0);
    let bank_dir = dir.0.join("bank");
    fs::create_dir_all(&bank_dir).unwrap();
    let config_path = dir.0.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let mut config = Config::load(Some(&config_path)).unwrap();
    config.bank_dir.clone_from(&bank_dir);
    let (queues, _audio, tap) = engine_queues(16);
    let mut state = EngineState::new(
        Bank::load_dir(&bank_dir).unwrap(),
        Arc::new(EngineShared::default()),
        queues,
        Tuner::new(tap, 48_000.0, config.tuner_reference_hz),
        config,
        48_000.0,
        128,
    );
    ok(
        Request::Metronome {
            on: Some(true),
            bpm: Some(90.0),
            meter: Some((3, 4)),
            volume: None,
        },
        &mut state,
    );
    let status = ok(Request::Status, &mut state);
    assert_eq!(
        metronome_view(&status),
        Some(MetronomeView {
            on: true,
            bpm: 90.0,
            meter: (3, 4),
            volume_db: -12.0,
            beat: 0,
            beats_played: 0,
        })
    );
}

#[test]
fn an_idle_metronome_lights_beat_one_and_rings_the_rest() {
    assert_eq!(
        beat_dots(4, BeatLight::Dark),
        [
            BeatDot::Filled {
                accent: true,
                flash: false
            },
            BeatDot::Ring { accent: false },
            BeatDot::Ring { accent: false },
            BeatDot::Ring { accent: false },
        ]
    );
}

#[test]
fn the_playing_beat_stays_filled_after_its_flash() {
    let playing = |flash| BeatLight::Beat {
        beat: 3,
        accent: false,
        flash,
    };
    for flash in [true, false] {
        assert_eq!(
            beat_dots(3, playing(flash)),
            [
                BeatDot::Ring { accent: true },
                BeatDot::Ring { accent: false },
                BeatDot::Filled {
                    accent: false,
                    flash
                },
            ]
        );
    }
}

#[test]
fn beat_one_playing_is_the_accented_dot() {
    let light = BeatLight::Beat {
        beat: 1,
        accent: true,
        flash: true,
    };
    assert_eq!(
        beat_dots(2, light),
        [
            BeatDot::Filled {
                accent: true,
                flash: true
            },
            BeatDot::Ring { accent: false },
        ]
    );
}

#[test]
fn a_whole_tempo_shows_no_decimal() {
    assert_eq!(bpm_text(120.0), "120");
    assert_eq!(bpm_text(97.5), "97.5");
}
