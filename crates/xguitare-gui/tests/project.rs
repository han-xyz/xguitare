use serde_json::{Value, json};
use xguitare_core::control::{BusRef, Position, Request, Response, TrackRef, TransportAction};
use xguitare_core::project::LoopRegion;
use xguitare_gui::mixer_model::{MixerTarget, ShownRack};
use xguitare_gui::model::{
    Command, KeyEvent, KeyModifiers, Outgoing, RemovedUnit, TrackEdit, requests_until_quit,
    response_text, restore,
};
use xguitare_gui::project_model::{
    AutosaveOffer, GridLine, ItemBlock, LoopEdge, MainView, ProjectRow, Tempo, TimelineView,
    TrackRow, TransportState, TransportView, ViewSwitch, autosave_offer, bars_beats_text,
    dragged_loop, grid_lines, item_blocks, key_active, parse_meter, project_end,
    project_key_to_command, project_name, project_rows, project_tempo, recording_from,
    recording_span, restored_view, snap_to_beat, still_closing, time_text, track_rows,
    transport_view,
};

const RATE: f64 = 48_000.0;
const FOUR_FOUR: Tempo = Tempo {
    bpm: 120.0,
    beats_per_bar: 4,
    note: 4,
};

fn project_status(transport: &str) -> Value {
    json!({
        "sample_rate": 48000, "revision": 3, "project": "Demo Song", "transport": transport,
        "position": {"frames": 36000, "bars_beats": "1.2"},
        "tempo": {"bpm": 120.0, "meter": [4, 4]},
        "loop": {"start": 0, "end": 192_000, "on": true},
        "click": {"on": true, "count_in": 1},
        "count_in_remaining": 0, "disk_overflows": 0, "loaded_audio_bytes": 3_840_000,
        "selected": 1,
        "tracks": [
            {"name": "Guitar", "input": "in A", "volume_db": -3.0, "pan": -0.25, "mute": false,
             "solo": false, "armed": true, "input_peak_db": -20.5},
            {"name": "Bass", "input": null, "volume_db": 0.0, "pan": 0.0, "mute": true,
             "solo": false, "armed": false, "input_peak_db": -120.0},
        ],
    })
}

fn project_items() -> Value {
    json!({
        "sample_rate": 48000,
        "tracks": [
            {"id": 0, "name": "Guitar", "items": [{"position": 48_000, "bars_beats": "1.3",
             "offset": 0, "length": 96_000, "take": 1,
             "takes": ["takes/guitar-1.wav", "takes/guitar-2.wav"]}]},
            {"id": 1, "name": "Bass", "items": [{"position": 0, "bars_beats": "1.1",
             "offset": 0, "length": 240_000, "take": 0, "takes": ["takes/bass-1.wav"]}]},
        ],
    })
}

#[test]
fn project_name_and_tempo_come_from_status() {
    let status = project_status("stopped");
    assert_eq!(project_name(Some(&status)), Some("Demo Song"));
    assert_eq!(project_tempo(&status), Some(FOUR_FOUR));
    let no_project = json!({"project": null, "tempo": null});
    assert_eq!(project_name(Some(&no_project)), None);
    assert_eq!(project_name(None), None);
    assert_eq!(project_tempo(&no_project), None);
}

#[test]
fn bars_beats_counts_from_one_with_hundredths() {
    assert_eq!(bars_beats_text(0, RATE, FOUR_FOUR), "1.1.00");
    assert_eq!(bars_beats_text(36_000, RATE, FOUR_FOUR), "1.2.50");
    assert_eq!(bars_beats_text(96_000, RATE, FOUR_FOUR), "2.1.00");
    let three_four = Tempo {
        bpm: 100.0,
        beats_per_bar: 3,
        note: 4,
    };
    assert_eq!(bars_beats_text(86_400, RATE, three_four), "2.1.00");
}

#[test]
fn time_is_minutes_seconds_and_milliseconds() {
    assert_eq!(time_text(0, RATE), "0:00.000");
    assert_eq!(time_text(2_992_560, RATE), "1:02.345");
}

