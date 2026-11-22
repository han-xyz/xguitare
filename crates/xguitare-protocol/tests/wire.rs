use serde_json::json;
use xguitare_protocol::{
    BankKind, BusRef, IndexOrName, LoopRegion, Position, RenderBits, RenderRange, Request,
    Response, SendLevel, TrackRef, TransportAction,
};

fn assert_round_trip(line: &str, expected: &Request) {
    let parsed: Request = serde_json::from_str(line).unwrap();
    assert_eq!(&parsed, expected, "{line}");
    let reparsed: Request = serde_json::from_str(&expected.to_line()).unwrap();
    assert_eq!(&reparsed, expected, "{line}");
}

#[test]
fn wire_types_keep_their_json() {
    let cases = [
        (
            r#"{"cmd":"transport","action":"locate","position":"2.1"}"#,
            Request::Transport {
                action: TransportAction::Locate,
                position: Some(Position::Text("2.1".into())),
            },
        ),
        (
            r#"{"cmd":"transport","action":"locate","position":96000}"#,
            Request::Transport {
                action: TransportAction::Locate,
                position: Some(Position::Frames(96_000)),
            },
        ),
        (
            r#"{"cmd":"track_select","track":"Bass"}"#,
            Request::TrackSelect {
                track: IndexOrName::Name("Bass".into()),
            },
        ),
        (
            r#"{"cmd":"bus_set","bus":0,"volume":-3,"solo":true}"#,
            Request::BusSet {
                bus: IndexOrName::Index(0),
                name: None,
                volume: Some(-3.0),
                pan: None,
                mute: None,
                solo: Some(true),
            },
        ),
        (
            r#"{"cmd":"send_set","track":"Guitar","bus":"Verb","level":-6}"#,
            Request::SendSet {
                track: IndexOrName::Name("Guitar".into()),
                bus: IndexOrName::Name("Verb".into()),
                level: Some(SendLevel::Db(-6.0)),
                pre: None,
            },
        ),
        (
            r#"{"cmd":"send_set","track":0,"bus":0,"level":"off"}"#,
            Request::SendSet {
                track: IndexOrName::Index(0),
                bus: IndexOrName::Index(0),
                level: Some(SendLevel::Off),
                pre: None,
            },
        ),
        (
            r#"{"cmd":"loop","start":0,"end":172800,"on":true}"#,
            Request::Loop {
                start: 0,
                end: 172_800,
                on: true,
            },
        ),
        (
            r#"{"cmd":"render","range":"loop","bits":16,"name":"Chorus"}"#,
            Request::Render {
                range: Some(RenderRange::Loop),
                bits: Some(RenderBits::Pcm16),
                name: Some("Chorus".into()),
            },
        ),
        (
            r#"{"cmd":"render","bits":"32f"}"#,
            Request::Render {
                range: None,
                bits: Some(RenderBits::Float32),
                name: None,
            },
        ),
        (
            r#"{"cmd":"set_settings","bank_kind":"bass"}"#,
            Request::SetSettings {
                bank_kind: Some(BankKind::Bass),
                ir_dir: None,
                projects_dir: None,
            },
        ),
    ];
    for (line, expected) in &cases {
        assert_round_trip(line, expected);
    }

    let region = LoopRegion {
        start: 0,
        end: 172_800,
        on: true,
    };
    let status_loop = json!({"end":172_800,"on":true,"start":0});
    assert_eq!(serde_json::to_value(region).unwrap(), status_loop);
    assert_eq!(
        serde_json::from_value::<LoopRegion>(status_loop).unwrap(),
        region
    );
}

#[test]
fn mixer_requests_write_the_documented_lines() {
    let send = Request::SendSet {
        track: TrackRef::Index(0),
        bus: BusRef::Name("Verb".to_string()),
        level: Some(SendLevel::Off),
        pre: None,
    };
    assert_eq!(
        send.to_line(),
        r#"{"cmd":"send_set","track":0,"bus":"Verb","level":"off","pre":null}"#
    );
    let get = Request::Get {
        param: None,
        track: None,
        bus: None,
        master: true,
    };
    assert_eq!(get.to_line(), r#"{"cmd":"get","param":null,"master":true}"#);
    let reset = Request::ClipReset {
        track: None,
        bus: Some(BusRef::Index(2)),
        master: false,
    };
    assert_eq!(reset.to_line(), r#"{"cmd":"clip_reset","bus":2}"#);
    let level = Request::SendSet {
        track: TrackRef::Index(1),
        bus: BusRef::Index(0),
        level: Some(SendLevel::Db(-6.0)),
        pre: Some(true),
    };
    assert_eq!(
        level.to_line(),
        r#"{"cmd":"send_set","track":1,"bus":0,"level":-6.0,"pre":true}"#
    );
}

#[test]
fn response_to_line_writes_the_documented_wire_lines() {
    assert_eq!(
        Response::err("invalid request").to_line(),
        r#"{"ok":false,"error":"invalid request"}"#
    );
    assert_eq!(
        Response::err_code("a project is still closing; try again", "still_closing").to_line(),
        r#"{"ok":false,"error":"a project is still closing; try again","code":"still_closing"}"#
    );
    let ok = Response::Ok(json!({"name": "Crunch", "program": 1}));
    assert_eq!(ok.to_line(), r#"{"ok":true,"name":"Crunch","program":1}"#);
    assert_eq!(Response::from_line(&ok.to_line()).unwrap(), ok);
}
