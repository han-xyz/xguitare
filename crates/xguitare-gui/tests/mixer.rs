use std::time::{Duration, Instant};

use serde_json::{Value, json};
use xguitare_core::control::{BusRef, SendLevel, TrackRef};
use xguitare_gui::mixer_model::{
    Audibility, MixerColumn, MixerTarget, RackFocus, RenderSent, RenderView, SendView, ShownRack,
    StereoPeakHold, aim_rack_commands, default_render_name, fader_db, fader_position,
    mixer_columns, rack_selectors, render_view, send_fraction, send_level, send_text,
};
use xguitare_gui::model::{Command, KeyEvent, key_to_command};
use xguitare_gui::project_model::project_key_to_command;

fn track(name: &str, mute: bool, solo: bool, sends: &[usize]) -> Value {
    let sends: Vec<Value> = sends
        .iter()
        .map(|bus| json!({"bus": bus, "level_db": -6.0, "pre": false}))
        .collect();
    json!({"name": name, "volume_db": 0.0, "pan": 0.0, "mute": mute, "solo": solo,
        "to_master": true, "sends": sends, "peak_db": [-120.0, -120.0], "clip": false})
}

fn bus(id: u64, name: &str, mute: bool, solo: bool) -> Value {
    json!({"id": id, "name": name, "volume_db": 0.0, "pan": 0.0, "mute": mute, "solo": solo,
        "peak_db": [-120.0, -120.0], "clip": false})
}

fn status(tracks: &[Value], buses: &[Value]) -> Value {
    json!({"project": "Demo Song", "transport": "stopped", "selected": 0,
        "loop": {"start": 0, "end": 0, "on": false},
        "tracks": tracks, "buses": buses,
        "master": {"volume_db": 0.0, "peak_db": [-120.0, -120.0], "clip": false},
        "render": null})
}

fn audibility(status: &Value) -> Vec<(String, Audibility)> {
    mixer_columns(status)
        .into_iter()
        .map(|column| (column.name, column.audibility))
        .collect()
}

fn named(pairs: &[(&str, Audibility)]) -> Vec<(String, Audibility)> {
    pairs
        .iter()
        .map(|(name, audible)| ((*name).to_string(), *audible))
        .collect()
}

#[test]
fn columns_are_tracks_then_buses_then_master() {
    let mut status = status(
        &[
            track("Guitar", false, false, &[1]),
            track("Keys", false, false, &[]),
        ],
        &[bus(0, "Verb", false, false), bus(3, "Delay", true, false)],
    );
    status["tracks"][0]["sends"][0]["pre"] = json!(true);
    status["tracks"][1]["to_master"] = json!(false);
    status["tracks"][0]["peak_db"] = json!([-3.5, 0.2]);
    status["tracks"][0]["clip"] = json!(true);
    status["master"]["volume_db"] = json!(null);
    let columns = mixer_columns(&status);
    let targets: Vec<MixerTarget> = columns.iter().map(|column| column.target).collect();
    assert_eq!(
        targets,
        [
            MixerTarget::Track(0),
            MixerTarget::Track(1),
            MixerTarget::Bus(0),
            MixerTarget::Bus(1),
            MixerTarget::Master
        ]
    );
    assert_eq!(
        columns[0],
        MixerColumn {
            target: MixerTarget::Track(0),
            name: "Guitar".to_string(),
            volume_db: 0.0,
            pan: Some(0.0),
            mute: false,
            solo: false,
            peak_db: [-3.5, 0.2],
            clip: true,
            to_master: Some(true),
            sends: vec![
                SendView {
                    bus: 0,
                    level_db: None,
                    pre: false,
                },
                SendView {
                    bus: 1,
                    level_db: Some(-6.0),
                    pre: true,
                },
            ],
            audibility: Audibility::Audible,
        }
    );
    assert_eq!(columns[1].to_master, Some(false));
    assert!(columns[3].mute);
    assert_eq!(columns[3].to_master, None);
    assert_eq!(columns[3].sends, []);
    let master = &columns[4];
    assert_eq!(master.name, "Master");
    assert_eq!(master.pan, None);
    assert_eq!(master.volume_db, f64::NEG_INFINITY);
}

