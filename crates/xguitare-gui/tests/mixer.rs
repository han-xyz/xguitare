use std::time::{Duration, Instant};

use serde_json::{Value, json};
use xguitare_gui::mixer_model::{
    Audibility, MixerColumn, MixerTarget, RackFocus, RenderSent, RenderView, STRIP_MAX_WIDTH,
    STRIP_MIN_WIDTH, SendView, ShownRack, StereoPeakHold, StripFit, StripRow, aim_rack_commands,
    default_render_name, fader_db, fader_position, mixer_columns, rack_selectors, render_view,
    send_fraction, send_level, send_text, shown_rack_column, strip_fit, strip_ladder, strip_row,
};
use xguitare_gui::model::{Band, Command, KeyEvent, key_to_command, progress_text_place};
use xguitare_gui::project_model::{MainView, project_key_to_command};
use xguitare_protocol::{BusRef, SendLevel, StatusReply, TrackRef, from_payload};

fn status_reply(payload: &Value) -> StatusReply {
    from_payload(payload).unwrap()
}

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

/// The Tracks get their index as `id`, which every `status` Track carries.
fn status(tracks: &[Value], buses: &[Value]) -> Value {
    let tracks: Vec<Value> = tracks
        .iter()
        .enumerate()
        .map(|(id, track)| {
            let mut track = track.clone();
            track["id"] = json!(id);
            track
        })
        .collect();
    json!({"project": "Demo Song", "transport": "stopped", "selected": 0,
        "loop": {"start": 0, "end": 0, "on": false},
        "tracks": tracks, "buses": buses,
        "master": {"volume_db": 0.0, "peak_db": [-120.0, -120.0], "clip": false},
        "render": null})
}

fn audibility(status: &Value) -> Vec<(String, Audibility)> {
    mixer_columns(&status_reply(status))
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
    let columns = mixer_columns(&status_reply(&status));
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
    assert_eq!(mixer_columns(&status_reply(&closed)), []);
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
fn strip_ladder_lights_up_to_the_level_on_the_fader_scale() {
    let lit = |db: f64| strip_ladder(db).iter().filter(|led| led.lit).count();
    assert_eq!(lit(-120.0), 0);
    assert_eq!(lit(6.0), strip_ladder(6.0).len());
    let unity = strip_ladder(-0.5);
    let below_unity = unity.len() * 3 / 4;
    assert!(unity[..below_unity].iter().all(|led| led.lit));
    assert!(unity[below_unity..].iter().all(|led| !led.lit));
    assert!(lit(-59.0) >= 1);
    assert!(lit(-12.0) < lit(-6.0));
}

#[test]
fn strip_ladder_bands_follow_the_meter_thresholds() {
    let ladder = strip_ladder(6.0);
    assert_eq!(ladder[0].band, Band::Green);
    assert_eq!(ladder[ladder.len() - 1].band, Band::Red);
    let segment_of = |db: f64| (fader_position(db) * ladder.len() as f32) as usize;
    assert_eq!(ladder[segment_of(-30.0)].band, Band::Green);
    assert_eq!(ladder[segment_of(-6.0)].band, Band::Yellow);
    assert_eq!(ladder[segment_of(3.0)].band, Band::Red);
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
        project_key_to_command(KeyEvent::CtrlM, &status_reply(&open), &[]),
        Some(Command::ToggleMixer)
    );
    assert_eq!(
        project_key_to_command(KeyEvent::CtrlAltR, &status_reply(&open), &[]),
        Some(Command::OpenRenderDialog)
    );
    let mut closed = open.clone();
    closed["project"] = json!(null);
    for key in [KeyEvent::CtrlM, KeyEvent::CtrlAltR] {
        assert_eq!(
            project_key_to_command(key, &status_reply(&closed), &[]),
            None
        );
        assert_eq!(key_to_command(key, &[]), None);
    }
}

