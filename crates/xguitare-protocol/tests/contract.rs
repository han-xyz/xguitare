use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use xguitare_protocol::{
    BankKind, CODE_STILL_CLOSING, GetReply, IndexOrName, ItemStatus, ItemsReply, LoopRegion,
    PROTOCOL_VERSION, ParamStatus, Position, RenderBits, RenderRange, Request, Response, SendLevel,
    TransportAction, from_payload, position_frame, shown_file_name, to_payload,
};

fn name(text: &str) -> IndexOrName {
    IndexOrName::Name(text.to_string())
}

fn text(value: &str) -> String {
    value.to_string()
}

#[test]
#[allow(clippy::too_many_lines, reason = "one row per documented request")]
fn every_documented_request_parses_to_its_variant() {
    let cases: Vec<(&str, Request)> = vec![
        (r#"{"cmd":"status"}"#, Request::Status),
        (r#"{"cmd":"list"}"#, Request::List),
        (r#"{"cmd":"modules"}"#, Request::Modules),
        (r#"{"cmd":"tuner"}"#, Request::Tuner),
        (r#"{"cmd":"ir_list"}"#, Request::IrList),
        (r#"{"cmd":"ports"}"#, Request::Ports),
        (r#"{"cmd":"settings"}"#, Request::Settings),
        (r#"{"cmd":"project_save"}"#, Request::ProjectSave),
        (r#"{"cmd":"project_close"}"#, Request::ProjectClose),
        (r#"{"cmd":"project_list"}"#, Request::ProjectList),
        (r#"{"cmd":"project_undo"}"#, Request::ProjectUndo),
        (r#"{"cmd":"project_redo"}"#, Request::ProjectRedo),
        (r#"{"cmd":"items"}"#, Request::Items),
        (r#"{"cmd":"metronome_tap"}"#, Request::MetronomeTap),
        (r#"{"cmd":"render_cancel"}"#, Request::RenderCancel),
        (r#"{"cmd":"takes_unused"}"#, Request::TakesUnused),
        (
            r#"{"cmd":"load","preset":"Crunch"}"#,
            Request::Load {
                preset: text("Crunch"),
                track: None,
                bus: None,
                master: false,
            },
        ),
        (
            r#"{"cmd":"set","param":"preamp.gain","value":150}"#,
            Request::Set {
                param: text("preamp.gain"),
                value: 150.0,
                track: None,
                bus: None,
                master: false,
            },
        ),
        (
            r#"{"cmd":"set","param":"preamp.gain","value":20,"track":"Bass"}"#,
            Request::Set {
                param: text("preamp.gain"),
                value: 20.0,
                track: Some(name("Bass")),
                bus: None,
                master: false,
            },
        ),
        (
            r#"{"cmd":"set","param":"compressor.threshold","value":-18,"master":true}"#,
            Request::Set {
                param: text("compressor.threshold"),
                value: -18.0,
                track: None,
                bus: None,
                master: true,
            },
        ),
        (
            r#"{"cmd":"get","param":"preamp.gain"}"#,
            Request::Get {
                param: Some(text("preamp.gain")),
                track: None,
                bus: None,
                master: false,
            },
        ),
        (r#"{"cmd":"save"}"#, Request::Save { name: None }),
        (
            r#"{"cmd":"save","as":"My Lead Tone!"}"#,
            Request::Save {
                name: Some(text("My Lead Tone!")),
            },
        ),
        (
            r#"{"cmd":"rack_add","type":"delay","after":"reverb"}"#,
            Request::RackAdd {
                r#type: text("delay"),
                after: Some(text("reverb")),
                at: None,
                track: None,
                bus: None,
                master: false,
            },
        ),
        (
            r#"{"cmd":"rack_add","type":"reverb","bus":"Verb"}"#,
            Request::RackAdd {
                r#type: text("reverb"),
                after: None,
                at: None,
                track: None,
                bus: Some(name("Verb")),
                master: false,
            },
        ),
        (
            r#"{"cmd":"rack_remove","id":"delay"}"#,
            Request::RackRemove {
                id: text("delay"),
                track: None,
                bus: None,
                master: false,
            },
        ),
        (
            r#"{"cmd":"rack_move","id":"delay","to":6}"#,
            Request::RackMove {
                id: text("delay"),
                to: 6,
                track: None,
                bus: None,
                master: false,
            },
        ),
        (r#"{"cmd":"mute","on":true}"#, Request::Mute { on: true }),
        (
            r#"{"cmd":"bypass","id":"preamp","on":true}"#,
            Request::Bypass {
                id: text("preamp"),
                on: true,
                track: None,
                bus: None,
                master: false,
            },
        ),
        (
            r#"{"cmd":"rack_ir","ir":"celestion/v30.wav","id":"cabinet"}"#,
            Request::RackIr {
                ir: text("celestion/v30.wav"),
                id: Some(text("cabinet")),
                track: None,
                bus: None,
                master: false,
            },
        ),
        (
            r#"{"cmd":"set_ports","input":"In:capture_MONO","output_left":"Out:playback_FL","output_right":"Out:playback_FR"}"#,
            Request::SetPorts {
                input: text("In:capture_MONO"),
                output_left: text("Out:playback_FL"),
                output_right: text("Out:playback_FR"),
            },
        ),
        (
            r#"{"cmd":"project_new","name":"Demo Song"}"#,
            Request::ProjectNew {
                name: text("Demo Song"),
            },
        ),
        (
            r#"{"cmd":"project_open","name":"Demo Song"}"#,
            Request::ProjectOpen {
                name: text("Demo Song"),
                restore: false,
            },
        ),
        (
            r#"{"cmd":"project_open","name":"Demo Song","restore":true}"#,
            Request::ProjectOpen {
                name: text("Demo Song"),
                restore: true,
            },
        ),
        (
            r#"{"cmd":"track_add","name":"Bass"}"#,
            Request::TrackAdd {
                name: Some(text("Bass")),
            },
        ),
        (
            r#"{"cmd":"track_remove","track":"Bass"}"#,
            Request::TrackRemove {
                track: name("Bass"),
            },
        ),
        (
            r#"{"cmd":"track_set","track":0,"input":"In:capture_MONO","volume":-3,"pan":-0.25,"arm":true}"#,
            Request::TrackSet {
                track: IndexOrName::Index(0),
                input: Some(text("In:capture_MONO")),
                volume: Some(-3.0),
                pan: Some(-0.25),
                mute: None,
                solo: None,
                arm: Some(true),
                name: None,
                to_master: None,
            },
        ),
        (
            r#"{"cmd":"transport","action":"play"}"#,
            Request::Transport {
                action: TransportAction::Play,
                position: None,
            },
        ),
        (
            r#"{"cmd":"transport","action":"stop"}"#,
            Request::Transport {
                action: TransportAction::Stop,
                position: None,
            },
        ),
        (
            r#"{"cmd":"transport","action":"record"}"#,
            Request::Transport {
                action: TransportAction::Record,
                position: None,
            },
        ),
        (
            r#"{"cmd":"tempo","bpm":100,"meter":[3,4]}"#,
            Request::Tempo {
                bpm: Some(100.0),
                meter: Some((3, 4)),
            },
        ),
        (
            r#"{"cmd":"click","on":true,"count_in":1}"#,
            Request::Click {
                on: Some(true),
                count_in: Some(1),
            },
        ),
        (
            r#"{"cmd":"metronome","on":true,"bpm":90,"meter":[3,4],"volume":-12}"#,
            Request::Metronome {
                on: Some(true),
                bpm: Some(90.0),
                meter: Some((3, 4)),
                volume: Some(-12.0),
            },
        ),
        (
            r#"{"cmd":"bus_add","name":"Verb"}"#,
            Request::BusAdd {
                name: Some(text("Verb")),
            },
        ),
        (
            r#"{"cmd":"bus_remove","bus":"Verb"}"#,
            Request::BusRemove { bus: name("Verb") },
        ),
        (
            r#"{"cmd":"master_set","volume":-3}"#,
            Request::MasterSet {
                volume: SendLevel::Db(-3.0),
            },
        ),
        (
            r#"{"cmd":"master_set","volume":"off"}"#,
            Request::MasterSet {
                volume: SendLevel::Off,
            },
        ),
        (
            r#"{"cmd":"render"}"#,
            Request::Render {
                range: None,
                bits: None,
                name: None,
            },
        ),
        (
            r#"{"cmd":"render","range":"project","bits":24}"#,
            Request::Render {
                range: Some(RenderRange::Project),
                bits: Some(RenderBits::Pcm24),
                name: None,
            },
        ),
        (
            r#"{"cmd":"clip_reset","bus":"Verb"}"#,
            Request::ClipReset {
                track: None,
                bus: Some(name("Verb")),
                master: false,
            },
        ),
        (
            r#"{"cmd":"item_move","ids":[3,4],"delta":-48000,"track":"Bass"}"#,
            Request::ItemMove {
                ids: vec![3, 4],
                delta: -48_000,
                track: Some(name("Bass")),
            },
        ),
        (
            r#"{"cmd":"item_trim","id":3,"start":100000,"end":180000}"#,
            Request::ItemTrim {
                id: 3,
                start: Some(100_000),
                end: Some(180_000),
            },
        ),
        (
            r#"{"cmd":"item_split","ids":[3],"at":144000}"#,
            Request::ItemSplit {
                ids: Some(vec![3]),
                at: 144_000,
            },
        ),
        (
            r#"{"cmd":"item_delete","ids":[3,7]}"#,
            Request::ItemDelete { ids: vec![3, 7] },
        ),
        (
            r#"{"cmd":"item_take","id":3,"take":0}"#,
            Request::ItemTake { id: 3, take: 0 },
        ),
        (
            r#"{"cmd":"item_duplicate","ids":[3,4]}"#,
            Request::ItemDuplicate { ids: vec![3, 4] },
        ),
        (
            r#"{"cmd":"item_copy","ids":[3,4]}"#,
            Request::ItemCopy { ids: vec![3, 4] },
        ),
        (
            r#"{"cmd":"item_paste","track":"Bass","at":192000}"#,
            Request::ItemPaste {
                track: name("Bass"),
                at: 192_000,
            },
        ),
        (
            r#"{"cmd":"take_peaks","file":"takes/guitar-2.wav","first":0,"count":16384}"#,
            Request::TakePeaks {
                file: text("takes/guitar-2.wav"),
                first: Some(0),
                count: Some(16_384),
            },
        ),
        (
            r#"{"cmd":"takes_clean","files":["takes/guitar-2.wav","takes/guitar-3.wav"]}"#,
            Request::TakesClean {
                files: vec![text("takes/guitar-2.wav"), text("takes/guitar-3.wav")],
            },
        ),
        (
            r#"{"cmd":"set_settings","bank_kind":"guitar","ir_dir":"/irs","projects_dir":"/p"}"#,
            Request::SetSettings {
                bank_kind: Some(BankKind::Guitar),
                ir_dir: Some("/irs".into()),
                projects_dir: Some("/p".into()),
            },
        ),
    ];
    for (line, expected) in &cases {
        let parsed: Request = serde_json::from_str(line).unwrap();
        assert_eq!(&parsed, expected, "{line}");
        let again: Request = serde_json::from_str(&expected.to_line()).unwrap();
        assert_eq!(&again, expected, "round trip of {line}");
    }
}

#[test]
fn requests_write_the_documented_fields() {
    let load = Request::Load {
        preset: text("Crunch"),
        track: None,
        bus: None,
        master: false,
    };
    assert_eq!(load.to_line(), r#"{"cmd":"load","preset":"Crunch"}"#);
    let master = Request::Set {
        param: text("a.b"),
        value: 1.5,
        track: None,
        bus: None,
        master: true,
    };
    assert_eq!(
        master.to_line(),
        r#"{"cmd":"set","param":"a.b","value":1.5,"master":true}"#
    );
    assert_eq!(
        Request::Save {
            name: Some(text("X"))
        }
        .to_line(),
        r#"{"cmd":"save","as":"X"}"#
    );
    assert_eq!(Request::ProjectSave.to_line(), r#"{"cmd":"project_save"}"#);
    assert_eq!(
        Request::RackIr {
            ir: text("a.wav"),
            id: None,
            track: Some(name("Bass")),
            bus: None,
            master: false,
        }
        .to_line(),
        r#"{"cmd":"rack_ir","ir":"a.wav","id":null,"track":"Bass"}"#
    );
    assert_eq!(
        Request::Transport {
            action: TransportAction::Locate,
            position: Some(Position::Text("1.1".into())),
        }
        .to_line(),
        r#"{"cmd":"transport","action":"locate","position":"1.1"}"#
    );
    assert_eq!(
        Request::Render {
            range: Some(RenderRange::Loop),
            bits: Some(RenderBits::Pcm24),
            name: None,
        }
        .to_line(),
        r#"{"cmd":"render","range":"loop","bits":24}"#
    );
    assert_eq!(
        Request::Render {
            range: Some(RenderRange::Project),
            bits: Some(RenderBits::Float32),
            name: None,
        }
        .to_line(),
        r#"{"cmd":"render","range":"project","bits":"32f"}"#
    );
    assert_eq!(
        Request::SetSettings {
            bank_kind: Some(BankKind::Guitar),
            ir_dir: None,
            projects_dir: None,
        }
        .to_line(),
        r#"{"cmd":"set_settings","bank_kind":"guitar","ir_dir":null}"#
    );
}

#[test]
fn malformed_wire_values_are_refused_with_their_messages() {
    let bits = serde_json::from_str::<RenderBits>("20").unwrap_err();
    assert!(bits.to_string().contains(r#"bits must be 16, 24 or "32f""#));
    let word = serde_json::from_str::<RenderBits>(r#""32""#).unwrap_err();
    assert!(word.to_string().contains(r#"bits must be 16, 24 or "32f""#));
    let level = serde_json::from_str::<SendLevel>(r#""loud""#).unwrap_err();
    assert!(
        level
            .to_string()
            .contains(r#"level must be a number in dB or "off""#)
    );
    assert!(serde_json::from_str::<BankKind>(r#""Bass""#).is_err());
    assert!(serde_json::from_str::<Request>(r#"{"cmd":"Status"}"#).is_err());
    assert!(serde_json::from_str::<Request>(r#"{"cmd":"nope"}"#).is_err());
    assert!(serde_json::from_str::<Request>(r#"{"cmd":"rack_move","id":"a"}"#).is_err());
    assert!(serde_json::from_str::<TransportAction>(r#""Play""#).is_err());
    assert!(serde_json::from_str::<RenderRange>(r#""whole""#).is_err());
    assert!(
        serde_json::from_str::<LoopRegion>(r#"{"start":0,"end":1,"on":true,"extra":1}"#).is_err()
    );
}

#[test]
fn bank_kind_is_lowercase_on_the_wire_and_in_display() {
    assert_eq!(
        serde_json::to_string(&BankKind::Guitar).unwrap(),
        r#""guitar""#
    );
    assert_eq!(serde_json::to_string(&BankKind::Bass).unwrap(), r#""bass""#);
    assert_eq!(BankKind::Guitar.to_string(), "guitar");
    assert_eq!(BankKind::Bass.to_string(), "bass");
    assert_eq!(BankKind::default(), BankKind::Guitar);
}

#[test]
fn index_or_name_and_position_pick_their_shape_from_the_json_type() {
    assert_eq!(
        serde_json::from_str::<IndexOrName>("2").unwrap(),
        IndexOrName::Index(2)
    );
    assert_eq!(
        serde_json::from_str::<IndexOrName>(r#""2""#).unwrap(),
        name("2")
    );
    assert_eq!(serde_json::to_string(&IndexOrName::Index(2)).unwrap(), "2");
    assert_eq!(
        serde_json::from_str::<IndexOrName>("70000").unwrap(),
        IndexOrName::Index(70_000)
    );
    assert_eq!(
        serde_json::from_str::<Position>("96000").unwrap(),
        Position::Frames(96_000)
    );
    assert_eq!(
        serde_json::from_str::<Position>(r#""1.5s""#).unwrap(),
        Position::Text(text("1.5s"))
    );
    assert_eq!(serde_json::to_string(&Position::Frames(7)).unwrap(), "7");
    assert_eq!(
        serde_json::from_str::<Position>("5000000000").unwrap(),
        Position::Frames(5_000_000_000)
    );
}

#[test]
fn responses_write_ok_with_flattened_payload_and_error_shape() {
    let ok = Response::Ok(json!({"a": 1, "b": [true]}));
    assert_eq!(
        serde_json::to_string(&ok).unwrap(),
        r#"{"ok":true,"a":1,"b":[true]}"#
    );
    assert_eq!(
        serde_json::to_string(&Response::Ok(json!(null))).unwrap(),
        r#"{"ok":true}"#
    );
    assert_eq!(
        serde_json::to_string(&Response::err("no such preset")).unwrap(),
        r#"{"ok":false,"error":"no such preset"}"#
    );
    assert_eq!(
        Response::from_line(r#"{"ok":true,"preset":"Crunch"}"#).unwrap(),
        Response::Ok(json!({"preset": "Crunch"}))
    );
    assert_eq!(
        Response::from_line(r#"{"ok":false,"error":"x"}"#).unwrap(),
        Response::err("x")
    );
    assert!(Response::from_line(r#"{"ok":false}"#).is_err());
    assert!(Response::from_line(r#"{"ok":"yes"}"#).is_err());
    assert!(Response::from_line(r#"{"error":"x"}"#).is_err());
}

#[test]
fn an_error_code_is_written_when_given_and_any_code_or_extra_field_parses() {
    assert_eq!(PROTOCOL_VERSION, 1);
    let closing = Response::err_code("a project is still closing; try again", CODE_STILL_CLOSING);
    let line =
        r#"{"ok":false,"error":"a project is still closing; try again","code":"still_closing"}"#;
    assert_eq!(serde_json::to_string(&closing).unwrap(), line);
    assert_eq!(Response::from_line(line).unwrap(), closing);
    assert_eq!(
        Response::from_line(r#"{"ok":false,"error":"x","code":"from_a_newer_daemon","hint":1}"#)
            .unwrap(),
        Response::err_code("x", "from_a_newer_daemon")
    );
    assert_eq!(
        Response::from_line(r#"{"ok":false,"error":"x","hint":[1]}"#).unwrap(),
        Response::err("x")
    );
}

#[test]
fn position_frame_reads_bars_beats_and_seconds() {
    let at = |text: &str| position_frame(text, 48_000, 120.0, 4);
    assert_eq!(at("1.1"), Ok(0.0));
    assert_eq!(at("1"), Ok(0.0));
    assert_eq!(at("2.1"), Ok(4.0 * 24_000.0));
    assert_eq!(at("2"), Ok(4.0 * 24_000.0));
    assert_eq!(at("1.4"), Ok(3.0 * 24_000.0));
    assert_eq!(at("0.5s"), Ok(24_000.0));
    assert_eq!(at("0s"), Ok(0.0));
    assert_eq!(at("0.00002s"), Ok(1.0));
    assert_eq!(position_frame("1.3", 48_000, 90.0, 3), Ok(2.0 * 32_000.0));
    assert_eq!(position_frame("2.1", 48_000, 90.0, 3), Ok(3.0 * 32_000.0));
    let bad = Err("position must be <bar>.<beat> or <seconds>s".to_string());
    for text in [
        "0.1", "1.0", "0", "1.5", "-1s", "s", "abc", "", "1.x", "NaNs", "infs", "1.2.3",
    ] {
        assert_eq!(at(text), bad, "{text:?}");
    }
}

#[test]
fn shown_file_name_escapes_control_and_bidi_characters_only() {
    assert_eq!(shown_file_name("take 1 é日.wav"), "take 1 é日.wav");
    assert_eq!(shown_file_name("a\nb\tc\u{7f}"), "a\\nb\\tc\\u{7f}");
    assert_eq!(shown_file_name("a\u{202e}b"), "a\\u{202e}b");
    assert_eq!(
        shown_file_name("\u{202a}\u{202d}\u{2066}\u{2069}"),
        "\\u{202a}\\u{202d}\\u{2066}\\u{2069}"
    );
    assert_eq!(
        shown_file_name("\u{2029}\u{2065}\u{202f}"),
        "\u{2029}\u{2065}\u{202f}"
    );
}

#[test]
fn an_error_with_a_code_of_another_shape_parses_as_an_error_with_its_text() {
    for extra in [
        r#","code":null"#,
        r#","code":7"#,
        r#","code":["still_closing"]"#,
        r#","code":{"a":1}"#,
        r#","code":true"#,
    ] {
        let line = format!(r#"{{"ok":false,"error":"boom"{extra}}}"#);
        assert_eq!(
            Response::from_line(&line).unwrap(),
            Response::err("boom"),
            "{line}"
        );
    }
    assert_eq!(
        serde_json::to_string(&Response::err("boom")).unwrap(),
        r#"{"ok":false,"error":"boom"}"#
    );
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replies")
}

/// The payload of every fixture whose name starts with `prefix`, by file name.
fn fixtures(prefix: &str) -> Vec<(String, Value)> {
    let mut found: Vec<(String, Value)> = fs::read_dir(fixture_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(prefix)
        })
        .map(|path| {
            let line = fs::read_to_string(&path).unwrap();
            let Response::Ok(payload) = Response::from_line(line.trim_end()).unwrap() else {
                panic!("{} holds an error reply", path.display());
            };
            (path.display().to_string(), payload)
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn assert_round_trips<T: Serialize + DeserializeOwned>(prefix: &str, count: usize) {
    let found = fixtures(prefix);
    assert_eq!(found.len(), count, "fixtures named {prefix}*");
    for (file, payload) in found {
        let reply: T = from_payload(&payload).unwrap();
        assert_eq!(to_payload(&reply), payload, "{file}");
    }
}

#[test]
fn every_get_and_items_fixture_round_trips_through_its_struct() {
    assert_round_trips::<GetReply>("get-", 4);
    assert_round_trips::<ItemsReply>("items", 1);
}

#[test]
fn a_cabinet_without_an_ir_keeps_ir_null_and_other_modules_leave_it_out() {
    let payload = json!({"params": [
        {"module": "cabinet", "type": "cabinet", "param": "level", "value": 0.0, "unit": "dB",
         "min": -20.0, "max": 20.0, "default": 0.0, "integer": false, "labels": [], "bypass": false, "ir": null},
        {"module": "gain", "type": "gain", "param": "level", "value": 0.0, "unit": "dB",
         "min": -60.0, "max": 20.0, "default": 0.0, "integer": false, "labels": [], "bypass": false},
    ]});
    let reply: GetReply = from_payload(&payload).unwrap();
    assert_eq!(reply.params[0].ir, Some(None));
    assert_eq!(reply.params[1].ir, None);
    assert_eq!(to_payload(&reply), payload);
}

#[test]
fn an_item_without_a_take_keeps_take_frames_null() {
    let payload = json!({"id": 4, "position": 0, "bars_beats": "1.1", "offset": 0, "length": 0,
        "take": 0, "takes": [], "take_frames": null});
    let item: ItemStatus = from_payload(&payload).unwrap();
    assert_eq!(item.take_frames, None);
    assert_eq!(to_payload(&item), payload);
}

#[test]
fn replies_with_extra_keys_at_top_level_and_nested_deserialise() {
    let get = json!({"params": [{"module": "gain", "param": "level", "value": 1.5, "hint": "red"}],
        "from_a_newer_daemon": 1});
    let reply: GetReply = from_payload(&get).unwrap();
    assert_eq!(reply.params[0].value, 1.5);
    let items = json!({"sample_rate": 48000, "extra": true,
        "tracks": [{"id": 0, "name": "Guitar", "colour": "red",
            "items": [{"id": 1, "take_frames": 10, "fade": 3}]}]});
    let reply: ItemsReply = from_payload(&items).unwrap();
    assert_eq!(reply.tracks[0].items[0].take_frames, Some(10));
}

#[test]
fn a_reply_missing_keys_deserialises_to_the_defaults() {
    assert_eq!(
        from_payload::<GetReply>(&json!({})).unwrap(),
        GetReply::default()
    );
    let param: ParamStatus = from_payload(&json!({"module": "gain"})).unwrap();
    assert_eq!(
        param,
        ParamStatus {
            module: "gain".to_string(),
            ..ParamStatus::default()
        }
    );
    let items: ItemsReply = from_payload(&json!({"tracks": [{"items": [{}]}]})).unwrap();
    assert_eq!(items.sample_rate, 0);
    assert_eq!(items.tracks[0].items[0], ItemStatus::default());
}