#[test]
fn timeline_maps_frames_to_x_and_back() {
    let view = TimelineView {
        first_frame: 1000.0,
        frames_per_px: 100.0,
    };
    assert!((view.x_of(5800.0) - 48.0).abs() < 1e-4);
    assert!((view.frame_at(48.0) - 5800.0).abs() < 1e-6);
    assert!(view.frame_at(-50.0).abs() < 1e-9);
}

#[test]
fn zoom_keeps_the_frame_under_the_pointer() {
    let view = TimelineView {
        first_frame: 0.0,
        frames_per_px: 100.0,
    };
    let zoomed = view.zoomed(2.0, 100.0);
    assert!((zoomed.frames_per_px - 50.0).abs() < 1e-9);
    assert!((zoomed.frame_at(100.0) - 10_000.0).abs() < 1e-6);
    let far_out = view.zoomed(1e-9, 0.0);
    assert!(far_out.frames_per_px <= RATE);
    let far_in = view.zoomed(1e9, 0.0);
    assert!(far_in.frames_per_px >= 1.0);
}

#[test]
fn scroll_stops_at_the_project_start() {
    let view = TimelineView {
        first_frame: 5000.0,
        frames_per_px: 100.0,
    };
    assert!((view.scrolled(10.0).first_frame - 6000.0).abs() < 1e-9);
    assert!(view.scrolled(-1000.0).first_frame.abs() < 1e-9);
}

#[test]
fn grid_has_every_beat_when_there_is_room() {
    let view = TimelineView {
        first_frame: 0.0,
        frames_per_px: 100.0,
    };
    let lines = grid_lines(view, 960.0, RATE, FOUR_FOUR);
    let expected: Vec<GridLine> = [(0, 1, 1), (24_000, 1, 2), (48_000, 1, 3), (72_000, 1, 4)]
        .into_iter()
        .chain([(96_000, 2, 1)])
        .map(|(frame, bar, beat)| GridLine {
            frame: f64::from(frame),
            bar,
            beat,
        })
        .collect();
    assert_eq!(lines, expected);
}

#[test]
fn grid_starts_at_the_first_visible_beat() {
    let view = TimelineView {
        first_frame: 30_000.0,
        frames_per_px: 100.0,
    };
    let lines = grid_lines(view, 200.0, RATE, FOUR_FOUR);
    assert_eq!(
        lines.iter().map(|line| line.frame).collect::<Vec<_>>(),
        [48_000.0]
    );
}