#[test]
fn no_project_has_no_columns() {
    let mut closed = status(&[], &[]);
    closed["project"] = json!(null);
    assert_eq!(mixer_columns(&closed), []);
}

#[test]
fn soloed_track_brings_in_the_bus_it_sends_to() {
    let status = status(
        &[
            track("soloed", false, true, &[0]),
            track("direct", false, false, &[]),
            track("other", false, false, &[0]),
        ],
        &[bus(0, "Verb", false, false)],
    );
    assert_eq!(
        audibility(&status),
        named(&[
            ("soloed", Audibility::Audible),
            ("direct", Audibility::Silent),
            ("other", Audibility::Silent),
            ("Verb", Audibility::SoloInPlace),
            ("Master", Audibility::Audible),
        ])
    );
}

#[test]
fn soloed_bus_brings_in_the_tracks_that_send_to_it() {
    let status = status(
        &[
            track("sender", false, false, &[0]),
            track("other", false, false, &[1]),
        ],
        &[bus(0, "Verb", false, true), bus(1, "Delay", false, false)],
    );
    assert_eq!(
        audibility(&status),
        named(&[
            ("sender", Audibility::SoloInPlace),
            ("other", Audibility::Silent),
            ("Verb", Audibility::Audible),
            ("Delay", Audibility::Silent),
            ("Master", Audibility::Audible),
        ])
    );
}

#[test]
fn nothing_soloed_lets_every_column_sound() {
    let status = status(
        &[
            track("a", false, false, &[0]),
            track("b", false, false, &[]),
        ],
        &[bus(0, "Verb", false, false)],
    );
    assert!(
        audibility(&status)
            .iter()
            .all(|(_, audible)| *audible == Audibility::Audible)
    );
}

#[test]
fn mute_wins_over_solo() {
    let status = status(
        &[
            track("muted", true, true, &[]),
            track("sender", false, false, &[0]),
        ],
        &[bus(0, "Verb", true, true)],
    );
    assert_eq!(
        audibility(&status),
        named(&[
            ("muted", Audibility::Silent),
            ("sender", Audibility::SoloInPlace),
            ("Verb", Audibility::Silent),
            ("Master", Audibility::Audible),
        ])
    );
}

#[test]
fn fader_puts_minus_inf_at_the_bottom_0_db_at_three_quarters_and_6_db_at_the_top() {
    assert_eq!(fader_position(f64::NEG_INFINITY), 0.0);
    assert!((fader_position(0.0) - 0.75).abs() < 1e-6);
    assert!((fader_position(6.0) - 1.0).abs() < 1e-6);
    assert_eq!(fader_position(12.0), 1.0);
    assert!(fader_position(-60.0) > 0.0 && fader_position(-60.0) < 0.1);
    assert_eq!(fader_db(0.0), f64::NEG_INFINITY);
    assert!(fader_db(0.75).abs() < 1e-4);
    assert!((fader_db(1.0) - 6.0).abs() < 1e-4);
    for db in [-60.0, -24.0, -6.0, -0.5, 3.0] {
        assert!((fader_db(fader_position(db)) - db).abs() < 1e-3, "{db}");
    }
    assert!(fader_position(-12.0) < fader_position(-6.0));
}

#[test]
fn peak_hold_keeps_each_side_for_one_second() {
    let start = Instant::now();
    let mut hold = StereoPeakHold::default();
    assert_eq!(hold.held_db(), [-120.0, -120.0]);
    hold.observe([-6.0, -20.0], start);
    hold.observe([-30.0, -10.0], start + Duration::from_millis(500));
    assert_eq!(hold.held_db(), [-6.0, -10.0]);
    hold.observe([-30.0, -30.0], start + Duration::from_millis(1100));
    assert_eq!(hold.held_db(), [-30.0, -10.0]);
    hold.observe([-40.0, -40.0], start + Duration::from_millis(1600));
    assert_eq!(hold.held_db(), [-30.0, -40.0]);
}