#[test]
fn render_dialog_follows_status_render() {
    let mut status = status(&[], &[]);
    assert_eq!(
        render_view(&status_reply(&status), None, None),
        RenderView::Idle
    );
    let file = "/home/you/Music/xguitare/demo-song/renders/demo-song.wav";
    let render = |state: &str| {
        json!({"name": "demo-song", "range": "project", "progress": 0.4, "state": state,
            "file": file})
    };
    status["render"] = render("running");
    assert_eq!(
        render_view(&status_reply(&status), None, None),
        RenderView::Running(0.4)
    );
    status["render"] = render("done");
    assert_eq!(
        render_view(&status_reply(&status), None, None),
        RenderView::Done(file.to_string())
    );
    status["render"] = render("cancelled");
    assert_eq!(
        render_view(&status_reply(&status), None, None),
        RenderView::Cancelled
    );
    status["render"] = render("failed");
    status["render"]["error"] = json!("No space left on device (os error 28)");
    assert_eq!(
        render_view(&status_reply(&status), None, None),
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
        default_render_name(Some(&projects), &status_reply(&status)),
        Some("demo-song".to_string())
    );
    assert_eq!(default_render_name(None, &status_reply(&status)), None);
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
    RackFocus::new(
        Some(shown),
        Some(&status_reply(&three_tracks_and_buses(selected))),
    )
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
fn switching_views_keeps_the_shown_rack() {
    for switch in [
        Command::ToggleMixer,
        Command::ShowView(MainView::Rack),
        Command::ShowView(MainView::Arrange),
    ] {
        let (commands, focus) =
            aim_rack_commands(mixer_showing(ShownRack::Master, 1), vec![switch.clone()]);
        assert_eq!(commands, [switch]);
        assert_eq!(focus.shown, Some(ShownRack::Master));
    }
}

#[test]
fn the_shown_rack_is_the_selected_track_until_a_strip_names_another() {
    let status = status_reply(&three_tracks_and_buses(1));
    assert_eq!(
        shown_rack_column(None, &status),
        Some(MixerTarget::Track(1))
    );
    assert_eq!(
        shown_rack_column(Some(ShownRack::Bus(3)), &status),
        Some(MixerTarget::Bus(1))
    );
    assert_eq!(
        shown_rack_column(Some(ShownRack::Master), &status),
        Some(MixerTarget::Master)
    );
    assert_eq!(
        shown_rack_column(Some(ShownRack::Bus(9)), &status),
        Some(MixerTarget::Track(1))
    );
    let (_, focus) = aim_rack_commands(
        RackFocus::new(None, Some(&status)),
        vec![Command::ShowRack(ShownRack::Bus(5)), rack_add()],
    );
    assert_eq!(focus.target(), Some(ShownRack::Bus(5)));
}

#[test]
fn a_bus_removed_elsewhere_closes_its_rack() {
    let status = status_reply(&three_tracks_and_buses(0));
    let gone = RackFocus::new(Some(ShownRack::Bus(9)), Some(&status));
    assert_eq!(gone.shown, None);
    let kept = RackFocus::new(Some(ShownRack::Bus(3)), Some(&status));
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
        previous: status_reply(&status).render,
    };
    assert_eq!(
        render_view(&status_reply(&status), None, Some(&sent)),
        RenderView::Idle
    );
    let earlier = ("no loop region".to_string(), start);
    assert_eq!(
        render_view(&status_reply(&status), Some(&earlier), Some(&sent)),
        RenderView::Idle
    );
    let refused = (
        "invalid render name '../x'".to_string(),
        clicked + Duration::from_millis(5),
    );
    assert_eq!(
        render_view(&status_reply(&status), Some(&refused), Some(&sent)),
        RenderView::Failed("invalid render name '../x'".to_string())
    );
    status["render"]["state"] = json!("running");
    status["render"]["progress"] = json!(0.1);
    assert_eq!(
        render_view(&status_reply(&status), None, Some(&sent)),
        RenderView::Running(0.1)
    );
}

fn fit(fader_height: f32, send_rows: usize) -> StripFit {
    StripFit {
        fader_height,
        send_rows,
    }
}

#[test]
fn strip_height_goes_to_the_fader_minimum_then_send_rows_then_the_fader() {
    assert_eq!(strip_fit(100.0, 2), fit(134.0, 2));
    assert_eq!(strip_fit(134.0 + 68.0, 8), fit(134.0, 2));
    assert_eq!(strip_fit(134.0 + 68.0 + 33.0, 8), fit(134.0, 2));
    assert_eq!(strip_fit(134.0 + 102.0, 8), fit(134.0, 3));
    assert_eq!(strip_fit(134.0 + 272.0 + 20.0, 8), fit(154.0, 8));
    assert_eq!(strip_fit(900.0, 8), fit(320.0, 8));
    assert_eq!(strip_fit(400.0, 1), fit(320.0, 1));
    assert_eq!(strip_fit(200.0, 0), fit(200.0, 0));
}

#[test]
fn a_growing_mixer_never_shrinks_the_fader_or_drops_a_send_row() {
    for buses in 0..=8 {
        let mut last = strip_fit(0.0, buses);
        for step in 0..=1200 {
            let next = strip_fit(step as f32, buses);
            assert!(
                next.fader_height >= last.fader_height && next.send_rows >= last.send_rows,
                "{buses} Buses at {step} px: {last:?} then {next:?}"
            );
            last = next;
        }
    }
}

#[test]
fn whole_strips_fill_the_width_beside_master() {
    for (width, shown) in [(927.0, 5), (671.0, 4)] {
        let row = strip_row(width, 8.0, 10);
        assert_eq!(row.shown, shown, "at {width} px");
        assert!(row.width >= STRIP_MIN_WIDTH && row.width <= STRIP_MAX_WIDTH);
        assert!((shown + 1) as f32 * (row.width + 8.0) <= width);
    }
    assert_eq!(
        strip_row(390.0, 8.0, 10),
        StripRow {
            width: STRIP_MAX_WIDTH,
            shown: 1
        }
    );
    let wide = strip_row(3000.0, 8.0, 30);
    assert!((wide.shown + 1) as f32 * (wide.width + 8.0) <= 3000.0);
    assert!((wide.shown + 2) as f32 * (wide.width + 8.0) > 3000.0);
    let few = strip_row(1568.0, 8.0, 6);
    assert_eq!(few.width, STRIP_MAX_WIDTH);
    assert_eq!(few.shown, 6);
    let narrow = strip_row(100.0, 8.0, 10);
    assert_eq!((narrow.shown, narrow.width), (1, STRIP_MIN_WIDTH));
}

#[test]
fn the_render_percentage_sits_on_one_side_of_the_lit_edge() {
    let bar = 348.0;
    let text = 30.0;
    for fraction in [0.0, 0.05, 0.09, 0.5, 0.95, 1.0] {
        let lit = bar * fraction;
        let (left, on_lit) = progress_text_place(lit, text);
        if on_lit {
            assert!(
                left + text <= lit,
                "{fraction}: on the lit part past its edge"
            );
        } else {
            assert!(left >= lit, "{fraction}: on the well across the lit part");
        }
    }
    assert!(!progress_text_place(bar * 0.05, text).1);
    assert!(progress_text_place(bar * 0.95, text).1);
}
