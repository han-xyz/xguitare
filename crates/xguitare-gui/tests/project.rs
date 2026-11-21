use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use serde_json::{Value, json};
use xguitare_client::TakesCleanStopped;
use xguitare_gui::mixer_model::{MixerTarget, ShownRack};
use xguitare_gui::model::{
    CleanTakesView, Command, KeyEvent, KeyModifiers, Message, Outgoing, RemovedUnit, TrackEdit,
    requests_until_quit, response_message, restore, takes_cleaned_message, unused_takes,
};
use xguitare_gui::project_model::{
    AutosaveOffer, GridLine, ItemBlock, ItemEdge, ItemHit, ItemKey, ItemKeyPress, ItemNodeAction,
    ItemSelection, KeyFocus, LoopEdge, MainView, MoveDrag, MovePointer, ProjectRow, Tempo,
    TimelineView, TrackRow, TransportState, TransportView, TrimDrag, ViewSwitch, VisibleTake,
    autosave_offer, bars_beats_text, copied_text, covered_stretches, dragged_loop,
    esc_clears_selection, grid_lines, hit_test, item_blocks, item_key_to_command, item_menu_keys,
    item_menu_labels, item_name, item_node_action, items_in_box, key_active, move_preview,
    parse_meter, project_end, project_key_to_command, project_name, project_rows, project_tempo,
    recording_from, recording_span, restored_view, snap_to_beat, still_closing, take_names,
    time_text, track_rows, transport_view, trim_preview, trim_step_command, undo_text,
    visible_stretches, waveform_columns,
};
use xguitare_protocol::{
    BusRef, LoopRegion, Position, Request, Response, TrackRef, TransportAction,
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
            {"id": 4, "name": "Guitar", "input": "in A", "volume_db": -3.0, "pan": -0.25, "mute": false,
             "solo": false, "armed": true, "input_peak_db": -20.5},
            {"id": 7, "name": "Bass", "input": null, "volume_db": 0.0, "pan": 0.0, "mute": true,
             "solo": false, "armed": false, "input_peak_db": -120.0},
        ],
    })
}