#[test]
fn send_knob_maps_off_and_db_to_the_wire() {
    assert_eq!(send_fraction(None), 0.0);
    assert!((send_fraction(Some(0.0)) - 0.75).abs() < 1e-6);
    assert_eq!(send_level(0.0), SendLevel::Off);
    assert_eq!(send_level(fader_position(-70.0)), SendLevel::Off);
    assert_eq!(send_level(1.0), SendLevel::Db(6.0));
    let SendLevel::Db(db) = send_level(send_fraction(Some(-6.0))) else {
        panic!("-6 dB turned off");
    };
    assert!((f64::from(db) + 6.0).abs() < 1e-3);
    assert_eq!(send_text(None), "off");
    assert_eq!(send_text(Some(-6.0)), "-6.0 dB");
}

#[test]
fn rack_selectors_name_the_track_bus_or_master() {
    assert_eq!(rack_selectors(None), (None, None, false));
    assert_eq!(
        rack_selectors(Some(MixerTarget::Track(2))),
        (Some(TrackRef::Index(2)), None, false)
    );
    assert_eq!(
        rack_selectors(Some(MixerTarget::Bus(1))),
        (None, Some(BusRef::Index(1)), false)
    );
    assert_eq!(
        rack_selectors(Some(MixerTarget::Master)),
        (None, None, true)
    );
}

#[test]
fn ctrl_m_and_ctrl_alt_r_work_only_in_a_project() {
    let open = status(&[track("Guitar", false, false, &[])], &[]);
    assert_eq!(
        project_key_to_command(KeyEvent::CtrlM, &open, &[]),
        Some(Command::ToggleMixer)
    );
    assert_eq!(
        project_key_to_command(KeyEvent::CtrlAltR, &open, &[]),
        Some(Command::OpenRenderDialog)
    );
    let mut closed = open.clone();
    closed["project"] = json!(null);
    for key in [KeyEvent::CtrlM, KeyEvent::CtrlAltR] {
        assert_eq!(project_key_to_command(key, &closed, &[]), None);
        assert_eq!(key_to_command(key, &[]), None);
    }
}

#[test]
fn render_dialog_follows_status_render() {
    let mut status = status(&[], &[]);
    assert_eq!(render_view(&status, None, None), RenderView::Idle);
    let file = "/home/you/Music/xguitare/demo-song/renders/demo-song.wav";
    let render = |state: &str| {
        json!({"name": "demo-song", "range": "project", "progress": 0.4, "state": state,
            "file": file})
    };
    status["render"] = render("running");
    assert_eq!(render_view(&status, None, None), RenderView::Running(0.4));
    status["render"] = render("done");
    assert_eq!(
        render_view(&status, None, None),
        RenderView::Done(file.to_string())
    );
    status["render"] = render("cancelled");
    assert_eq!(render_view(&status, None, None), RenderView::Cancelled);
    status["render"] = render("failed");
    status["render"]["error"] = json!("No space left on device (os error 28)");
    assert_eq!(
        render_view(&status, None, None),
        RenderView::Failed("No space left on device (os error 28)".to_string())
    );
}

#[test]
fn render_name_defaults_to_the_project_folder() {
    let status = status(&[], &[]);
    let projects = json!({"projects": [
        {"folder": "live", "name": "Live", "valid": true},
        {"folder": "demo-song", "name": "Demo Song", "valid": true},
    ]});
    assert_eq!(
        default_render_name(Some(&projects), &status),
        Some("demo-song".to_string())
    );
    assert_eq!(default_render_name(None, &status), None);
}

fn rack_add() -> Command {
    Command::RackAdd {
        type_name: "reverb".to_string(),
        after: None,
        at: None,
    }
}

fn three_tracks_and_buses(selected: usize) -> Value {
    let mut status = status(
        &[
            track("Guitar", false, false, &[]),
            track("Bass", false, false, &[]),
            track("Keys", false, false, &[]),
        ],
        &[
            bus(0, "Verb", false, false),
            bus(3, "Delay", false, false),
            bus(5, "Room", false, false),
        ],
    );
    status["selected"] = json!(selected);
    status
}

fn mixer_showing(shown: ShownRack, selected: usize) -> RackFocus {
    RackFocus::new(true, Some(shown), Some(&three_tracks_and_buses(selected)))
}

