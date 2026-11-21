use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::{Value, json};
use xguitare_core::arrangement::ArrangementCommand;
use xguitare_core::control::{EngineState, handle};
use xguitare_core::engine::{AudioQueues, Command, EngineShared, engine_queues};
use xguitare_core::flatten::PlacedTake;
use xguitare_core::metronome::MetronomeSettings;
use xguitare_core::project::{Item, Project, Take};
use xguitare_core::tuner::Tuner;
use xguitare_core::{Bank, BuildWarning, Config};
use xguitare_protocol::{Request, Response, TrackRef, TransportAction};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn state(test: &str) -> (EngineState, AudioQueues, TempDir) {
    let dir = std::env::temp_dir().join(format!("xguitare-items-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bank_dir = dir.join("bank");
    fs::create_dir_all(&bank_dir).unwrap();
    let config_path = dir.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let mut config = Config::load(Some(&config_path)).unwrap();
    config.bank_dir.clone_from(&bank_dir);
    config.projects_dir = dir.join("projects");
    let shared = Arc::new(EngineShared::default());
    shared.sample_rate.store(48_000, Ordering::Relaxed);
    let (queues, audio, tap) = engine_queues(64);
    let tuner = Tuner::new(tap, 48_000.0, config.tuner_reference_hz);
    let state = EngineState::new(
        Bank::load_dir(&bank_dir).unwrap(),
        shared,
        queues,
        tuner,
        config,
        48_000.0,
        128,
    );
    (state, audio, TempDir(dir))
}

fn ok((response, _): (Response, Vec<BuildWarning>)) -> Value {
    match response {
        Response::Ok(payload) => payload,
        Response::Err { error, .. } => panic!("expected ok, got error {error}"),
    }
}

fn error((response, _): (Response, Vec<BuildWarning>)) -> String {
    match response {
        Response::Err { error, .. } => error,
        Response::Ok(payload) => panic!("expected error, got {payload}"),
    }
}

fn write_take(path: &Path, samples: &[f32]) {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn item(id: u64, position: u64, file: &str, frames: u64) -> Item {
    Item {
        id,
        position,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![Take {
            file: file.to_string(),
            frames,
        }],
    }
}

/// Opens a Project "Song" with one Track per entry of `tracks`, holding its Items, and a Take file per entry of `takes` at `(level, frames)`.
fn open_with(
    state: &mut EngineState,
    tracks: Vec<(&str, Vec<Item>)>,
    takes: &[(&str, f32, usize)],
) {
    ok(handle(
        state,
        Request::ProjectNew {
            name: "Song".to_string(),
        },
    ));
    for (name, _) in &tracks {
        ok(handle(
            state,
            Request::TrackAdd {
                name: Some((*name).to_string()),
            },
        ));
    }
    ok(handle(state, Request::ProjectClose));
    let folder = state.config.projects_dir.join("song");
    for &(file, level, frames) in takes {
        write_take(&folder.join(file), &vec![level; frames]);
    }
    let mut project = Project::load(&folder).unwrap().0;
    for (track, (_, items)) in project.tracks.iter_mut().zip(tracks) {
        track.items = items;
    }
    project.next_item_id = project
        .tracks
        .iter()
        .flat_map(|track| &track.items)
        .map(|item| item.id + 1)
        .max()
        .unwrap_or(1);
    project.save(&folder).unwrap();
    ok(handle(
        state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
}

const TAKES: [(&str, f32, usize); 2] = [("takes/a.wav", 0.25, 4800), ("takes/b.wav", 0.5, 4800)];

fn guitar_and_bass(state: &mut EngineState) {
    open_with(
        state,
        vec![
            (
                "Guitar",
                vec![
                    item(1, 1000, "takes/a.wav", 4800),
                    item(2, 3000, "takes/b.wav", 4800),
                    item(3, 20_000, "takes/b.wav", 4800),
                ],
            ),
            ("Bass", vec![item(4, 0, "takes/b.wav", 4800)]),
        ],
        &TAKES,
    );
}

/// Each Track's Items as `[id, position, offset, length]`, in Track order and stacking order.
fn layout(state: &mut EngineState) -> Vec<Vec<[u64; 4]>> {
    let items = ok(handle(state, Request::Items));
    items["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| {
            track["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| {
                    ["id", "position", "offset", "length"].map(|key| item[key].as_u64().unwrap())
                })
                .collect()
        })
        .collect()
}

fn drain_commands(audio: &mut AudioQueues) -> Vec<Command> {
    let mut commands = Vec::new();
    while let Ok(command) = audio.commands.pop() {
        commands.push(command);
    }
    commands
}

/// The `SetTrackItems` commands sent, as (Track, Item list).
fn item_swaps(audio: &mut AudioQueues) -> Vec<(usize, Vec<PlacedTake>)> {
    drain_commands(audio)
        .into_iter()
        .filter_map(|command| match command {
            Command::Arrangement(ArrangementCommand::SetTrackItems { track, items }) => {
                Some((track, items))
            }
            _ => None,
        })
        .collect()
}

fn item_move(ids: &[u64], delta: i64, track: Option<TrackRef>) -> Request {
    Request::ItemMove {
        ids: ids.to_vec(),
        delta,
        track,
    }
}

fn item_trim(id: u64, start: Option<u64>, end: Option<u64>) -> Request {
    Request::ItemTrim { id, start, end }
}

fn item_split(ids: Option<&[u64]>, at: u64) -> Request {
    Request::ItemSplit {
        ids: ids.map(<[u64]>::to_vec),
        at,
    }
}

fn item_delete(ids: &[u64]) -> Request {
    Request::ItemDelete { ids: ids.to_vec() }
}

fn item_take(id: u64, take: usize) -> Request {
    Request::ItemTake { id, take }
}

fn every_edit() -> [Request; 5] {
    [
        item_move(&[1], 10, None),
        item_trim(1, Some(1100), None),
        item_split(None, 2000),
        item_delete(&[1]),
        item_take(1, 0),
    ]
}

#[test]
fn item_edits_need_an_open_project() {
    let (mut state, _audio, _dir) = state("none-open");
    for request in every_edit() {
        assert_eq!(error(handle(&mut state, request)), "no project open");
    }
}

#[test]
fn item_edits_name_an_unknown_item() {
    let (mut state, _audio, _dir) = state("unknown");
    guitar_and_bass(&mut state);
    for request in [
        item_move(&[1, 9], 10, None),
        item_trim(9, Some(1100), None),
        item_split(Some(&[9]), 2000),
        item_delete(&[9]),
        item_take(9, 0),
    ] {
        assert_eq!(error(handle(&mut state, request)), "no such item: 9");
    }
    assert_eq!(
        error(handle(&mut state, item_delete(&[]))),
        "give at least one item"
    );
}

#[test]
fn item_edits_are_refused_while_recording() {
    let (mut state, _audio, _dir) = state("recording");
    guitar_and_bass(&mut state);
    ok(handle(
        &mut state,
        Request::TrackSet {
            track: TrackRef::Index(1),
            input: None,
            volume: None,
            pan: None,
            mute: None,
            solo: None,
            arm: Some(true),
            name: None,
            to_master: None,
        },
    ));
    ok(handle(
        &mut state,
        Request::Transport {
            action: TransportAction::Record,
            position: None,
        },
    ));
    let before = layout(&mut state);
    for request in every_edit() {
        assert_eq!(
            error(handle(&mut state, request)),
            "cannot edit items while recording"
        );
    }
    assert_eq!(layout(&mut state), before);
}

#[test]
fn item_move_shifts_every_item_by_one_clamped_delta_onto_the_top() {
    let (mut state, mut audio, _dir) = state("move");
    guitar_and_bass(&mut state);
    drain_commands(&mut audio);
    let revision = state.revision;
    let moved = ok(handle(&mut state, item_move(&[2, 1], -1500, None)));
    assert!(state.revision > revision);
    assert_eq!(
        moved,
        json!({"items": [
            {"track": 0, "id": 1, "position": 0, "bars_beats": "1.1", "offset": 0, "length": 4800, "take": 0, "takes": ["takes/a.wav"], "take_frames": 4800},
            {"track": 0, "id": 2, "position": 2000, "bars_beats": "1.1", "offset": 0, "length": 4800, "take": 0, "takes": ["takes/b.wav"], "take_frames": 4800},
        ]}),
        "clamped at frame 0, the distance kept, in their stacking order"
    );
    assert_eq!(
        layout(&mut state)[0],
        [[3, 20_000, 0, 4800], [1, 0, 0, 4800], [2, 2000, 0, 4800]]
    );
    let swaps = item_swaps(&mut audio);
    assert_eq!(
        swaps.iter().map(|(track, _)| *track).collect::<Vec<_>>(),
        [0],
        "only the changed Track gets a new list"
    );
}

#[test]
fn a_moved_item_is_heard_over_the_one_it_lands_on() {
    let (mut state, mut audio, _dir) = state("move-top");
    guitar_and_bass(&mut state);
    drain_commands(&mut audio);
    ok(handle(&mut state, item_move(&[1], 19_000, None)));
    let swaps = item_swaps(&mut audio);
    let (_, items) = &swaps[0];
    let middle = 22_000;
    let heard: Vec<f32> = items
        .iter()
        .filter(|span| span.position <= middle && middle < span.position + span.len as u64)
        .map(|span| span.samples[0])
        .collect();
    assert_eq!(
        heard,
        [0.25],
        "only Item 1's Take sounds where it covers Item 3"
    );
}

#[test]
fn item_move_to_another_track_keeps_the_track_distance() {
    let (mut state, _audio, _dir) = state("move-track");
    open_with(
        &mut state,
        vec![
            ("Guitar", vec![item(1, 0, "takes/a.wav", 4800)]),
            ("Bass", vec![item(2, 0, "takes/b.wav", 4800)]),
            ("Keys", Vec::new()),
        ],
        &TAKES,
    );
    let moved = ok(handle(
        &mut state,
        item_move(&[1, 2], 0, Some(TrackRef::Name("Bass".to_string()))),
    ));
    let tracks: Vec<&Value> = moved["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| &item["track"])
        .collect();
    assert_eq!(tracks, [&json!(1), &json!(2)]);
    assert_eq!(
        layout(&mut state),
        [vec![], vec![[1, 0, 0, 4800]], vec![[2, 0, 0, 4800]]]
    );
    let before = layout(&mut state);
    assert_eq!(
        error(handle(
            &mut state,
            item_move(&[1, 2], 0, Some(TrackRef::Index(2)))
        )),
        "item 2 would move to a track that does not exist"
    );
    assert_eq!(
        error(handle(
            &mut state,
            item_move(&[1], 0, Some(TrackRef::Name("Drums".to_string())))
        )),
        "no track 'Drums'"
    );
    assert_eq!(layout(&mut state), before);
}

#[test]
fn item_trim_moves_the_edges_within_the_take() {
    let (mut state, _audio, _dir) = state("trim");
    guitar_and_bass(&mut state);
    let trimmed = ok(handle(&mut state, item_trim(1, Some(1200), None)));
    assert_eq!(trimmed["items"][0]["position"], 1200);
    assert_eq!(layout(&mut state)[0][0], [1, 1200, 200, 4600]);
    ok(handle(&mut state, item_trim(1, None, Some(9000))));
    assert_eq!(
        layout(&mut state)[0][0],
        [1, 1200, 200, 4600],
        "the end clamps to the Take's last frame"
    );
    ok(handle(&mut state, item_trim(1, Some(0), Some(3000))));
    assert_eq!(
        layout(&mut state)[0][0],
        [1, 1000, 0, 2000],
        "the start clamps to the Take's first frame"
    );
    let before = layout(&mut state);
    assert_eq!(
        error(handle(&mut state, item_trim(1, Some(2500), Some(2500)))),
        "item 1 would have no frames left"
    );
    assert_eq!(layout(&mut state), before);
}

#[test]
fn item_trim_clamps_an_item_on_the_last_frame_of_its_take() {
    let (mut state, _audio, _dir) = state("trim-last-frame");
    let mut last = item(1, 10_000, "takes/a.wav", 4800);
    last.offset = 4799;
    open_with(&mut state, vec![("Guitar", vec![last])], &TAKES);
    ok(handle(&mut state, item_trim(1, Some(0), Some(90_000))));
    assert_eq!(layout(&mut state), [vec![[1, 5201, 0, 4800]]]);
}

#[test]
fn item_trim_to_the_edges_it_has_leaves_history_redo_and_revision_alone() {
    let (mut state, _audio, _dir) = state("trim-unchanged");
    open_with(
        &mut state,
        vec![("Guitar", vec![item(1, 1000, "takes/a.wav", 4800)])],
        &TAKES,
    );
    ok(handle(&mut state, item_move(&[1], 10, None)));
    ok(handle(&mut state, Request::ProjectUndo));
    let revision = state.revision;
    ok(handle(&mut state, item_trim(1, None, Some(5800))));
    let status = ok(handle(&mut state, Request::Status));
    assert_eq!(
        (&status["undo"], &status["redo"]),
        (&json!(null), &json!("move Item"))
    );
    assert_eq!(state.revision, revision);
}

#[test]
fn item_split_cuts_inside_items_and_keeps_every_take_on_both_halves() {
    let (mut state, _audio, _dir) = state("split");
    let mut looped = item(1, 1000, "takes/a.wav", 4800);
    looped.takes.push(Take {
        file: "takes/b.wav".to_string(),
        frames: 4800,
    });
    looped.take = 1;
    open_with(
        &mut state,
        vec![
            ("Guitar", vec![looped, item(2, 2000, "takes/a.wav", 4800)]),
            ("Bass", vec![item(3, 0, "takes/b.wav", 4800)]),
        ],
        &TAKES,
    );
    let revision = state.revision;
    let split = ok(handle(&mut state, item_split(Some(&[1]), 1500)));
    assert!(state.revision > revision);
    assert_eq!(
        split,
        json!({"items": [
            {"track": 0, "id": 1, "position": 1000, "bars_beats": "1.1", "offset": 0, "length": 500, "take": 1, "takes": ["takes/a.wav", "takes/b.wav"], "take_frames": 4800},
            {"track": 0, "id": 4, "position": 1500, "bars_beats": "1.1", "offset": 500, "length": 4300, "take": 1, "takes": ["takes/a.wav", "takes/b.wav"], "take_frames": 4800},
        ]})
    );
    assert_eq!(
        layout(&mut state)[0],
        [[1, 1000, 0, 500], [4, 1500, 500, 4300], [2, 2000, 0, 4800]],
        "the right half stays at its Item's place in the stack"
    );
    let under = ok(handle(&mut state, item_split(None, 3000)));
    let ids: Vec<&Value> = under["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| &item["id"])
        .collect();
    assert_eq!(
        ids,
        [&json!(4), &json!(5), &json!(2), &json!(6)],
        "with no ids, the Items under the frame on the selected Track"
    );
    assert_eq!(layout(&mut state)[1], [[3, 0, 0, 4800]]);
    let ends: [(u64, &[u64]); 3] = [(1000, &[1, 2]), (1500, &[1]), (0, &[1, 2])];
    for (at, ids) in ends {
        let before = layout(&mut state);
        assert_eq!(
            error(handle(&mut state, item_split(Some(ids), at))),
            format!("nothing to split at {at}")
        );
        assert_eq!(layout(&mut state), before);
    }
}

#[test]
fn item_delete_removes_the_items() {
    let (mut state, mut audio, _dir) = state("delete");
    guitar_and_bass(&mut state);
    drain_commands(&mut audio);
    let revision = state.revision;
    assert_eq!(
        ok(handle(&mut state, item_delete(&[2, 4]))),
        json!({"deleted": [2, 4]})
    );
    assert!(state.revision > revision);
    assert_eq!(
        layout(&mut state),
        [vec![[1, 1000, 0, 4800], [3, 20_000, 0, 4800]], vec![]]
    );
    let mut tracks: Vec<usize> = item_swaps(&mut audio)
        .into_iter()
        .map(|(track, _)| track)
        .collect();
    tracks.sort_unstable();
    assert_eq!(tracks, [0, 1]);
}

#[test]
fn item_take_loads_the_take_and_clamps_the_length_to_it() {
    let (mut state, mut audio, _dir) = state("take");
    let mut looped = item(1, 0, "takes/a.wav", 4800);
    for (file, frames) in [
        ("takes/c.wav", 3000),
        ("takes/d.wav", 50),
        ("takes/e.wav", 4800),
    ] {
        looped.takes.push(Take {
            file: file.to_string(),
            frames,
        });
    }
    looped.offset = 100;
    looped.length = Some(4700);
    open_with(
        &mut state,
        vec![("Guitar", vec![looped])],
        &[("takes/a.wav", 0.25, 4800), ("takes/c.wav", 0.75, 3000)],
    );
    let loaded =
        |state: &mut EngineState| ok(handle(state, Request::Status))["loaded_audio_bytes"].clone();
    assert_eq!(loaded(&mut state), json!(4800 * 4));
    drain_commands(&mut audio);
    let switched = ok(handle(&mut state, item_take(1, 1)));
    assert_eq!(switched["items"][0]["take"], 1);
    assert_eq!(switched["items"][0]["length"], 2900);
    assert_eq!(loaded(&mut state), json!((4800 + 3000) * 4));
    let swaps = item_swaps(&mut audio);
    assert!((swaps[0].1[0].samples[0] - 0.75).abs() < 1e-6);
    assert_eq!(
        error(handle(&mut state, item_take(1, 2))),
        "item 1 starts past the end of take 2"
    );
    assert!(
        error(handle(&mut state, item_take(1, 3))).starts_with("cannot load take takes/e.wav: "),
        "a missing file"
    );
    assert_eq!(
        error(handle(&mut state, item_take(1, 4))),
        "item 1 has no take 4"
    );
    assert_eq!(layout(&mut state), [vec![[1, 0, 100, 2900]]]);
}

fn fill_commands_to(state: &mut EngineState, free: usize) {
    let filler = || {
        Command::Metronome(MetronomeSettings {
            on: false,
            bpm: 120.0,
            meter: (4, 4),
            volume_db: -12.0,
            sample_rate: 48_000.0,
        })
    };
    while state.queues.commands.slots() > free {
        assert!(state.queues.commands.push(filler()).is_ok());
    }
}

#[test]
fn a_busy_engine_leaves_the_project_and_revision_unchanged() {
    let (mut state, mut audio, _dir) = state("busy");
    guitar_and_bass(&mut state);
    drain_commands(&mut audio);
    let before = layout(&mut state);
    let loaded = ok(handle(&mut state, Request::Status))["loaded_audio_bytes"].clone();
    fill_commands_to(&mut state, 1);
    let revision = state.revision;
    assert_eq!(
        error(handle(&mut state, item_delete(&[1, 4]))),
        "engine busy",
        "two Tracks change and one slot is free"
    );
    assert_eq!(state.revision, revision);
    assert!(
        item_swaps(&mut audio).is_empty(),
        "no Track got its new list"
    );
    assert_eq!(layout(&mut state), before);
    assert_eq!(
        ok(handle(&mut state, Request::Status))["loaded_audio_bytes"],
        loaded
    );
}

fn item_duplicate(ids: &[u64]) -> Request {
    Request::ItemDuplicate { ids: ids.to_vec() }
}

fn item_copy(ids: &[u64]) -> Request {
    Request::ItemCopy { ids: ids.to_vec() }
}

fn item_paste(track: TrackRef, at: u64) -> Request {
    Request::ItemPaste { track, at }
}

/// The `status` fields `undo` and `clipboard`.
fn undo_and_clipboard(state: &mut EngineState) -> (Value, Value) {
    let status = ok(handle(state, Request::Status));
    (status["undo"].clone(), status["clipboard"].clone())
}

/// Each Item of the `items` reply as `[id, track, position]`.
fn placed(reply: &Value) -> Vec<[u64; 3]> {
    reply["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| ["id", "track", "position"].map(|key| item[key].as_u64().unwrap()))
        .collect()
}

#[test]
fn item_duplicate_copies_one_item_right_after_itself() {
    let (mut state, _audio, _dir) = state("duplicate-one");
    open_with(
        &mut state,
        vec![(
            "Guitar",
            vec![Item {
                id: 1,
                position: 1000,
                offset: 100,
                length: Some(2000),
                take: 1,
                takes: vec![
                    Take {
                        file: "takes/a.wav".to_string(),
                        frames: 4800,
                    },
                    Take {
                        file: "takes/b.wav".to_string(),
                        frames: 4800,
                    },
                ],
            }],
        )],
        &TAKES,
    );
    let revision = state.revision;
    let reply = ok(handle(&mut state, item_duplicate(&[1])));
    assert!(state.revision > revision);
    assert_eq!(placed(&reply), [[2, 0, 3000]]);
    let items = ok(handle(&mut state, Request::Items));
    let [original, copy] = [0, 1].map(|index| items["tracks"][0]["items"][index].clone());
    for key in ["offset", "length", "take", "takes"] {
        assert_eq!(copy[key], original[key], "{key}");
    }
    assert_eq!(undo_and_clipboard(&mut state).0, "duplicate Item");
}

#[test]
fn item_duplicate_places_a_group_after_its_end_keeping_tracks_and_distances() {
    let (mut state, mut audio, _dir) = state("duplicate-group");
    guitar_and_bass(&mut state);
    drain_commands(&mut audio);
    let before = layout(&mut state);
    let reply = ok(handle(&mut state, item_duplicate(&[4, 2])));
    assert_eq!(
        placed(&reply),
        [[5, 0, 10_800], [6, 1, 7800]],
        "the group runs from 0 to 7800"
    );
    let after = layout(&mut state);
    assert_eq!(
        after,
        [
            vec![
                [1, 1000, 0, 4800],
                [2, 3000, 0, 4800],
                [3, 20_000, 0, 4800],
                [5, 10_800, 0, 4800]
            ],
            vec![[4, 0, 0, 4800], [6, 7800, 0, 4800]],
        ]
    );
    assert_eq!(item_swaps(&mut audio).len(), 2);
    assert_eq!(undo_and_clipboard(&mut state).0, "duplicate 2 Items");
    ok(handle(&mut state, Request::ProjectUndo));
    assert_eq!(layout(&mut state), before);
    ok(handle(&mut state, Request::ProjectRedo));
    assert_eq!(layout(&mut state), after);
}

#[test]
fn item_copy_fills_the_clipboard_without_an_edit() {
    let (mut state, _audio, _dir) = state("copy");
    guitar_and_bass(&mut state);
    assert_eq!(undo_and_clipboard(&mut state), (Value::Null, json!(0)));
    let revision = state.revision;
    assert_eq!(
        ok(handle(&mut state, item_copy(&[2, 4, 2]))),
        json!({"copied": [2, 4]})
    );
    assert_eq!(state.revision, revision);
    assert_eq!(undo_and_clipboard(&mut state), (Value::Null, json!(2)));
}

#[test]
fn item_paste_places_the_clipboard_on_another_track_and_time_each_time() {
    let (mut state, _audio, _dir) = state("paste");
    guitar_and_bass(&mut state);
    ok(handle(&mut state, item_copy(&[4, 2])));
    let before = layout(&mut state);
    let revision = state.revision;
    let reply = ok(handle(
        &mut state,
        item_paste(TrackRef::Name("Bass".to_string()), 40_000),
    ));
    assert!(state.revision > revision);
    assert_eq!(reply["dropped"], 0);
    assert_eq!(
        placed(&reply),
        [[5, 0, 43_000], [6, 1, 40_000]],
        "Item 4, the first, lands on Bass at 40000; Item 2 keeps its 3000 frames and one Track up"
    );
    assert_eq!(
        undo_and_clipboard(&mut state),
        (json!("paste 2 Items"), json!(2))
    );
    let reply = ok(handle(&mut state, item_paste(TrackRef::Index(1), 40_000)));
    assert_eq!(placed(&reply), [[7, 0, 43_000], [8, 1, 40_000]]);
    let after = layout(&mut state);
    assert_eq!(
        after[0][3..],
        [[5, 43_000, 0, 4800], [7, 43_000, 0, 4800]],
        "on top"
    );
    assert_eq!(after[1][1..], [[6, 40_000, 0, 4800], [8, 40_000, 0, 4800]]);
    ok(handle(&mut state, Request::ProjectUndo));
    ok(handle(&mut state, Request::ProjectUndo));
    assert_eq!(layout(&mut state), before);
    ok(handle(&mut state, Request::ProjectRedo));
    assert_eq!(layout(&mut state)[1].len(), 2);
}

#[test]
fn item_paste_drops_items_whose_track_does_not_exist() {
    let (mut state, _audio, _dir) = state("paste-drop");
    guitar_and_bass(&mut state);
    ok(handle(&mut state, item_copy(&[4, 2])));
    let reply = ok(handle(&mut state, item_paste(TrackRef::Index(0), 100)));
    assert_eq!(reply["dropped"], 1);
    assert_eq!(
        placed(&reply),
        [[5, 0, 100]],
        "Item 2 would land above Track 0"
    );
    assert_eq!(undo_and_clipboard(&mut state).0, "paste Item");
    assert_eq!(
        error(handle(
            &mut state,
            item_paste(TrackRef::Name("Drums".to_string()), 0)
        )),
        "no track 'Drums'"
    );
}

#[test]
fn the_clipboard_survives_edits_and_is_cleared_on_close() {
    let (mut state, _audio, _dir) = state("clipboard-close");
    guitar_and_bass(&mut state);
    assert_eq!(
        error(handle(&mut state, item_paste(TrackRef::Index(0), 0))),
        "clipboard is empty"
    );
    ok(handle(&mut state, item_copy(&[2])));
    ok(handle(&mut state, item_delete(&[2])));
    ok(handle(&mut state, item_move(&[1], 500, None)));
    let reply = ok(handle(&mut state, item_paste(TrackRef::Index(1), 9000)));
    assert_eq!(placed(&reply), [[5, 1, 9000]]);
    ok(handle(&mut state, Request::ProjectClose));
    ok(handle(
        &mut state,
        Request::ProjectOpen {
            name: "Song".to_string(),
            restore: false,
        },
    ));
    assert_eq!(undo_and_clipboard(&mut state).1, 0);
    assert_eq!(
        error(handle(&mut state, item_paste(TrackRef::Index(0), 0))),
        "clipboard is empty"
    );
}

#[test]
fn item_paste_fails_when_a_take_file_of_the_clipboard_is_gone() {
    let (mut state, _audio, dir) = state("paste-gone");
    guitar_and_bass(&mut state);
    ok(handle(&mut state, item_copy(&[1])));
    fs::remove_file(dir.0.join("projects/song/takes/a.wav")).unwrap();
    let before = layout(&mut state);
    assert_eq!(
        error(handle(&mut state, item_paste(TrackRef::Index(0), 0))),
        "cannot paste: take takes/a.wav no longer exists"
    );
    assert_eq!(layout(&mut state), before);
}

#[test]
fn duplicate_copy_and_paste_name_their_errors() {
    let (mut state, _audio, _dir) = state("clipboard-errors");
    for request in [
        item_duplicate(&[1]),
        item_copy(&[1]),
        item_paste(TrackRef::Index(0), 0),
    ] {
        assert_eq!(error(handle(&mut state, request)), "no project open");
    }
    guitar_and_bass(&mut state);
    for request in [item_duplicate(&[1, 9]), item_copy(&[9])] {
        assert_eq!(error(handle(&mut state, request)), "no such item: 9");
    }
    for request in [item_duplicate(&[]), item_copy(&[])] {
        assert_eq!(error(handle(&mut state, request)), "give at least one item");
    }
}

#[test]
fn duplicate_and_paste_are_refused_while_recording_and_copy_is_not() {
    let (mut state, _audio, _dir) = state("clipboard-recording");
    guitar_and_bass(&mut state);
    ok(handle(
        &mut state,
        Request::TrackSet {
            track: TrackRef::Index(1),
            input: None,
            volume: None,
            pan: None,
            mute: None,
            solo: None,
            arm: Some(true),
            name: None,
            to_master: None,
        },
    ));
    ok(handle(
        &mut state,
        Request::Transport {
            action: TransportAction::Record,
            position: None,
        },
    ));
    let before = layout(&mut state);
    ok(handle(&mut state, item_copy(&[1])));
    for request in [item_duplicate(&[1]), item_paste(TrackRef::Index(0), 0)] {
        assert_eq!(
            error(handle(&mut state, request)),
            "cannot edit items while recording"
        );
    }
    assert_eq!(layout(&mut state), before);
}

const LAST_FRAME: u64 = i64::MAX as u64;

#[test]
fn duplicate_and_paste_past_the_last_frame_change_nothing() {
    let (mut state, mut audio, _dir) = state("past-last-frame");
    open_with(
        &mut state,
        vec![(
            "Guitar",
            vec![item(1, LAST_FRAME - 4800, "takes/a.wav", 4800)],
        )],
        &TAKES,
    );
    ok(handle(&mut state, item_copy(&[1])));
    ok(handle(&mut state, item_move(&[1], -10, None)));
    drain_commands(&mut audio);
    let before = layout(&mut state);
    let revision = state.revision;
    let history = undo_and_clipboard(&mut state);
    for request in [
        item_duplicate(&[1]),
        item_paste(TrackRef::Index(0), LAST_FRAME - 4799),
    ] {
        assert_eq!(
            error(handle(&mut state, request)),
            "track 'Guitar' item 1 ends past the last frame"
        );
        assert_eq!(state.revision, revision);
        assert_eq!(layout(&mut state), before);
        assert_eq!(undo_and_clipboard(&mut state), history);
        assert!(item_swaps(&mut audio).is_empty(), "no Track got a new list");
    }
    ok(handle(&mut state, Request::ProjectUndo));
    assert_eq!(layout(&mut state), [vec![[1, LAST_FRAME - 4800, 0, 4800]]]);
}