#[test]
fn zoomed_out_grid_thins_to_bars() {
    let bars_only = TimelineView {
        first_frame: 0.0,
        frames_per_px: 4000.0,
    };
    let lines = grid_lines(bars_only, 100.0, RATE, FOUR_FOUR);
    assert!(lines.iter().all(|line| line.beat == 1));
    assert_eq!(
        lines.iter().map(|line| line.bar).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    let every_other_bar = TimelineView {
        first_frame: 0.0,
        frames_per_px: 20_000.0,
    };
    let lines = grid_lines(every_other_bar, 100.0, RATE, FOUR_FOUR);
    assert_eq!(
        lines.iter().map(|line| line.bar).collect::<Vec<_>>(),
        [1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21]
    );
}

#[test]
fn transport_view_from_status() {
    let recording = transport_view(&project_status("recording")).unwrap();
    assert_eq!(
        recording,
        TransportView {
            state: TransportState::Recording,
            frames: 36_000,
            loop_region: LoopRegion {
                start: 0,
                end: 192_000,
                on: true
            },
            click: true,
            count_in: 1,
            count_in_remaining: 0,
            disk_overflows: 0,
            loaded_audio_bytes: 3_840_000,
        }
    );
    assert_eq!(recording.count_in_text(), None);
    assert_eq!(recording.loaded_audio_text(), "audio 3.8 MB");
    assert!(recording.play_lit());
    assert!(recording.record_lit());
    let stopped = transport_view(&project_status("stopped")).unwrap();
    assert!(!stopped.play_lit());
    assert!(!stopped.record_lit());
    let playing = transport_view(&project_status("playing")).unwrap();
    assert!(playing.play_lit());
    assert!(!playing.record_lit());
    assert_eq!(transport_view(&json!({"project": null})), None);
}

#[test]
fn a_running_count_in_shows_the_beats_left() {
    let mut status = project_status("recording");
    status["count_in_remaining"] = json!(3);
    assert_eq!(
        transport_view(&status).unwrap().count_in_text().as_deref(),
        Some("count-in: 3")
    );
}

#[test]
fn disk_overflows_show_a_warning() {
    let mut status = project_status("recording");
    assert_eq!(transport_view(&status).unwrap().disk_warning(), None);
    status["disk_overflows"] = json!(3);
    assert_eq!(
        transport_view(&status).unwrap().disk_warning().as_deref(),
        Some("disk too slow: frames dropped 3 times")
    );
}

#[test]
fn track_rows_from_status() {
    let rows = track_rows(&project_status("stopped"));
    assert_eq!(
        rows,
        [
            TrackRow {
                index: 0,
                name: "Guitar".to_string(),
                input: Some("in A".to_string()),
                volume_db: -3.0,
                pan: -0.25,
                mute: false,
                solo: false,
                armed: true,
                input_peak_db: -20.5,
                selected: false,
            },
            TrackRow {
                index: 1,
                name: "Bass".to_string(),
                input: None,
                volume_db: 0.0,
                pan: 0.0,
                mute: true,
                solo: false,
                armed: false,
                input_peak_db: -120.0,
                selected: true,
            },
        ]
    );
}

#[test]
fn items_show_their_active_take() {
    let blocks = item_blocks(&project_items());
    assert_eq!(
        blocks,
        [
            ItemBlock {
                track: "Guitar".to_string(),
                position: 48_000,
                length: 96_000,
                take_name: "guitar-2".to_string(),
                take: 1,
                takes: 2,
            },
            ItemBlock {
                track: "Bass".to_string(),
                position: 0,
                length: 240_000,
                take_name: "bass-1".to_string(),
                take: 0,
                takes: 1,
            },
        ]
    );
    assert_eq!(blocks[0].label(), "guitar-2 (2/2)");
    assert_eq!(blocks[1].label(), "bass-1");
}

#[test]
fn tracks_without_items_have_no_blocks() {
    let items = json!({"tracks": [{"id": 0, "name": "Guitar", "items": []}]});
    assert_eq!(item_blocks(&items), []);
}

#[test]
fn project_end_is_the_last_item_or_loop_end() {
    let items = item_blocks(&project_items());
    assert_eq!(project_end(&project_status("stopped"), &items), 240_000);
    let mut status = project_status("stopped");
    status["loop"]["end"] = json!(480_000);
    assert_eq!(project_end(&status, &items), 480_000);
}

#[test]
fn project_keys_follow_reaper() {
    let items = item_blocks(&project_items());
    let none = KeyModifiers::default();
    let stopped = project_status("stopped");
    let playing = project_status("playing");
    let recording = project_status("recording");
    let transport = |action| Some(Command::Transport(action));
    assert_eq!(
        project_key_to_command(KeyEvent::Space, &stopped, &items),
        transport(TransportAction::Play)
    );
    assert_eq!(
        project_key_to_command(KeyEvent::Space, &playing, &items),
        transport(TransportAction::Stop)
    );
    assert_eq!(
        project_key_to_command(KeyEvent::Space, &recording, &items),
        transport(TransportAction::Stop)
    );
    assert_eq!(
        project_key_to_command(KeyEvent::CtrlR, &stopped, &items),
        transport(TransportAction::Record)
    );
    assert_eq!(
        project_key_to_command(KeyEvent::R, &stopped, &items),
        Some(Command::Loop(LoopRegion {
            start: 0,
            end: 192_000,
            on: false
        }))
    );
    assert_eq!(
        project_key_to_command(KeyEvent::Home, &stopped, &items),
        Some(Command::Locate(0))
    );
    assert_eq!(
        project_key_to_command(KeyEvent::End, &stopped, &items),
        Some(Command::Locate(240_000))
    );
    assert_eq!(
        project_key_to_command(KeyEvent::CtrlS, &stopped, &items),
        Some(Command::ProjectSave)
    );
    assert_eq!(
        project_key_to_command(KeyEvent::Digit(1, none), &stopped, &items),
        None
    );
}

#[test]
fn loop_key_on_an_empty_region_loops_four_bars() {
    let mut status = project_status("stopped");
    status["loop"] = json!({"start": 0, "end": 0, "on": false});
    let items = [];
    assert_eq!(
        project_key_to_command(KeyEvent::R, &status, &items),
        Some(Command::Loop(LoopRegion {
            start: 0,
            end: 384_000,
            on: true
        }))
    );
}

#[test]
fn project_keys_need_an_open_project() {
    let mut closed = project_status("stopped");
    closed["project"] = json!(null);
    for key in [
        KeyEvent::Space,
        KeyEvent::R,
        KeyEvent::CtrlR,
        KeyEvent::Home,
        KeyEvent::End,
        KeyEvent::CtrlS,
    ] {
        assert_eq!(project_key_to_command(key, &closed, &[]), None, "{key:?}");
    }
}

#[test]
fn plain_keys_wait_for_no_focus() {
    let digit = KeyEvent::Digit(1, KeyModifiers::default());
    assert!(key_active(KeyEvent::Space, false, false));
    assert!(!key_active(KeyEvent::Space, false, true));
    assert!(!key_active(KeyEvent::Home, false, true));
    assert!(!key_active(KeyEvent::R, false, true));
    assert!(key_active(KeyEvent::CtrlS, false, true));
    assert!(key_active(KeyEvent::CtrlR, false, true));
    assert!(key_active(digit, false, true));
    assert!(!key_active(KeyEvent::CtrlS, true, true));
    assert!(!key_active(digit, true, true));
}

const CTRL: KeyModifiers = KeyModifiers {
    ctrl: true,
    alt: false,
};

#[test]
fn ctrl_digits_select_the_views_in_a_project() {
    let stopped = project_status("stopped");
    for (digit, view) in [
        (1, MainView::Rack),
        (2, MainView::Arrange),
        (3, MainView::Mixer),
    ] {
        assert_eq!(
            project_key_to_command(KeyEvent::Digit(digit, CTRL), &stopped, &[]),
            Some(Command::ShowView(view))
        );
    }
    assert_eq!(
        project_key_to_command(KeyEvent::Digit(4, CTRL), &stopped, &[]),
        None
    );
    let ctrl_alt = KeyModifiers {
        ctrl: true,
        alt: true,
    };
    assert_eq!(
        project_key_to_command(KeyEvent::Digit(1, ctrl_alt), &stopped, &[]),
        None
    );
    let mut closed = stopped;
    closed["project"] = json!(null);
    assert_eq!(
        project_key_to_command(KeyEvent::Digit(1, CTRL), &closed, &[]),
        None
    );
}

#[test]
fn view_keys_wait_while_a_text_field_has_focus() {
    let ctrl_two = KeyEvent::Digit(2, CTRL);
    assert!(key_active(ctrl_two, false, true));
    assert!(!key_active(ctrl_two, true, true));
    assert!(!key_active(KeyEvent::CtrlM, true, true));
}

#[test]
fn ctrl_m_shows_the_mixer_and_goes_back_to_the_previous_view() {
    for from in [MainView::Rack, MainView::Arrange] {
        let mut views = ViewSwitch::new(from);
        views.follow(&Command::ToggleMixer);
        assert_eq!(views.shown, MainView::Mixer);
        views.follow(&Command::ToggleMixer);
        assert_eq!(views.shown, from);
    }
    let mut views = ViewSwitch::new(MainView::Rack);
    views.follow(&Command::ShowView(MainView::Mixer));
    views.follow(&Command::ShowView(MainView::Mixer));
    assert_eq!(views.shown, MainView::Mixer);
    views.follow(&Command::ToggleMixer);
    assert_eq!(views.shown, MainView::Rack);
}

#[test]
fn a_mixer_restored_from_storage_goes_back_to_arrange() {
    let mut views = ViewSwitch::new(MainView::Mixer);
    views.follow(&Command::ToggleMixer);
    assert_eq!(views.shown, MainView::Arrange);
}

#[test]
fn the_stored_view_round_trips_and_defaults_to_arrange() {
    for view in [MainView::Rack, MainView::Arrange, MainView::Mixer] {
        assert_eq!(restored_view(Some(view.stored())), view);
    }
    assert_eq!(restored_view(None), MainView::Arrange);
    assert_eq!(restored_view(Some("timeline")), MainView::Arrange);
}

#[test]
fn loop_edges_drag_and_swap_when_crossed() {
    let region = LoopRegion {
        start: 0,
        end: 96_000,
        on: true,
    };
    assert_eq!(
        dragged_loop(region, LoopEdge::End, 48_000),
        LoopRegion {
            start: 0,
            end: 48_000,
            on: true
        }
    );
    assert_eq!(
        dragged_loop(region, LoopEdge::Start, 120_000),
        LoopRegion {
            start: 96_000,
            end: 120_000,
            on: true
        }
    );
}

#[test]
fn snap_goes_to_the_nearest_beat() {
    assert_eq!(snap_to_beat(30_000.0, RATE, FOUR_FOUR), 24_000);
    assert_eq!(snap_to_beat(37_000.0, RATE, FOUR_FOUR), 48_000);
    assert_eq!(snap_to_beat(-5.0, RATE, FOUR_FOUR), 0);
}

#[test]
fn meter_is_beats_slash_note() {
    assert_eq!(parse_meter("3/4"), Some((3, 4)));
    assert_eq!(parse_meter(" 7 / 8 "), Some((7, 8)));
    assert_eq!(parse_meter("4"), None);
    assert_eq!(parse_meter("x/4"), None);
}

#[test]
fn rack_requests_carry_the_selected_track() {
    let queued = vec![
        Command::Set {
            param: "preamp.gain".to_string(),
            value: 20.0,
        },
        Command::TrackSelect(2),
        Command::Bypass {
            id: "chorus".to_string(),
            on: true,
        },
    ];
    let (outgoing, quit) = requests_until_quit(queued, Some(MixerTarget::Track(1)), None);
    assert!(!quit);
    assert_eq!(
        outgoing,
        [
            Outgoing::Request(Request::Set {
                param: "preamp.gain".to_string(),
                value: 20.0,
                track: Some(TrackRef::Index(1)),
                bus: None,
                master: false,
            }),
            Outgoing::Request(Request::TrackSelect {
                track: TrackRef::Index(2),
            }),
            Outgoing::Request(Request::Bypass {
                id: "chorus".to_string(),
                on: true,
                track: Some(TrackRef::Index(2)),
                bus: None,
                master: false,
            }),
        ]
    );
}

#[test]
fn rack_requests_find_the_shown_bus_by_id_and_master() {
    let queued = vec![
        Command::ShowRack(ShownRack::Bus(3)),
        Command::RackAdd {
            type_name: "reverb".to_string(),
            after: None,
            at: None,
        },
        Command::Set {
            param: "reverb.mix".to_string(),
            value: 40.0,
        },
        Command::ShowRack(ShownRack::Master),
        Command::RackRemove {
            id: "compressor".to_string(),
        },
    ];
    let status = json!({"buses": [{"id": 0, "name": "Verb"}, {"id": 3, "name": "Delay"}]});
    let (outgoing, quit) = requests_until_quit(queued, Some(MixerTarget::Track(0)), Some(&status));
    assert!(!quit);
    assert_eq!(
        outgoing,
        [
            Outgoing::Request(Request::RackAdd {
                r#type: "reverb".to_string(),
                after: None,
                at: None,
                track: None,
                bus: Some(BusRef::Index(1)),
                master: false,
            }),
            Outgoing::Request(Request::Set {
                param: "reverb.mix".to_string(),
                value: 40.0,
                track: None,
                bus: Some(BusRef::Index(1)),
                master: false,
            }),
            Outgoing::Request(Request::RackRemove {
                id: "compressor".to_string(),
                track: None,
                bus: None,
                master: true,
            }),
        ]
    );
}

#[test]
fn rack_requests_for_a_shown_bus_that_is_gone_are_dropped() {
    let queued = vec![
        Command::ShowRack(ShownRack::Bus(7)),
        Command::Set {
            param: "reverb.mix".to_string(),
            value: 40.0,
        },
        Command::Load("Clean".to_string()),
        Command::Mute(true),
        Command::TrackSelect(1),
        Command::RackRemove {
            id: "compressor".to_string(),
        },
    ];
    let status = json!({"buses": [{"id": 0, "name": "Verb"}]});
    let (outgoing, _) = requests_until_quit(queued, Some(MixerTarget::Track(0)), Some(&status));
    assert_eq!(
        outgoing,
        [
            Outgoing::Request(Request::Mute { on: true }),
            Outgoing::Request(Request::TrackSelect {
                track: TrackRef::Index(1),
            }),
            Outgoing::Request(Request::RackRemove {
                id: "compressor".to_string(),
                track: Some(TrackRef::Index(1)),
                bus: None,
                master: false,
            }),
        ]
    );
}

#[test]
fn project_commands_become_their_requests() {
    let queued = vec![
        Command::ProjectNew("Demo Song".to_string()),
        Command::ProjectOpen("Demo Song".to_string()),
        Command::ProjectSave,
        Command::ProjectClose,
        Command::TrackAdd,
        Command::TrackSet {
            track: 0,
            edit: TrackEdit {
                arm: Some(true),
                ..TrackEdit::default()
            },
        },
        Command::Transport(TransportAction::Record),
        Command::Locate(48_000),
        Command::Tempo {
            bpm: Some(100.0),
            meter: None,
        },
        Command::Click {
            on: None,
            count_in: Some(2),
        },
        Command::RestoreAutosave("Demo Song".to_string()),
        Command::RefreshProjects,
    ];
    let (outgoing, _) = requests_until_quit(queued, None, None);
    let request = |req| Outgoing::Request(req);
    assert_eq!(
        outgoing,
        [
            request(Request::ProjectNew {
                name: "Demo Song".to_string()
            }),
            request(Request::ProjectOpen {
                name: "Demo Song".to_string(),
                restore: false,
            }),
            request(Request::ProjectSave),
            request(Request::ProjectClose),
            request(Request::TrackAdd { name: None }),
            request(Request::TrackSet {
                track: TrackRef::Index(0),
                input: None,
                volume: None,
                pan: None,
                mute: None,
                solo: None,
                arm: Some(true),
                name: None,
                to_master: None,
            }),
            request(Request::Transport {
                action: TransportAction::Record,
                position: None,
            }),
            request(Request::Transport {
                action: TransportAction::Locate,
                position: Some(Position::Frames(48_000)),
            }),
            request(Request::Tempo {
                bpm: Some(100.0),
                meter: None,
            }),
            request(Request::Click {
                on: None,
                count_in: Some(2),
            }),
            Outgoing::RestoreAutosave("Demo Song".to_string()),
        ]
    );
}

#[test]
fn a_newer_autosave_is_offered_once_opened() {
    let open = Request::ProjectOpen {
        name: "demo-song".to_string(),
        restore: false,
    };
    let answer = |newer: bool, restored: bool| {
        Ok(Response::Ok(json!({
            "autosave_newer": newer, "restored": restored, "name": "Demo Song",
            "path": "/x/demo-song",
        })))
    };
    assert_eq!(
        autosave_offer(&open, &answer(true, false)),
        Some(AutosaveOffer {
            folder: "demo-song".to_string(),
            name: "Demo Song".to_string(),
        })
    );
    assert_eq!(autosave_offer(&open, &answer(false, false)), None);
    assert_eq!(autosave_offer(&open, &answer(true, true)), None);
    assert_eq!(
        autosave_offer(&Request::ProjectSave, &answer(true, false)),
        None
    );
}

#[test]
fn only_a_closing_project_is_retried() {
    let error = |text: &str| {
        Ok(Response::Err {
            error: text.to_string(),
        })
    };
    assert!(still_closing(&error(
        "a project is still closing; try again"
    )));
    assert!(!still_closing(&error("no project 'x'")));
    assert!(!still_closing(&Ok(Response::Ok(json!({})))));
}

#[test]
fn project_save_reports_its_path() {
    let saved = Ok(Response::Ok(json!({"path": "/x/demo-song/project.toml"})));
    assert_eq!(
        response_text(&Request::ProjectSave, saved).as_deref(),
        Some("saved /x/demo-song/project.toml")
    );
}

#[test]
fn project_list_rows() {
    let list = json!({"projects": [
        {"folder": "demo-song", "name": "Demo Song", "valid": true},
        {"folder": "broken", "name": "broken", "valid": false, "error": "line 1"},
    ]});
    assert_eq!(
        project_rows(&list),
        [
            ProjectRow {
                folder: "demo-song".to_string(),
                name: "Demo Song".to_string(),
                error: None,
            },
            ProjectRow {
                folder: "broken".to_string(),
                name: "broken".to_string(),
                error: Some("line 1".to_string()),
            },
        ]
    );
}

#[test]
fn undo_goes_back_to_the_track_the_module_left() {
    let rack = json!({"params": [
        {"module": "drive", "type": "drive", "param": "gain", "value": 30.0, "bypass": true},
        {"module": "cabinet", "type": "cabinet", "param": "level", "value": 3.0,
         "ir": "guitar-cab.wav"},
    ]});
    let status = project_status("stopped");
    let removed =
        RemovedUnit::new(&rack, Some(&status), "cabinet", std::time::Instant::now()).unwrap();
    let bass = Some(TrackRef::Name("Bass".to_string()));
    assert_eq!(removed.restore.track, bass);
    assert_eq!(
        removed.restore.add_request(&["drive".to_string()]),
        Request::RackAdd {
            r#type: "cabinet".to_string(),
            after: Some("drive".to_string()),
            at: None,
            track: bass.clone(),
            bus: None,
            master: false,
        }
    );
    let tracks: Vec<Option<TrackRef>> = removed
        .restore
        .follow_up_requests("cabinet")
        .into_iter()
        .map(|req| match req {
            Request::Set { track, .. } | Request::RackIr { track, .. } => track,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(tracks, [bass.clone(), bass]);
    let preset_rack = restore(&rack, "cabinet").unwrap();
    assert_eq!(preset_rack.track, None);
}

#[test]
fn a_recording_take_starts_where_recording_was_first_seen() {
    let recording = transport_view(&project_status("recording")).unwrap();
    assert_eq!(recording_from(None, &recording), Some(36_000));
    assert_eq!(recording_from(Some(12_000), &recording), Some(12_000));
    let stopped = transport_view(&project_status("stopped")).unwrap();
    assert_eq!(recording_from(Some(12_000), &stopped), None);
    let mut counting = project_status("recording");
    counting["count_in_remaining"] = json!(2);
    assert_eq!(
        recording_from(None, &transport_view(&counting).unwrap()),
        None
    );
}

#[test]
fn a_recording_take_reaches_the_playhead_and_after_a_loop_wraps_starts_at_the_loop() {
    let recording = transport_view(&project_status("recording")).unwrap();
    assert_eq!(recording_span(12_000, &recording), (12_000, 36_000));
    assert_eq!(recording_span(100_000, &recording), (0, 36_000));
}