#[test]
fn selecting_a_track_closes_the_bus_rack_and_aims_at_the_track() {
    let (commands, focus) = aim_rack_commands(
        mixer_showing(ShownRack::Bus(0), 0),
        vec![Command::TrackSelect(2), rack_add()],
    );
    assert_eq!(
        commands,
        [
            Command::TrackSelect(2),
            Command::ShowRack(ShownRack::Track),
            rack_add(),
        ]
    );
    assert_eq!(focus.shown, Some(ShownRack::Track));
}

#[test]
fn removing_the_shown_bus_closes_its_rack() {
    let (commands, focus) = aim_rack_commands(
        mixer_showing(ShownRack::Bus(3), 0),
        vec![Command::BusRemove(1), rack_add()],
    );
    assert_eq!(
        commands,
        [
            Command::BusRemove(1),
            Command::ShowRack(ShownRack::Track),
            rack_add(),
        ]
    );
    assert_eq!(focus.shown, None);
}

#[test]
fn removing_an_earlier_bus_keeps_the_shown_bus() {
    let set = Command::Set {
        param: "reverb.mix".to_string(),
        value: 40.0,
    };
    let (commands, focus) = aim_rack_commands(
        mixer_showing(ShownRack::Bus(5), 0),
        vec![Command::BusRemove(0), set.clone()],
    );
    assert_eq!(
        commands,
        [
            Command::BusRemove(0),
            Command::ShowRack(ShownRack::Bus(5)),
            set,
        ]
    );
    assert_eq!(focus.shown, Some(ShownRack::Bus(5)));
}

#[test]
fn closing_the_mixer_shows_the_selected_track_again() {
    let (commands, focus) = aim_rack_commands(
        mixer_showing(ShownRack::Master, 1),
        vec![Command::ToggleMixer],
    );
    assert_eq!(
        commands,
        [Command::ToggleMixer, Command::ShowRack(ShownRack::Track),]
    );
    assert!(!focus.mixer_open);
    assert_eq!(focus.shown, None);
}

#[test]
fn opening_the_mixer_shows_the_selected_track_rack() {
    let closed = RackFocus::new(false, None, Some(&three_tracks_and_buses(1)));
    let (_, focus) = aim_rack_commands(closed, vec![Command::ToggleMixer]);
    assert!(focus.mixer_open);
    assert_eq!(focus.shown, Some(ShownRack::Track));
}

#[test]
fn a_bus_removed_elsewhere_closes_its_rack() {
    let status = three_tracks_and_buses(0);
    let gone = RackFocus::new(true, Some(ShownRack::Bus(9)), Some(&status));
    assert_eq!(gone.shown, None);
    let kept = RackFocus::new(true, Some(ShownRack::Bus(3)), Some(&status));
    assert_eq!(kept.shown, Some(ShownRack::Bus(3)));
}

#[test]
fn rack_requests_without_a_project_name_no_rack() {
    let (commands, _) = aim_rack_commands(RackFocus::default(), vec![rack_add()]);
    assert_eq!(commands, [rack_add()]);
}

#[test]
fn a_refused_render_shows_its_error_in_place_of_the_last_render() {
    let mut status = status(&[], &[]);
    status["render"] = json!({"name": "demo-song", "range": "project", "progress": 1.0,
        "state": "done", "file": "/x/renders/demo-song.wav"});
    let start = Instant::now();
    let clicked = start + Duration::from_secs(1);
    let sent = RenderSent {
        at: clicked,
        previous: status["render"].clone(),
    };
    assert_eq!(render_view(&status, None, Some(&sent)), RenderView::Idle);
    let earlier = ("no loop region".to_string(), start);
    assert_eq!(
        render_view(&status, Some(&earlier), Some(&sent)),
        RenderView::Idle
    );
    let refused = (
        "invalid render name '../x'".to_string(),
        clicked + Duration::from_millis(5),
    );
    assert_eq!(
        render_view(&status, Some(&refused), Some(&sent)),
        RenderView::Failed("invalid render name '../x'".to_string())
    );
    status["render"]["state"] = json!("running");
    status["render"]["progress"] = json!(0.1);
    assert_eq!(
        render_view(&status, None, Some(&sent)),
        RenderView::Running(0.1)
    );
}
