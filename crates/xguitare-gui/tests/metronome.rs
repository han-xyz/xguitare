use serde_json::{Value, json};
use xguitare_core::control::Request;
use xguitare_gui::metronome_model::{
    BeatLight, MetronomeView, beat_light, metronome_key_to_command, metronome_view,
};
use xguitare_gui::model::{Command, KeyEvent, Outgoing, requests_until_quit};
use xguitare_gui::project_model::{key_active, project_key_to_command};

fn status(on: bool, beat: u32) -> Value {
    json!({
        "project": null,
        "metronome": {"on": on, "bpm": 90.0, "meter": [3, 4], "volume_db": -12.0, "beat": beat},
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
        assert!(key_active(key, false, false), "{key:?}");
        assert!(key_active(key, false, true), "{key:?}");
        assert!(!key_active(key, true, true), "{key:?}");
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