fn project_items() -> Value {
    json!({
        "sample_rate": 48000,
        "tracks": [
            {"id": 4, "name": "Guitar", "items": [{"id": 3, "position": 48_000, "bars_beats": "1.3",
             "offset": 512, "length": 96_000, "take": 1,
             "takes": ["takes/guitar-1.wav", "takes/guitar-2.wav"]}]},
            {"id": 7, "name": "Bass", "items": [{"id": 1, "position": 0, "bars_beats": "1.1",
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
                id: 4,
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
                id: 7,
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
                id: 3,
                track_id: 4,
                position: 48_000,
                offset: 512,
                length: 96_000,
                file: "takes/guitar-2.wav".to_string(),
                take_frames: 0,
                take_name: "guitar-2".to_string(),
                take: 1,
                takes: 2,
            },
            ItemBlock {
                id: 1,
                track_id: 7,
                position: 0,
                offset: 0,
                length: 240_000,
                file: "takes/bass-1.wav".to_string(),
                take_frames: 0,
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
    assert!(key_active(KeyEvent::Space, KeyFocus::Nothing));
    assert!(!key_active(KeyEvent::Space, KeyFocus::Widget));
    assert!(!key_active(KeyEvent::Home, KeyFocus::Widget));
    assert!(!key_active(KeyEvent::R, KeyFocus::Widget));
    assert!(key_active(KeyEvent::CtrlS, KeyFocus::Widget));
    assert!(key_active(KeyEvent::CtrlR, KeyFocus::Widget));
    assert!(key_active(digit, KeyFocus::Widget));
    assert!(!key_active(KeyEvent::CtrlS, KeyFocus::TextField));
    assert!(!key_active(digit, KeyFocus::TextField));
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
    assert!(key_active(ctrl_two, KeyFocus::Widget));
    assert!(!key_active(ctrl_two, KeyFocus::TextField));
    assert!(!key_active(KeyEvent::CtrlM, KeyFocus::TextField));
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
    let error = |text: &str| Ok(Response::err(text.to_string()));
    assert!(still_closing(&error(
        "a project is still closing; try again"
    )));
    assert!(still_closing(&Ok(Response::err_code(
        "closing, please wait",
        "still_closing"
    ))));
    assert!(!still_closing(&error("no project 'x'")));
    assert!(!still_closing(&Ok(Response::Ok(json!({})))));
}

#[test]
fn a_retry_needs_the_code_or_exactly_the_old_text_without_one() {
    let parsed = |line: &str| Ok(Response::from_line(line).unwrap());
    assert!(still_closing(&parsed(
        r#"{"ok":false,"error":"a project is still closing; try again","code":"still_closing"}"#
    )));
    assert!(still_closing(&parsed(
        r#"{"ok":false,"error":"a project is still closing; try again","code":null}"#
    )));
    assert!(!still_closing(&parsed(
        r#"{"ok":false,"error":"a project is still closing; try again later"}"#
    )));
    assert!(!still_closing(&parsed(
        r#"{"ok":false,"error":"x: a project is still closing; try again"}"#
    )));
    assert!(!still_closing(&parsed(
        r#"{"ok":false,"error":"closing, please wait","code":"busy"}"#
    )));
    assert!(!still_closing(&parsed(
        r#"{"ok":false,"error":"no project 'x'","code":"still_closing_not"}"#
    )));
}

#[test]
fn project_save_reports_its_path() {
    let saved = Ok(Response::Ok(json!({"path": "/x/demo-song/project.toml"})));
    assert_eq!(
        response_message(&Request::ProjectSave, saved),
        Some(Message::Notice(
            "saved /x/demo-song/project.toml".to_string()
        ))
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

/// `[min, max]` per 256-frame bucket.
fn table(pairs: &[(i8, i8)]) -> Vec<i8> {
    pairs.iter().flat_map(|&(min, max)| [min, max]).collect()
}

#[test]
fn zoomed_out_a_column_holds_the_min_of_mins_and_max_of_maxes() {
    let peaks = table(&[
        (-10, 20),
        (-50, 5),
        (-3, 90),
        (0, 1),
        (-1, 2),
        (-2, 3),
        (-127, 4),
        (-4, 30),
    ]);
    assert_eq!(
        waveform_columns(&peaks, 0..2048, 2),
        [(-50, 90), (-127, 30)]
    );
}

#[test]
fn zoomed_in_a_column_repeats_its_bucket() {
    let peaks = table(&[(-10, 20), (-50, 5)]);
    assert_eq!(
        waveform_columns(&peaks, 0..512, 8),
        [
            (-10, 20),
            (-10, 20),
            (-10, 20),
            (-10, 20),
            (-50, 5),
            (-50, 5),
            (-50, 5),
            (-50, 5)
        ]
    );
    assert_eq!(
        waveform_columns(&peaks, 384..512, 2),
        [(-50, 5), (-50, 5)],
        "frames from the middle of the table"
    );
}

#[test]
fn columns_stop_at_the_end_of_the_table_and_empty_input_draws_none() {
    let peaks = table(&[(-10, 20), (-50, 5)]);
    assert_eq!(waveform_columns(&peaks, 0..1024, 4), [(-10, 20), (-50, 5)]);
    assert_eq!(waveform_columns(&[], 0..1024, 4), []);
    assert_eq!(waveform_columns(&peaks, 0..1024, 0), []);
    assert_eq!(waveform_columns(&peaks, 256..256, 4), []);
}

#[test]
fn the_visible_take_frames_are_the_items_played_range_inside_the_view() {
    let item = ItemBlock {
        id: 1,
        track_id: 0,
        position: 1024,
        offset: 256,
        length: 512,
        file: "takes/a.wav".to_string(),
        take_frames: 0,
        take_name: "a".to_string(),
        take: 0,
        takes: 1,
    };
    let view = |first_frame| TimelineView {
        first_frame,
        frames_per_px: 64.0,
    };
    assert_eq!(
        item.visible_take(view(0.0), 1000.0),
        Some(VisibleTake {
            x: 16.0,
            width: 8,
            frames: 256..768
        }),
        "offset to offset + length"
    );
    assert_eq!(
        item.visible_take(view(1280.0), 1000.0),
        Some(VisibleTake {
            x: 0.0,
            width: 4,
            frames: 512..768
        }),
        "the start scrolled off the left"
    );
    assert_eq!(
        item.visible_take(view(0.0), 20.0),
        Some(VisibleTake {
            x: 16.0,
            width: 4,
            frames: 256..512
        }),
        "the end past the right"
    );
    assert_eq!(item.visible_take(view(2048.0), 1000.0), None);
}

fn block(id: u64, track_id: u64, position: u64, length: u64) -> ItemBlock {
    ItemBlock {
        id,
        track_id,
        position,
        offset: 0,
        length,
        file: format!("takes/t{id}.wav"),
        take_frames: length,
        take_name: format!("t{id}"),
        take: 0,
        takes: 1,
    }
}

/// One beat (24 000 frames at 120 bpm) is 50 px.
const EDIT_VIEW: TimelineView = TimelineView {
    first_frame: 0.0,
    frames_per_px: 480.0,
};
const GUITAR: u64 = 4;
const BASS: u64 = 7;
const TRACK_IDS: [u64; 2] = [GUITAR, BASS];

/// Guitar: Item 3 at beats 2–6 under Item 5 at beats 4–5; Bass: Item 1 from 0 for ten beats.
fn edit_items() -> Vec<ItemBlock> {
    vec![
        block(3, GUITAR, 48_000, 96_000),
        block(1, BASS, 0, 240_000),
        block(5, GUITAR, 96_000, 24_000),
    ]
}

fn selection(ids: &[u64]) -> ItemSelection {
    let mut selection = ItemSelection::default();
    selection.select_box(ids, false);
    selection
}

#[test]
fn hit_test_finds_body_edges_lane_and_the_top_item() {
    let items = edit_items();
    let hit = |track, x| hit_test(&items, track, EDIT_VIEW, x);
    assert_eq!(hit(GUITAR, 150.0), ItemHit::Body(3));
    assert_eq!(hit(GUITAR, 102.0), ItemHit::StartEdge(3));
    assert_eq!(hit(GUITAR, 298.0), ItemHit::EndEdge(3));
    assert_eq!(
        hit(GUITAR, 225.0),
        ItemHit::Body(5),
        "the later Item is on top"
    );
    assert_eq!(hit(GUITAR, 50.0), ItemHit::Lane);
    assert_eq!(hit(GUITAR, 300.0), ItemHit::Lane, "the end is not inside");
    assert_eq!(hit(BASS, 50.0), ItemHit::Body(1));
}

#[test]
fn click_selects_only_this_and_ctrl_click_toggles() {
    let mut picked = ItemSelection::default();
    picked.pick(3, false);
    picked.pick(1, false);
    assert_eq!(picked.ids(), [1]);
    picked.pick(3, true);
    assert_eq!(picked.ids(), [1, 3]);
    picked.pick(1, true);
    assert_eq!(picked.ids(), [3]);
    picked.clear();
    assert!(picked.is_empty());
}

#[test]
fn grabbing_a_selected_item_keeps_the_selection() {
    let mut grabbed = selection(&[1, 3]);
    grabbed.grab(3, false);
    assert_eq!(grabbed.ids(), [1, 3]);
    grabbed.grab(5, false);
    assert_eq!(grabbed.ids(), [5]);
    grabbed.grab(1, true);
    assert_eq!(grabbed.ids(), [1, 5]);
}

#[test]
fn a_box_selects_the_items_it_touches_across_tracks() {
    let items = edit_items();
    assert_eq!(
        items_in_box(&items, &[GUITAR, BASS], 100_000.0..130_000.0),
        [3, 1, 5]
    );
    assert_eq!(
        items_in_box(&items, &[GUITAR], 0.0..40_000.0),
        [] as [u64; 0]
    );
    assert_eq!(items_in_box(&items, &[GUITAR], 130_000.0..150_000.0), [3]);
    let mut boxed = selection(&[5]);
    boxed.select_box(&[3], false);
    assert_eq!(boxed.ids(), [3]);
    boxed.select_box(&[1], true);
    assert_eq!(boxed.ids(), [1, 3]);
}

#[test]
fn selection_follows_ids_and_takes_both_halves_of_a_split() {
    let before: Vec<u64> = vec![1, 3, 5];
    let after = vec![
        block(3, GUITAR, 48_000, 48_000),
        block(1, BASS, 0, 240_000),
        block(8, GUITAR, 96_000, 48_000),
    ];
    let mut kept = selection(&[3, 5]);
    kept.follow(&before, &after, None);
    assert_eq!(kept.ids(), [3], "5 is gone");
    let mut split = selection(&[3]);
    split.follow(&before, &after, Some(96_000));
    assert_eq!(split.ids(), [3, 8]);
    let mut default_split = ItemSelection::default();
    default_split.follow(&before, &after, Some(96_000));
    assert_eq!(default_split.ids(), [3, 8], "the selected Track's halves");
    let mut other = selection(&[1]);
    other.follow(&before, &after, Some(96_000));
    assert_eq!(other.ids(), [1], "a half of an unselected Item stays out");
}

fn pointer(frame: f64, row: usize) -> MovePointer {
    MovePointer {
        frame,
        row,
        snap: true,
        lock_track: false,
    }
}

#[test]
fn a_move_snaps_the_grabbed_start_to_the_visible_grid() {
    let items = edit_items();
    let grid = grid_lines(EDIT_VIEW, 1000.0, RATE, FOUR_FOUR);
    let drag = MoveDrag::new(3, &selection(&[3]), 60_000.0, 0);
    let snapped = move_preview(&drag, &items, &TRACK_IDS, &grid, pointer(90_000.0, 0));
    assert_eq!(
        (snapped.delta, snapped.rows, snapped.snap_line),
        (24_000, 0, Some(72_000))
    );
    let free = MovePointer {
        snap: false,
        ..pointer(90_000.0, 0)
    };
    let free = move_preview(&drag, &items, &TRACK_IDS, &grid, free);
    assert_eq!((free.delta, free.snap_line), (30_000, None));
    assert_eq!(
        snapped.command(&drag),
        Some(Command::ItemMove {
            ids: vec![3],
            delta: 24_000,
            track: None
        })
    );
    assert_eq!(snapped.place(&items[0], 0), (72_000, 0));
}

#[test]
fn a_move_stops_at_frame_zero_keeping_distances_and_sends_nothing_when_still() {
    let items = edit_items();
    let grid = grid_lines(EDIT_VIEW, 1000.0, RATE, FOUR_FOUR);
    let drag = MoveDrag::new(3, &selection(&[1, 3]), 60_000.0, 0);
    let earlier = move_preview(&drag, &items, &TRACK_IDS, &grid, pointer(10_000.0, 0));
    assert_eq!((earlier.delta, earlier.snap_line), (0, None));
    assert_eq!(earlier.command(&drag), None);
    let only = MoveDrag::new(3, &selection(&[3]), 60_000.0, 0);
    let free = MovePointer {
        snap: false,
        ..pointer(0.0, 0)
    };
    let clamped = move_preview(&only, &items, &TRACK_IDS, &grid, free);
    assert_eq!(clamped.delta, -48_000);
}

#[test]
fn a_move_changes_track_by_the_row_under_the_pointer_unless_locked() {
    let items = edit_items();
    let grid = grid_lines(EDIT_VIEW, 1000.0, RATE, FOUR_FOUR);
    let drag = MoveDrag::new(3, &selection(&[1, 3]), 60_000.0, 0);
    let one = MoveDrag::new(3, &selection(&[3]), 60_000.0, 0);
    let down = move_preview(&one, &items, &TRACK_IDS, &grid, pointer(60_000.0, 1));
    assert_eq!((down.delta, down.rows), (0, 1));
    assert_eq!(
        down.command(&one),
        Some(Command::ItemMove {
            ids: vec![3],
            delta: 0,
            track: Some(1)
        })
    );
    assert_eq!(down.place(&items[0], 0), (48_000, 1));
    let locked = MovePointer {
        lock_track: true,
        ..pointer(60_000.0, 1)
    };
    assert_eq!(
        move_preview(&one, &items, &TRACK_IDS, &grid, locked).rows,
        0
    );
    let both = move_preview(&drag, &items, &TRACK_IDS, &grid, pointer(84_000.0, 1));
    assert_eq!(both.rows, 0, "the Bass Item has no Track below");
    assert_eq!(
        both.command(&drag),
        Some(Command::ItemMove {
            ids: vec![3, 1],
            delta: 24_000,
            track: None
        }),
        "the grabbed Item first"
    );
}

#[test]
fn item_keys_split_delete_undo_redo_and_nudge() {
    let items = edit_items();
    let status = project_status("stopped");
    let key = |key, ids: &[u64]| {
        item_key_to_command(key, &status, &items, &selection(ids), Some(EDIT_VIEW))
    };
    assert_eq!(
        key(KeyEvent::S, &[3]),
        Some(Command::ItemSplit {
            ids: Some(vec![3]),
            at: 36_000
        })
    );
    assert_eq!(
        key(KeyEvent::S, &[]),
        Some(Command::ItemSplit {
            ids: None,
            at: 36_000
        })
    );
    assert_eq!(
        key(KeyEvent::Delete, &[1, 3]),
        Some(Command::ItemDelete(vec![1, 3]))
    );
    assert_eq!(key(KeyEvent::Delete, &[]), None);
    assert_eq!(
        key(KeyEvent::AltDown, &[3]),
        Some(Command::ItemMove {
            ids: vec![3],
            delta: 0,
            track: Some(1)
        })
    );
    assert_eq!(key(KeyEvent::AltUp, &[3]), None, "no Track above Guitar");
    assert_eq!(key(KeyEvent::AltDown, &[1, 3]), None, "no Track below Bass");
    assert_eq!(
        key(KeyEvent::AltUp, &[1]),
        Some(Command::ItemMove {
            ids: vec![1],
            delta: 0,
            track: Some(0)
        })
    );
    assert_eq!(key(KeyEvent::AltDown, &[]), None);
    let nudge = |delta| {
        Some(Command::ItemMove {
            ids: vec![3],
            delta,
            track: None,
        })
    };
    assert_eq!(key(KeyEvent::Right, &[3]), nudge(24_000));
    assert_eq!(key(KeyEvent::Left, &[3]), nudge(-24_000));
    assert_eq!(key(KeyEvent::Left, &[1]), None, "already at frame 0");
    assert_eq!(key(KeyEvent::Right, &[]), None);
    let zoomed_out = TimelineView {
        first_frame: 0.0,
        frames_per_px: 4_800.0,
    };
    assert_eq!(
        item_key_to_command(
            KeyEvent::Right,
            &status,
            &items,
            &selection(&[3]),
            Some(zoomed_out)
        ),
        nudge(96_000),
        "one bar when beats are too close to draw"
    );
    let mut closed = project_status("stopped");
    closed["project"] = json!(null);
    assert_eq!(
        item_key_to_command(KeyEvent::S, &closed, &items, &selection(&[]), None),
        None
    );
}

#[test]
fn undo_and_redo_keys_are_project_keys() {
    let stopped = project_status("stopped");
    assert_eq!(
        project_key_to_command(KeyEvent::CtrlZ, &stopped, &[]),
        Some(Command::ProjectUndo)
    );
    assert_eq!(
        project_key_to_command(KeyEvent::CtrlShiftZ, &stopped, &[]),
        Some(Command::ProjectRedo)
    );
    let mut closed = stopped;
    closed["project"] = json!(null);
    assert_eq!(project_key_to_command(KeyEvent::CtrlZ, &closed, &[]), None);
}

#[test]
fn item_keys_wait_only_for_a_text_field_and_arrows_for_no_focus() {
    for key in [
        KeyEvent::S,
        KeyEvent::Delete,
        KeyEvent::CtrlZ,
        KeyEvent::CtrlShiftZ,
    ] {
        assert!(key_active(key, KeyFocus::Widget), "{key:?}");
        assert!(!key_active(key, KeyFocus::TextField), "{key:?}");
    }
    for key in [
        KeyEvent::Left,
        KeyEvent::Right,
        KeyEvent::AltUp,
        KeyEvent::AltDown,
    ] {
        assert!(key_active(key, KeyFocus::Nothing), "{key:?}");
        assert!(!key_active(key, KeyFocus::Widget), "{key:?}");
    }
    assert_eq!(
        project_key_to_command(KeyEvent::S, &project_status("stopped"), &[]),
        None
    );
}

#[test]
fn esc_clears_the_selection_before_the_toast() {
    assert!(esc_clears_selection((false, false), false, true));
    assert!(!esc_clears_selection((false, false), false, false));
    assert!(!esc_clears_selection((true, false), false, true));
    assert!(!esc_clears_selection((false, true), false, true));
    assert!(!esc_clears_selection((false, false), true, true));
}

#[test]
fn the_undo_label_comes_from_status() {
    let mut status = project_status("stopped");
    status["undo"] = json!("move 2 Items");
    assert_eq!(undo_text(&status).as_deref(), Some("Undo: move 2 Items"));
    status["undo"] = json!(null);
    assert_eq!(undo_text(&status), None);
}

#[test]
fn an_item_is_named_by_take_track_and_bar() {
    let item = block(9, BASS, 200_000, 48_000);
    assert_eq!(
        item_name(
            &item,
            std::slice::from_ref(&item),
            "Bass DI",
            RATE,
            FOUR_FOUR
        ),
        "Item t9, Bass DI, bar 3"
    );
    let first = block(2, BASS, 0, 1);
    assert_eq!(
        item_name(
            &first,
            std::slice::from_ref(&first),
            "Bass DI",
            RATE,
            FOUR_FOUR
        ),
        "Item t2, Bass DI, bar 1"
    );
}

#[test]
fn an_item_name_tells_its_take_count_and_what_covers_it() {
    let lower = ItemBlock {
        take: 1,
        takes: 3,
        ..block(1, GUITAR, 0, 400_000)
    };
    let items = vec![
        lower.clone(),
        block(2, GUITAR, 100_000, 50_000),
        block(3, BASS, 0, 400_000),
        block(4, GUITAR, 300_000, 200_000),
    ];
    assert_eq!(
        item_name(&lower, &items, "Guitar", RATE, FOUR_FOUR),
        "Item t1, Take 2 of 3, Guitar, bar 1, partly covered by t2 and t4"
    );
    assert_eq!(
        item_name(&items[3], &items, "Guitar", RATE, FOUR_FOUR),
        "Item t4, Guitar, bar 4"
    );
    let buried = vec![
        block(5, GUITAR, 96_000, 48_000),
        block(6, GUITAR, 0, 400_000),
    ];
    assert_eq!(
        item_name(&buried[0], &buried, "Guitar", RATE, FOUR_FOUR),
        "Item t5, Guitar, bar 2, covered by t6"
    );
}

#[test]
fn visible_stretches_are_what_the_items_above_leave_uncovered() {
    let items = vec![
        block(1, GUITAR, 0, 1000),
        block(2, BASS, 100, 100),
        block(3, GUITAR, 200, 200),
        block(4, GUITAR, 900, 300),
        block(5, GUITAR, 0, 50),
    ];
    assert_eq!(
        visible_stretches(&items, 1),
        [
            Range {
                start: 50,
                end: 200
            },
            Range {
                start: 400,
                end: 900
            }
        ]
    );
    assert_eq!(
        visible_stretches(&items, 4),
        [Range {
            start: 900,
            end: 1200
        }]
    );
    let buried = vec![block(6, GUITAR, 100, 100), block(7, GUITAR, 0, 400)];
    assert_eq!(visible_stretches(&buried, 6), Vec::<Range<u64>>::new());
}

#[test]
fn a_covered_item_answers_only_on_its_visible_stretches() {
    let items = vec![
        block(1, GUITAR, 0, 96_000),
        block(2, GUITAR, 48_000, 96_000),
    ];
    let hit = |x| hit_test(&items, GUITAR, EDIT_VIEW, x);
    assert_eq!(hit(50.0), ItemHit::Body(1));
    assert_eq!(
        hit(150.0),
        ItemHit::Body(2),
        "where the lower Item is covered"
    );
    assert_eq!(
        hit(198.0),
        ItemHit::Body(2),
        "the lower Item's hidden end is not grabbed"
    );
    assert_eq!(hit(102.0), ItemHit::StartEdge(2));
}

#[test]
fn copied_items_are_named_on_the_system_clipboard() {
    let items = edit_items();
    assert_eq!(copied_text(&items, &[3, 5]), "xguitare Items: t3, t5");
    assert_eq!(copied_text(&items, &[1]), "xguitare Items: t1");
}

#[test]
fn item_menu_edits_name_the_count_of_more_than_one_item() {
    assert_eq!(
        item_menu_labels(1),
        ["Split at playhead", "Duplicate", "Copy", "Delete"]
    );
    assert_eq!(
        item_menu_labels(2),
        [
            "Split 2 Items at playhead",
            "Duplicate 2 Items",
            "Copy 2 Items",
            "Delete 2 Items"
        ]
    );
}

#[test]
fn item_menu_keys_show_only_on_a_selected_item() {
    let both = selection(&[1, 2]);
    assert_eq!(
        item_menu_keys(&both, 2),
        Some(["S", "Ctrl+D", "Ctrl+C", "Delete"])
    );
    assert_eq!(item_menu_keys(&both, 3), None);
    assert_eq!(item_menu_keys(&ItemSelection::default(), 1), None);
}

#[test]
fn item_edits_become_their_requests() {
    let queued = vec![
        Command::ItemMove {
            ids: vec![3, 1],
            delta: -24_000,
            track: Some(1),
        },
        Command::ItemMove {
            ids: vec![3],
            delta: 24_000,
            track: None,
        },
        Command::ItemSplit {
            ids: None,
            at: 36_000,
        },
        Command::ItemDelete(vec![3]),
        Command::ProjectUndo,
        Command::ProjectRedo,
    ];
    let (outgoing, _) = requests_until_quit(queued, None, None);
    let request = |req| Outgoing::Request(req);
    assert_eq!(
        outgoing,
        [
            request(Request::ItemMove {
                ids: vec![3, 1],
                delta: -24_000,
                track: Some(TrackRef::Index(1)),
            }),
            request(Request::ItemMove {
                ids: vec![3],
                delta: 24_000,
                track: None,
            }),
            request(Request::ItemSplit {
                ids: None,
                at: 36_000,
            }),
            request(Request::ItemDelete { ids: vec![3] }),
            request(Request::ProjectUndo),
            request(Request::ProjectRedo),
        ]
    );
}

#[test]
fn a_focused_item_leaves_the_transport_and_view_keys_active() {
    for key in [KeyEvent::Space, KeyEvent::R, KeyEvent::Home, KeyEvent::End] {
        assert!(key_active(key, KeyFocus::Item), "{key:?}");
    }
    for key in [
        KeyEvent::Left,
        KeyEvent::Right,
        KeyEvent::AltUp,
        KeyEvent::AltDown,
    ] {
        assert!(
            !key_active(key, KeyFocus::Item),
            "{key:?} is the Item's own"
        );
    }
    for key in [
        KeyEvent::S,
        KeyEvent::Delete,
        KeyEvent::CtrlZ,
        KeyEvent::CtrlS,
    ] {
        assert!(key_active(key, KeyFocus::Item), "{key:?}");
    }
}

fn press(key: ItemKey, (ctrl, shift, alt): (bool, bool, bool)) -> Option<ItemNodeAction> {
    item_node_action(ItemKeyPress {
        key,
        ctrl,
        shift,
        alt,
    })
}

#[test]
fn a_focused_item_takes_its_keys_only_with_their_own_modifiers() {
    const NONE: (bool, bool, bool) = (false, false, false);
    const CTRL: (bool, bool, bool) = (true, false, false);
    const SHIFT: (bool, bool, bool) = (false, true, false);
    const ALT: (bool, bool, bool) = (false, false, true);
    const CTRL_ALT: (bool, bool, bool) = (true, false, true);
    assert_eq!(press(ItemKey::Enter, NONE), Some(ItemNodeAction::Select));
    assert_eq!(press(ItemKey::Enter, CTRL), Some(ItemNodeAction::Toggle));
    assert_eq!(press(ItemKey::Left, NONE), Some(ItemNodeAction::Nudge(-1)));
    assert_eq!(press(ItemKey::Right, NONE), Some(ItemNodeAction::Nudge(1)));
    assert_eq!(press(ItemKey::Up, ALT), Some(ItemNodeAction::TrackStep(-1)));
    assert_eq!(
        press(ItemKey::Down, ALT),
        Some(ItemNodeAction::TrackStep(1))
    );
    for (key, modifiers) in [
        (ItemKey::Enter, SHIFT),
        (ItemKey::Enter, ALT),
        (ItemKey::Enter, CTRL_ALT),
        (ItemKey::Left, CTRL),
        (ItemKey::Left, ALT),
        (ItemKey::Right, CTRL_ALT),
        (ItemKey::Up, NONE),
        (ItemKey::Down, CTRL),
        (ItemKey::Up, CTRL_ALT),
    ] {
        assert_eq!(press(key, modifiers), None, "{key:?} {modifiers:?}");
    }
}

/// Plays frames 12 000 to 60 000 of a 120 000-frame Take from 48 000: the Take reaches from 36 000 to 156 000 on the timeline.
fn trimmed_item() -> ItemBlock {
    ItemBlock {
        offset: 12_000,
        take_frames: 120_000,
        ..block(5, GUITAR, 48_000, 48_000)
    }
}

fn trim(edge: ItemEdge, to: f64, snap: bool) -> (u64, u64, Option<u64>) {
    let item = trimmed_item();
    let grid = grid_lines(EDIT_VIEW, 1000.0, RATE, FOUR_FOUR);
    let grabbed = match edge {
        ItemEdge::Start => 48_000.0,
        ItemEdge::End => 96_000.0,
    };
    let drag = TrimDrag::new(5, edge, grabbed);
    let preview = trim_preview(&drag, &[item], &grid, to, snap).unwrap();
    (preview.start, preview.end, preview.snap_line)
}

#[test]
fn a_trim_snaps_its_edge_to_the_grid_unless_free() {
    assert_eq!(
        trim(ItemEdge::Start, 78_000.0, true),
        (72_000, 96_000, Some(72_000))
    );
    assert_eq!(
        trim(ItemEdge::Start, 78_000.0, false),
        (78_000, 96_000, None)
    );
    assert_eq!(
        trim(ItemEdge::End, 115_000.0, true),
        (48_000, 120_000, Some(120_000))
    );
}

#[test]
fn a_trim_stays_inside_the_take_and_keeps_one_frame() {
    assert_eq!(
        trim(ItemEdge::Start, -90_000.0, true),
        (36_000, 96_000, None)
    );
    assert_eq!(
        trim(ItemEdge::Start, 200_000.0, false),
        (95_999, 96_000, None)
    );
    assert_eq!(
        trim(ItemEdge::End, 900_000.0, true),
        (48_000, 156_000, None)
    );
    assert_eq!(trim(ItemEdge::End, 0.0, false), (48_000, 48_001, None));
}

#[test]
fn a_trim_sends_the_moved_edge_and_previews_the_trimmed_item() {
    let item = trimmed_item();
    let grid = grid_lines(EDIT_VIEW, 1000.0, RATE, FOUR_FOUR);
    let start = TrimDrag::new(5, ItemEdge::Start, 48_000.0);
    let preview = trim_preview(&start, std::slice::from_ref(&item), &grid, 78_000.0, true).unwrap();
    assert_eq!(
        preview.command(&item),
        Some(Command::ItemTrim {
            id: 5,
            start: Some(72_000),
            end: None,
        })
    );
    assert_eq!(
        preview.apply(&item),
        ItemBlock {
            position: 72_000,
            offset: 36_000,
            length: 24_000,
            ..item.clone()
        }
    );
    let end = TrimDrag::new(5, ItemEdge::End, 96_000.0);
    let still = trim_preview(&end, std::slice::from_ref(&item), &grid, 96_000.0, false).unwrap();
    assert_eq!(still.command(&item), None);
    let grown = trim_preview(&end, std::slice::from_ref(&item), &grid, 100_000.0, false).unwrap();
    assert_eq!(
        grown.command(&item),
        Some(Command::ItemTrim {
            id: 5,
            start: None,
            end: Some(100_000),
        })
    );
    assert_eq!(
        trim_preview(
            &TrimDrag::new(9, ItemEdge::End, 0.0),
            &[item],
            &grid,
            0.0,
            true
        ),
        None
    );
}

#[test]
fn trim_keys_step_an_edge_inside_the_take() {
    let item = trimmed_item();
    assert_eq!(
        trim_step_command(&item, ItemEdge::End, 24_000.0),
        Some(Command::ItemTrim {
            id: 5,
            start: None,
            end: Some(120_000),
        })
    );
    assert_eq!(
        trim_step_command(&item, ItemEdge::Start, -24_000.0),
        Some(Command::ItemTrim {
            id: 5,
            start: Some(36_000),
            end: None,
        })
    );
    let at_take_end = ItemBlock {
        length: 108_000,
        ..trimmed_item()
    };
    assert_eq!(
        trim_step_command(&at_take_end, ItemEdge::End, 24_000.0),
        None
    );
    assert_eq!(
        press(ItemKey::Left, (false, true, false)),
        Some(ItemNodeAction::Trim(ItemEdge::End, -1))
    );
    assert_eq!(
        press(ItemKey::Right, (true, true, false)),
        Some(ItemNodeAction::Trim(ItemEdge::Start, 1))
    );
    assert_eq!(press(ItemKey::Right, (false, true, true)), None);
}

#[test]
fn t_duplicate_copy_and_paste_keys() {
    let mut items = edit_items();
    items[0].takes = 3;
    items[0].take = 2;
    let mut status = project_status("stopped");
    status["selected"] = json!(1);
    status["clipboard"] = json!(0);
    let key = |key, status: &Value, ids: &[u64]| {
        item_key_to_command(key, status, &items, &selection(ids), Some(EDIT_VIEW))
    };
    assert_eq!(
        key(KeyEvent::T, &status, &[3]),
        Some(Command::ItemTake { id: 3, take: 0 }),
        "T wraps to the first Take"
    );
    assert_eq!(key(KeyEvent::T, &status, &[1]), None, "one Take only");
    assert_eq!(
        key(KeyEvent::T, &status, &[1, 3]),
        None,
        "more than one Item"
    );
    assert_eq!(
        key(KeyEvent::CtrlD, &status, &[1, 3]),
        Some(Command::ItemDuplicate(vec![1, 3]))
    );
    assert_eq!(
        key(KeyEvent::CtrlC, &status, &[3]),
        Some(Command::ItemCopy(vec![3]))
    );
    assert_eq!(key(KeyEvent::CtrlD, &status, &[]), None);
    assert_eq!(key(KeyEvent::CtrlC, &status, &[]), None);
    assert_eq!(key(KeyEvent::CtrlV, &status, &[]), None, "empty clipboard");
    status["clipboard"] = json!(2);
    assert_eq!(
        key(KeyEvent::CtrlV, &status, &[]),
        Some(Command::ItemPaste {
            track: 1,
            at: 36_000
        })
    );
    for edit in [KeyEvent::CtrlD, KeyEvent::CtrlC, KeyEvent::CtrlV] {
        assert!(key_active(edit, KeyFocus::Item), "{edit:?}");
        assert!(!key_active(edit, KeyFocus::TextField), "{edit:?}");
        assert_eq!(project_key_to_command(edit, &status, &[]), None, "{edit:?}");
    }
}

#[test]
fn take_names_lists_an_items_takes_by_file_name() {
    let payload = json!({"tracks": [
        {"id": 0, "items": [{"id": 4, "position": 0, "offset": 0, "length": 10, "take": 1,
         "takes": ["takes/a-1.wav", "takes/a-2.wav", "b.wav"]}]},
    ]});
    assert_eq!(take_names(&payload, 4), ["a-1", "a-2", "b"]);
    assert_eq!(take_names(&payload, 5), Vec::<String>::new());
}

#[test]
fn new_items_after_a_duplicate_or_paste_become_the_selection() {
    let before = [3, 1, 5];
    let mut after = edit_items();
    after.push(block(8, GUITAR, 144_000, 96_000));
    after.push(block(9, BASS, 240_000, 240_000));
    let mut chosen = selection(&[3, 1]);
    chosen.select_new(&before, &after);
    assert_eq!(chosen.ids(), [8, 9]);
    let mut unchanged = selection(&[3]);
    unchanged.select_new(&before, &edit_items());
    assert_eq!(unchanged.ids(), [3], "nothing new: the selection stays");
}

#[test]
fn shift_f10_and_the_menu_key_open_a_focused_items_menu() {
    assert_eq!(
        press(ItemKey::F10, (false, true, false)),
        Some(ItemNodeAction::Menu)
    );
    assert_eq!(
        press(ItemKey::Menu, (false, false, false)),
        Some(ItemNodeAction::Menu)
    );
    assert_eq!(press(ItemKey::F10, (false, false, false)), None);
    assert_eq!(press(ItemKey::Menu, (true, false, false)), None);
}

/// The frames of each of `spans` (start, end; later on top) that core's `flatten` does not play, from the stretches it plays.
fn unheard_by_core(spans: &[(u64, u64)]) -> Vec<Vec<Range<u64>>> {
    let core_items: Vec<xguitare_core::project::Item> = spans
        .iter()
        .enumerate()
        .map(|(index, &(start, end))| xguitare_core::project::Item {
            id: index as u64,
            position: start,
            offset: 0,
            length: Some(end - start),
            take: 0,
            takes: vec![xguitare_core::project::Take {
                file: format!("t{index}.wav"),
                frames: end - start,
            }],
        })
        .collect();
    let takes: HashMap<String, Arc<[f32]>> = spans
        .iter()
        .enumerate()
        .map(|(index, &(start, end))| {
            let samples: Arc<[f32]> = vec![0.0; (end - start) as usize].into();
            (format!("t{index}.wav"), samples)
        })
        .collect();
    let placed = xguitare_core::flatten::flatten(&core_items, &takes, 0).unwrap();
    spans
        .iter()
        .enumerate()
        .map(|(index, &(start, end))| {
            let own = &takes[&format!("t{index}.wav")];
            let mut heard: Vec<Range<u64>> = placed
                .iter()
                .filter(|span| Arc::ptr_eq(&span.samples, own))
                .map(|span| span.position..span.position + span.len as u64)
                .collect();
            heard.sort_by_key(|range| range.start);
            let mut unheard = Vec::new();
            let mut from = start;
            for range in heard {
                if range.start > from {
                    unheard.push(from..range.start);
                }
                from = from.max(range.end);
            }
            if from < end {
                unheard.push(from..end);
            }
            unheard
        })
        .collect()
}

#[test]
fn covered_stretches_are_what_core_does_not_play() {
    let cases: [&[(u64, u64)]; 6] = [
        &[(0, 400), (100, 200)],
        &[(0, 400), (300, 600)],
        &[(200, 600), (0, 300)],
        &[(100, 200), (0, 400)],
        &[(0, 1000), (100, 500), (300, 700)],
        &[(0, 1000), (200, 400), (600, 800), (300, 700)],
    ];
    for spans in cases {
        let items: Vec<ItemBlock> = spans
            .iter()
            .enumerate()
            .map(|(index, &(start, end))| block(index as u64, GUITAR, start, end - start))
            .collect();
        let ours: Vec<Vec<Range<u64>>> = (0..spans.len() as u64)
            .map(|id| covered_stretches(&items, id))
            .collect();
        assert_eq!(ours, unheard_by_core(spans), "{spans:?}");
    }
}

#[test]
fn covered_stretches_only_count_items_above_on_the_same_track() {
    let items = vec![
        block(1, GUITAR, 0, 400),
        block(2, BASS, 100, 100),
        block(3, GUITAR, 300, 50),
        block(4, GUITAR, 320, 100),
    ];
    assert_eq!(
        covered_stretches(&items, 1),
        [Range {
            start: 300,
            end: 400
        }]
    );
    assert_eq!(
        covered_stretches(&items, 3),
        [Range {
            start: 320,
            end: 350
        }]
    );
    assert_eq!(covered_stretches(&items, 2), Vec::<Range<u64>>::new());
    assert_eq!(covered_stretches(&items, 4), Vec::<Range<u64>>::new());
}

#[test]
fn takes_clean_reports_the_takes_deleted_and_the_space_freed() {
    let cleaned = |deleted: u64, bytes: u64| {
        response_message(
            &Request::TakesClean {
                files: vec!["takes/a.wav".to_string()],
            },
            Ok(Response::Ok(
                json!({"deleted": deleted, "bytes": bytes, "failed": []}),
            )),
        )
    };
    assert_eq!(
        cleaned(1, 800),
        Some(Message::Notice("Deleted 1 Take, 800 B freed".to_string()))
    );
    assert_eq!(
        cleaned(3, 12_345_678),
        Some(Message::Notice(
            "Deleted 3 Takes, 12.3 MB freed".to_string()
        ))
    );
}

#[test]
fn takes_clean_names_the_first_file_it_could_not_delete() {
    let message = response_message(
        &Request::TakesClean {
            files: vec!["takes/a.wav".to_string(), "takes/b\u{202e}.wav".to_string()],
        },
        Ok(Response::Ok(json!({"deleted": 1, "bytes": 800, "failed": [
            {"file": "takes/b\u{202e}.wav", "error": "Permission denied (os error 13)"},
            {"file": "takes/c.wav", "error": "not a regular file"},
        ]}))),
    );
    assert_eq!(
        message,
        Some(Message::Error {
            title: "Could not clean up the Takes",
            detail: "Deleted 1 Take, 800 B freed; could not delete takes/b\\u{202e}.wav: Permission denied (os error 13), and 1 more".to_string(),
        })
    );
}

#[test]
fn a_clean_up_stopped_by_an_error_reports_what_went_before() {
    let req = Request::TakesClean {
        files: vec!["takes/a.wav".to_string()],
    };
    let stopped = |cleaned: Option<Value>| {
        takes_cleaned_message(
            &req,
            Err(TakesCleanStopped {
                cleaned,
                error: "cannot clean takes while recording".to_string(),
            }),
        )
    };
    assert_eq!(
        stopped(Some(json!({"deleted": 500, "bytes": 2000, "failed": []}))),
        Some(Message::Error {
            title: "Could not clean up the Takes",
            detail: "Deleted 500 Takes, 2.0 KB freed; cannot clean takes while recording"
                .to_string(),
        })
    );
    assert_eq!(
        stopped(None),
        Some(Message::Error {
            title: "Could not clean up the Takes",
            detail: "cannot clean takes while recording".to_string(),
        })
    );
}

#[test]
fn clean_takes_sends_takes_clean_and_the_refresh_sends_nothing_itself() {
    let files = vec!["takes/a.wav".to_string(), "takes/b.wav".to_string()];
    let queued = vec![
        Command::RefreshUnusedTakes,
        Command::CleanTakes(files.clone()),
    ];
    let (outgoing, quit) = requests_until_quit(queued, None, None);
    assert!(!quit);
    assert_eq!(outgoing, [Outgoing::Request(Request::TakesClean { files })]);
}

#[test]
fn unused_takes_lists_names_sizes_and_the_total() {
    let reply = json!({"files": [
        {"file": "takes/guitar-2.wav", "bytes": 1_234_567},
        {"file": "takes/bass\u{202e}vaw.wav", "bytes": 800},
    ], "bytes": 1_235_367});
    let unused = unused_takes(&reply);
    assert_eq!(
        unused.files,
        [
            "takes/guitar-2.wav".to_string(),
            "takes/bass\u{202e}vaw.wav".to_string()
        ]
    );
    assert_eq!(
        unused.rows,
        [
            ("guitar-2.wav".to_string(), "1.2 MB".to_string()),
            ("bass\\u{202e}vaw.wav".to_string(), "800 B".to_string()),
        ]
    );
    assert_eq!(unused.total, "1.2 MB");
    assert_eq!(
        CleanTakesView::from_reply(&Err("takes is not a directory".to_string())),
        CleanTakesView::Failed("takes is not a directory".to_string())
    );
    assert_eq!(
        CleanTakesView::from_reply(&Ok(reply)),
        CleanTakesView::Listed(unused)
    );
}
