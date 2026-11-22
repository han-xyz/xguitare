use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use xguitare_core::Preset;
use xguitare_core::project::{
    AUTOSAVE_FILE, Item, Master, PROJECT_FILE, Project, Send, Take, Track, autosave_due,
    autosave_newer,
};
use xguitare_protocol::LoopRegion;

const TWO_TRACKS: &str = r#"name = "Song"
sample_rate = 48000
tempo_bpm = 96.5
meter = [7, 8]
latency_offset_ms = 2.5

[loop_region]
start = 1000
end = 96000
on = true

[[tracks]]
name = "Guitar"
input = "Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO"
volume_db = -3.0
pan = -0.5
mute = false
solo = true
arm = true

[[tracks.module]]
type = "drive"
id = "drv"
bypass = true
params = { drive = 0.7 }

[[tracks.module]]
type = "cabinet"
id = "cab"
ir = "irs/v30.wav"

[[tracks.items]]
position = 48000
take = 1

[[tracks.items.takes]]
file = "takes/guitar-1.wav"
frames = 96000

[[tracks.items.takes]]
file = "takes/guitar-2.wav"
frames = 95000

[[tracks]]
name = "Bass"
volume_db = 0.0
pan = 0.0
mute = true
solo = false
arm = false
"#;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-project-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn load_error(dir: &Path, text: &str) -> String {
    fs::write(dir.join(PROJECT_FILE), text).unwrap();
    Project::load(dir).unwrap_err().to_string()
}

fn with_take_file(file: &str) -> String {
    TWO_TRACKS.replace("takes/guitar-2.wav", file)
}

fn set_mtime(path: &Path, time: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[test]
fn round_trip_keeps_tracks_items_takes_and_racks() {
    let dir = temp_dir("round-trip");
    fs::write(dir.0.join(PROJECT_FILE), TWO_TRACKS).unwrap();
    let project = Project::load(&dir.0).unwrap().0;

    assert_eq!(project.name, "Song");
    assert_eq!(project.sample_rate, 48_000);
    assert_eq!(project.tempo_bpm, 96.5);
    assert_eq!(project.meter, (7, 8));
    assert_eq!(
        project.loop_region,
        LoopRegion {
            start: 1000,
            end: 96_000,
            on: true
        }
    );
    assert_eq!(project.latency_offset_ms, 2.5);
    assert_eq!(project.tracks.len(), 2);
    let guitar = &project.tracks[0];
    assert_eq!(
        guitar.input.as_deref(),
        Some("Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO")
    );
    assert_eq!((guitar.volume_db, guitar.pan), (-3.0, -0.5));
    assert_eq!((guitar.mute, guitar.solo, guitar.arm), (false, true, true));
    assert!(guitar.modules[0].bypass);
    assert_eq!(guitar.modules[1].ir.as_deref(), Some("irs/v30.wav"));
    assert_eq!(
        guitar.items,
        vec![Item {
            id: 1,
            position: 48_000,
            offset: 0,
            length: None,
            take: 1,
            takes: vec![
                Take {
                    file: "takes/guitar-1.wav".to_string(),
                    frames: 96_000
                },
                Take {
                    file: "takes/guitar-2.wav".to_string(),
                    frames: 95_000
                },
            ],
        }]
    );
    let bass = &project.tracks[1];
    assert_eq!(bass.input, None);
    assert!(bass.mute);
    assert!(bass.modules.is_empty() && bass.items.is_empty());

    let copy = temp_dir("round-trip-copy");
    project.save(&copy.0).unwrap();
    assert_eq!(Project::load(&copy.0).unwrap().0, project);
}

#[test]
fn save_leaves_no_temp_file() {
    let dir = temp_dir("save-clean");
    fs::write(dir.0.join(PROJECT_FILE), TWO_TRACKS).unwrap();
    let project = Project::load(&dir.0).unwrap().0;
    project.save(&dir.0).unwrap();
    let names: Vec<_> = fs::read_dir(&dir.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![PROJECT_FILE]);
}

#[test]
fn save_gives_the_default_mode() {
    let dir = temp_dir("save-mode");
    fs::write(dir.0.join(PROJECT_FILE), TWO_TRACKS).unwrap();
    let project = Project::load(&dir.0).unwrap().0;
    fs::remove_file(dir.0.join(PROJECT_FILE)).unwrap();
    project.save(&dir.0).unwrap();
    fs::write(dir.0.join("plain"), "").unwrap();
    assert_eq!(mode(&dir.0.join(PROJECT_FILE)), mode(&dir.0.join("plain")));
}

#[test]
fn a_failed_save_names_the_project_file() {
    let dir = temp_dir("save-error");
    fs::write(dir.0.join(PROJECT_FILE), TWO_TRACKS).unwrap();
    let project = Project::load(&dir.0).unwrap().0;
    let blocked = temp_dir("save-blocked");
    fs::create_dir_all(blocked.0.join(PROJECT_FILE).join("inside")).unwrap();
    let message = project.save(&blocked.0).unwrap_err().to_string();
    let prefix = format!("{}: ", blocked.0.join(PROJECT_FILE).display());
    assert!(message.starts_with(&prefix), "{message}");
}

#[test]
fn new_creates_slugged_folder_with_takes() {
    let dir = temp_dir("new");
    let (folder, project) = Project::new(&dir.0, "  My First Song! ").unwrap();
    assert_eq!(folder, dir.0.join("my-first-song"));
    assert_eq!(project.name, "My First Song!");
    assert_eq!(project.tracks, []);
    assert!(folder.join("takes").is_dir());
    assert_eq!(Project::load(&folder).unwrap().0, project);
}

#[test]
fn new_names_cannot_escape_projects_dir() {
    let dir = temp_dir("escape");
    let projects = dir.0.join("projects");
    for name in ["../x", "/abs", "a/../../b", ".."] {
        match Project::new(&projects, name) {
            Ok((folder, _)) => assert_eq!(folder.parent(), Some(projects.as_path()), "{name}"),
            Err(error) => assert_eq!(error.to_string(), "project name must not be empty"),
        }
    }
    let entries: Vec<_> = fs::read_dir(&dir.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec!["projects"]);
    assert!(projects.join("x").is_dir() && projects.join("abs").is_dir());
}

#[test]
fn new_empty_name_is_error() {
    let dir = temp_dir("new-empty");
    assert_eq!(
        Project::new(&dir.0, " -- ").unwrap_err().to_string(),
        "project name must not be empty"
    );
}

#[test]
fn new_duplicate_name_is_error() {
    let dir = temp_dir("duplicate");
    Project::new(&dir.0, "Song").unwrap();
    assert_eq!(
        Project::new(&dir.0, "song").unwrap_err().to_string(),
        "a project named 'song' already exists"
    );
}

#[test]
fn list_finds_project_folders_only() {
    let dir = temp_dir("list");
    assert_eq!(
        Project::list(&dir.0.join("missing")).unwrap(),
        Vec::<PathBuf>::new()
    );
    Project::new(&dir.0, "B side").unwrap();
    Project::new(&dir.0, "A side").unwrap();
    fs::create_dir(dir.0.join("not-a-project")).unwrap();
    assert_eq!(
        Project::list(&dir.0).unwrap(),
        vec![dir.0.join("a-side"), dir.0.join("b-side")]
    );
}

#[test]
fn seventeenth_track_is_error() {
    let dir = temp_dir("seventeen");
    let (_, mut project) = Project::new(&dir.0, "Big").unwrap();
    let preset = Preset::parse("name = \"Clean\"\n[[module]]\ntype = \"gain\"\n").unwrap();
    for n in 0..16 {
        project
            .add_track(Track::from_preset(&format!("T{n}"), &preset))
            .unwrap();
    }
    assert_eq!(
        project
            .add_track(Track::from_preset("T16", &preset))
            .unwrap_err()
            .to_string(),
        "a project has at most 16 tracks"
    );
    assert_eq!(project.tracks.len(), 16);

    let seventeen = TWO_TRACKS.to_string()
        + &"[[tracks]]\nname = \"X\"\nvolume_db = 0.0\npan = 0.0\n".repeat(15);
    assert_eq!(
        load_error(&dir.0, &seventeen),
        "a project has at most 16 tracks"
    );
}

#[test]
fn track_from_preset_copies_modules() {
    let preset = Preset::parse(
        "name = \"Lead\"\n[[module]]\ntype = \"drive\"\nbypass = true\n[[module]]\ntype = \"cabinet\"\nir = \"v30.wav\"\n",
    )
    .unwrap();
    let track = Track::from_preset("Lead gtr", &preset);
    assert_eq!(track.name, "Lead gtr");
    assert_eq!(track.modules, preset.modules);
    assert_eq!(track.input, None);
    assert_eq!((track.volume_db, track.pan), (0.0, 0.0));
    assert!(!track.mute && !track.solo && !track.arm);
    assert_eq!(track.items, []);
}

#[test]
fn take_paths_other_than_takes_name_wav_are_rejected() {
    let dir = temp_dir("take-paths");
    for file in [
        "../x.wav",
        "/abs/x.wav",
        "takes/../../x.wav",
        "",
        "takes//x.wav",
        "project.toml",
        "x.wav",
        "takes/a/b.wav",
        "takes/x.mp3",
        "takes/.wav",
        "takes/../x.wav",
    ] {
        assert_eq!(
            load_error(&dir.0, &with_take_file(file)),
            format!("take file '{file}' must be takes/<name>.wav"),
        );
    }
}

#[test]
fn track_names_follow_the_rules_of_track_add() {
    let dir = temp_dir("track-names");
    let with_name =
        |name: &str| TWO_TRACKS.replace(r#"name = "Bass""#, &format!(r#"name = "{name}""#));
    for (name, error) in [
        ("GUITAR", "a track named 'GUITAR' already exists"),
        ("42", "track name must not be only digits"),
        (r" \u0007 ", "track name must not be empty"),
    ] {
        assert_eq!(load_error(&dir.0, &with_name(name)), error);
    }
    fs::write(dir.0.join(PROJECT_FILE), with_name(r" Ba\u001bss ")).unwrap();
    assert_eq!(Project::load(&dir.0).unwrap().0.tracks[1].name, "Bass");
}

#[test]
fn track_module_errors_name_the_track() {
    let dir = temp_dir("module-error");
    let text = TWO_TRACKS.replace("type = \"drive\"", "type = \"wah\"");
    assert_eq!(
        load_error(&dir.0, &text),
        "track 'Guitar': unknown module type 'wah'"
    );
    let text = TWO_TRACKS.replace("ir = \"irs/v30.wav\"", "ir = \"../v30.wav\"");
    assert_eq!(
        load_error(&dir.0, &text),
        "track 'Guitar': cabinet 'cab' ir must be a relative name inside the IR directory or the Bank directory"
    );
}

#[test]
fn active_take_out_of_range_is_error() {
    let dir = temp_dir("take-index");
    let text = TWO_TRACKS.replace("take = 1", "take = 2");
    assert_eq!(
        load_error(&dir.0, &text),
        "track 'Guitar' item 0 has no take 2"
    );
}

#[test]
fn syntax_error_names_file_and_position() {
    let dir = temp_dir("syntax");
    let message = load_error(&dir.0, "name = \"Song\"\n[loop_region\n");
    assert!(
        message.starts_with(&format!("{}: line 2:", dir.0.join(PROJECT_FILE).display())),
        "{message}"
    );
}

#[test]
fn a_saved_project_starts_with_format_1() {
    let dir = temp_dir("format-saved");
    let (folder, project) = Project::new(&dir.0, "Song").unwrap();
    project.save_autosave(&folder).unwrap();
    for file in [PROJECT_FILE, AUTOSAVE_FILE] {
        let text = fs::read_to_string(folder.join(file)).unwrap();
        assert_eq!(text.lines().next(), Some("format = 1"), "{file}");
    }
}

#[test]
fn a_project_without_a_format_key_loads() {
    let dir = temp_dir("format-missing");
    fs::write(dir.0.join(PROJECT_FILE), TWO_TRACKS).unwrap();
    let without = Project::load(&dir.0).unwrap();
    fs::write(
        dir.0.join(PROJECT_FILE),
        format!("format = 1\n{TWO_TRACKS}"),
    )
    .unwrap();
    assert_eq!(Project::load(&dir.0).unwrap(), without);
}

#[test]
fn a_project_from_a_newer_format_is_refused() {
    let dir = temp_dir("format-newer");
    assert_eq!(
        load_error(&dir.0, &format!("format = 2\n{TWO_TRACKS}")),
        "project format 2 is newer than this xguitare (reads up to 1)"
    );
}

#[test]
fn a_newer_format_with_unknown_keys_is_refused_by_its_format() {
    let dir = temp_dir("format-newer-keys");
    let text = format!("format = 3\nfuture = true\n{TWO_TRACKS}")
        .replace("pan = 0.0", "pan = 0.0\nwidth = 1.0");
    assert_eq!(
        load_error(&dir.0, &text),
        "project format 3 is newer than this xguitare (reads up to 1)"
    );
}

#[test]
fn an_autosave_from_a_newer_format_is_refused() {
    let dir = temp_dir("format-newer-autosave");
    fs::write(
        dir.0.join(AUTOSAVE_FILE),
        format!("format = 2\n{TWO_TRACKS}"),
    )
    .unwrap();
    assert_eq!(
        Project::load_autosave(&dir.0).unwrap_err().to_string(),
        "project format 2 is newer than this xguitare (reads up to 1)"
    );
}

#[test]
fn a_format_of_zero_is_refused() {
    let dir = temp_dir("format-zero");
    for format in ["0", "-1", "1.5", "\"1\""] {
        assert_eq!(
            load_error(&dir.0, &format!("format = {format}\n{TWO_TRACKS}")),
            "project format must be a whole number of 1 or more",
            "{format}"
        );
    }
}

#[test]
fn autosave_due_after_sixty_seconds_while_dirty() {
    let last_save = Instant::now();
    let later = |secs| last_save + Duration::from_secs(secs);
    assert!(!autosave_due(last_save, later(59), true));
    assert!(autosave_due(last_save, later(60), true));
    assert!(autosave_due(last_save, later(600), true));
    assert!(!autosave_due(last_save, later(600), false));
}

#[test]
fn autosave_round_trips_beside_project_file() {
    let dir = temp_dir("autosave");
    let (folder, mut project) = Project::new(&dir.0, "Song").unwrap();
    project.tempo_bpm = 140.0;
    project.save_autosave(&folder).unwrap();
    assert!(folder.join(AUTOSAVE_FILE).is_file());
    assert_eq!(Project::load_autosave(&folder).unwrap().0, project);
    assert_eq!(Project::load(&folder).unwrap().0.tempo_bpm, 120.0);
}

#[test]
fn autosave_newer_compares_mtimes() {
    let dir = temp_dir("autosave-newer");
    let (folder, project) = Project::new(&dir.0, "Song").unwrap();
    assert!(!autosave_newer(&folder));
    project.save_autosave(&folder).unwrap();
    let now = SystemTime::now();
    set_mtime(&folder.join(PROJECT_FILE), now - Duration::from_secs(120));
    set_mtime(&folder.join(AUTOSAVE_FILE), now - Duration::from_secs(60));
    assert!(autosave_newer(&folder));
    set_mtime(&folder.join(PROJECT_FILE), now);
    assert!(!autosave_newer(&folder));
    fs::remove_file(folder.join(PROJECT_FILE)).unwrap();
    assert!(autosave_newer(&folder));
}

fn assert_load_errors(test: &str, find: &str, replacements: &[&str], message: &str) {
    let dir = temp_dir(test);
    assert!(TWO_TRACKS.contains(find), "{find}");
    for replacement in replacements {
        assert_eq!(
            load_error(&dir.0, &TWO_TRACKS.replacen(find, replacement, 1)),
            message,
            "{replacement}"
        );
    }
}

#[test]
fn tempo_out_of_range_is_rejected() {
    assert_load_errors(
        "tempo",
        "tempo_bpm = 96.5",
        &[
            "tempo_bpm = 19.9",
            "tempo_bpm = 300.1",
            "tempo_bpm = nan",
            "tempo_bpm = inf",
        ],
        "tempo must be between 20 and 300 bpm",
    );
}

#[test]
fn meter_out_of_range_is_rejected() {
    assert_load_errors(
        "meter",
        "meter = [7, 8]",
        &[
            "meter = [0, 4]",
            "meter = [17, 4]",
            "meter = [4, 3]",
            "meter = [4, 0]",
            "meter = [4, 32]",
        ],
        "meter must be 1 to 16 beats of a 2, 4, 8 or 16 note",
    );
}

#[test]
fn zero_sample_rate_is_rejected() {
    assert_load_errors(
        "sample-rate",
        "sample_rate = 48000",
        &["sample_rate = 0"],
        "sample rate must be above 0",
    );
}

#[test]
fn latency_offset_out_of_range_is_rejected() {
    assert_load_errors(
        "latency",
        "latency_offset_ms = 2.5",
        &[
            "latency_offset_ms = 1000.5",
            "latency_offset_ms = -1000.5",
            "latency_offset_ms = nan",
        ],
        "latency offset must be between -1000 and 1000 ms",
    );
}

#[test]
fn loop_on_needs_start_before_end() {
    assert_load_errors(
        "loop",
        "start = 1000\nend = 96000",
        &["start = 96000\nend = 96000", "start = 96001\nend = 96000"],
        "loop start must be before loop end",
    );
    let dir = temp_dir("loop-off");
    let off = TWO_TRACKS.replace(
        "start = 1000\nend = 96000\non = true",
        "start = 5\nend = 0\non = false",
    );
    fs::write(dir.0.join(PROJECT_FILE), off).unwrap();
    assert!(!Project::load(&dir.0).unwrap().0.loop_region.on);
}

#[test]
fn track_volume_out_of_range_is_rejected() {
    assert_load_errors(
        "volume",
        "volume_db = -3.0",
        &["volume_db = -60.5", "volume_db = 6.5", "volume_db = nan"],
        "track 'Guitar' volume must be between -60 and 6 dB",
    );
}

#[test]
fn track_pan_out_of_range_is_rejected() {
    assert_load_errors(
        "pan",
        "pan = -0.5",
        &["pan = -1.5", "pan = 1.01", "pan = nan"],
        "track 'Guitar' pan must be between -1 and 1",
    );
}

#[test]
fn item_ending_past_the_last_frame_is_rejected() {
    assert_load_errors(
        "item-end",
        "position = 48000",
        &["position = 9223372036854775807"],
        "track 'Guitar' item 0 ends past the last frame",
    );
}

#[test]
fn save_refuses_what_load_would_reject() {
    let dir = temp_dir("save-checks");
    let (folder, project) = Project::new(&dir.0, "Song").unwrap();
    let saved = fs::read_to_string(folder.join(PROJECT_FILE)).unwrap();

    let mut slow = project.clone();
    slow.tempo_bpm = 0.0;
    assert_eq!(
        slow.save(&folder).unwrap_err().to_string(),
        "tempo must be between 20 and 300 bpm"
    );
    let mut endless = project.clone();
    endless.loop_region.end = u64::MAX;
    assert_eq!(
        endless.save(&folder).unwrap_err().to_string(),
        "loop region ends past the last frame"
    );
    let mut stray = project;
    let mut track = Track::from_preset("Gtr", &Preset::parse("name = \"P\"\n").unwrap());
    track.items.push(Item {
        id: 1,
        position: 0,
        offset: 0,
        length: None,
        take: 0,
        takes: vec![Take {
            file: "project.toml".to_string(),
            frames: 1,
        }],
    });
    stray.tracks.push(track);
    stray.next_item_id = 2;
    assert_eq!(
        stray.save(&folder).unwrap_err().to_string(),
        "take file 'project.toml' must be takes/<name>.wav"
    );
    assert_eq!(
        fs::read_to_string(folder.join(PROJECT_FILE)).unwrap(),
        saved
    );
}

#[test]
fn add_track_checks_the_track() {
    let dir = temp_dir("add-track");
    let (_, mut project) = Project::new(&dir.0, "Song").unwrap();
    let mut track = Track::from_preset("Loud", &Preset::parse("name = \"P\"\n").unwrap());
    track.volume_db = 12.0;
    assert_eq!(
        project.add_track(track).unwrap_err().to_string(),
        "track 'Loud' volume must be between -60 and 6 dB"
    );
    assert_eq!(project.tracks, []);
}

#[test]
fn item_offset_round_trips_and_is_range_checked() {
    let dir = temp_dir("item-offset");
    let (folder, mut project) = Project::new(&dir.0, "Song").unwrap();
    let mut track = Track::from_preset("Gtr", &Preset::parse("name = \"P\"\n").unwrap());
    track.items.push(Item {
        id: 1,
        position: 0,
        offset: 256,
        length: None,
        take: 0,
        takes: vec![Take {
            file: "takes/gtr-1.wav".to_string(),
            frames: 1000,
        }],
    });
    project.tracks.push(track);
    project.next_item_id = 2;
    project.save(&folder).unwrap();
    assert!(
        fs::read_to_string(folder.join(PROJECT_FILE))
            .unwrap()
            .contains("offset = 256")
    );
    assert_eq!(Project::load(&folder).unwrap().0, project);
    project.tracks[0].items[0].offset = u64::MAX;
    assert_eq!(
        project.save(&folder).unwrap_err().to_string(),
        "track 'Gtr' item 0 offset is past the last frame"
    );
}

#[test]
fn click_defaults_off_and_count_in_is_range_checked() {
    let dir = temp_dir("click");
    fs::write(dir.0.join(PROJECT_FILE), TWO_TRACKS).unwrap();
    let project = Project::load(&dir.0).unwrap().0;
    assert_eq!((project.click, project.count_in), (false, 0));
    assert_load_errors(
        "count-in",
        "tempo_bpm = 96.5",
        &["tempo_bpm = 96.5\ncount_in = 3"],
        "count-in must be 0, 1 or 2 bars",
    );
}

const MIXER: &str = r#"name = "Mix"
sample_rate = 48000
tempo_bpm = 120.0
meter = [4, 4]
latency_offset_ms = 0.0

[loop_region]
start = 0
end = 0
on = false

[master]
volume_db = -3.0

[[master.module]]
type = "compressor"
id = "glue"

[[master.module]]
type = "gain"
id = "trim"
params = { level = -1.5 }

[[buses]]
id = 3
name = "Verb"
volume_db = -6.0
pan = 0.25
mute = false
solo = true

[[buses.module]]
type = "reverb"
id = "room"

[[buses.module]]
type = "gate"
id = "tail"

[[buses]]
id = 7
name = "Echo"
volume_db = 0.0
pan = 0.0
mute = true
solo = false

[[tracks]]
name = "Guitar"
volume_db = 0.0
pan = 0.0
to_master = false

[[tracks.sends]]
bus = 3
level_db = -12.0
pre = false

[[tracks.sends]]
bus = 7
level_db = 6.0
pre = true

[[tracks]]
name = "Bass"
volume_db = 0.0
pan = 0.0
"#;

#[test]
fn round_trip_keeps_buses_sends_and_master() {
    let dir = temp_dir("mixer-round-trip");
    fs::write(dir.0.join(PROJECT_FILE), MIXER).unwrap();
    let project = Project::load(&dir.0).unwrap().0;
    assert_eq!(project.master.volume_db, -3.0);
    let master: Vec<&str> = project
        .master
        .modules
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(master, ["glue", "trim"]);
    assert_eq!(project.buses.len(), 2);
    let verb = &project.buses[0];
    assert_eq!((verb.id, verb.name.as_str()), (3, "Verb"));
    assert_eq!(
        (verb.volume_db, verb.pan, verb.mute, verb.solo),
        (-6.0, 0.25, false, true)
    );
    let rack: Vec<&str> = verb.modules.iter().map(|m| m.type_name.as_str()).collect();
    assert_eq!(rack, ["reverb", "gate"]);
    assert!(project.buses[1].mute);
    let guitar = &project.tracks[0];
    assert!(!guitar.to_master);
    assert_eq!(
        guitar.sends,
        [
            Send {
                bus: 3,
                level_db: -12.0,
                pre: false
            },
            Send {
                bus: 7,
                level_db: 6.0,
                pre: true
            },
        ]
    );
    assert!(project.tracks[1].to_master && project.tracks[1].sends.is_empty());

    let copy = temp_dir("mixer-round-trip-copy");
    project.save(&copy.0).unwrap();
    assert_eq!(Project::load(&copy.0).unwrap().0, project);
}

#[test]
fn a_file_without_a_mixer_loads_with_no_buses_and_master_at_zero_db() {
    let dir = temp_dir("mixer-old-file");
    fs::write(dir.0.join(PROJECT_FILE), TWO_TRACKS).unwrap();
    let project = Project::load(&dir.0).unwrap().0;
    assert_eq!(project.buses, []);
    assert_eq!(project.master, Master::default());
    assert_eq!(project.master.volume_db, 0.0);
    assert_eq!(project.master.modules, []);
    assert!(
        project
            .tracks
            .iter()
            .all(|track| track.to_master && track.sends.is_empty())
    );
}

#[test]
fn only_sends_that_are_not_off_are_saved() {
    let dir = temp_dir("mixer-off-sends");
    fs::write(dir.0.join(PROJECT_FILE), MIXER).unwrap();
    let mut project = Project::load(&dir.0).unwrap().0;
    project.tracks[0].sends[1].level_db = f32::NEG_INFINITY;
    project.master.volume_db = f32::NEG_INFINITY;
    project.save(&dir.0).unwrap();
    let text = fs::read_to_string(dir.0.join(PROJECT_FILE)).unwrap();
    assert_eq!(text.matches("[[tracks.sends]]").count(), 1, "{text}");
    let loaded = Project::load(&dir.0).unwrap().0;
    assert_eq!(loaded.tracks[0].sends, project.tracks[0].sends[..1]);
    assert_eq!(loaded.master.volume_db, f32::NEG_INFINITY);
}

fn assert_mixer_errors(test: &str, find: &str, replacements: &[&str], message: &str) {
    let dir = temp_dir(test);
    assert!(MIXER.contains(find), "{find}");
    for replacement in replacements {
        assert_eq!(
            load_error(&dir.0, &MIXER.replacen(find, replacement, 1)),
            message,
            "{replacement}"
        );
    }
}

#[test]
fn ninth_bus_is_error() {
    let extra = (10..17)
        .map(|id| format!("[[buses]]\nid = {id}\nname = \"B{id}\"\nvolume_db = 0.0\npan = 0.0\n"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_mixer_errors(
        "mixer-nine-buses",
        "[[tracks]]\nname = \"Guitar\"",
        &[&format!("{extra}\n[[tracks]]\nname = \"Guitar\"")],
        "at most 8 buses",
    );
}

#[test]
fn a_send_to_an_unknown_bus_is_error() {
    assert_mixer_errors(
        "mixer-unknown-bus",
        "bus = 7",
        &["bus = 8"],
        "track 'Guitar' sends to unknown bus 8",
    );
}

#[test]
fn two_sends_to_one_bus_is_error() {
    assert_mixer_errors(
        "mixer-duplicate-send",
        "bus = 7",
        &["bus = 3"],
        "track 'Guitar' has two sends to bus 3",
    );
}

#[test]
fn send_level_out_of_range_is_error() {
    assert_mixer_errors(
        "mixer-send-level",
        "level_db = -12.0",
        &[
            "level_db = 6.5",
            "level_db = -61.0",
            "level_db = nan",
            "level_db = inf",
        ],
        "track 'Guitar' send level must be -inf or between -60 and 6 dB",
    );
}

#[test]
fn bus_volume_and_pan_out_of_range_are_errors() {
    assert_mixer_errors(
        "mixer-bus-volume",
        "volume_db = -6.0",
        &["volume_db = 7.0", "volume_db = -inf", "volume_db = nan"],
        "bus 'Verb' volume must be between -60 and 6 dB",
    );
    assert_mixer_errors(
        "mixer-bus-pan",
        "pan = 0.25",
        &["pan = 1.5", "pan = nan"],
        "bus 'Verb' pan must be between -1 and 1",
    );
}

#[test]
fn master_volume_out_of_range_is_error() {
    assert_mixer_errors(
        "mixer-master-volume",
        "volume_db = -3.0",
        &["volume_db = 6.5", "volume_db = -61.0", "volume_db = nan"],
        "master volume must be -inf or between -60 and 6 dB",
    );
}

#[test]
fn a_cabinet_on_a_bus_or_master_is_error() {
    let cabinet = "type = \"cabinet\"\nid = \"cab\"";
    assert_mixer_errors(
        "mixer-bus-cabinet",
        "type = \"gate\"\nid = \"tail\"",
        &[cabinet],
        "a cabinet is not allowed on a Bus or Master",
    );
    assert_mixer_errors(
        "mixer-master-cabinet",
        "type = \"compressor\"\nid = \"glue\"",
        &[cabinet],
        "a cabinet is not allowed on a Bus or Master",
    );
}

#[test]
fn bus_and_master_module_errors_name_the_strip() {
    assert_mixer_errors(
        "mixer-bus-module",
        "type = \"reverb\"",
        &["type = \"wah\""],
        "bus 'Verb': unknown module type 'wah'",
    );
    assert_mixer_errors(
        "mixer-master-module",
        "type = \"compressor\"",
        &["type = \"wah\""],
        "master: unknown module type 'wah'",
    );
}

#[test]
fn bus_names_and_ids_follow_the_rules() {
    for (name, error) in [
        ("verb", "a bus named 'verb' already exists"),
        ("12", "bus name must not be only digits"),
        (" ", "bus name must not be empty"),
    ] {
        assert_mixer_errors(
            "mixer-bus-names",
            "name = \"Echo\"",
            &[&format!("name = \"{name}\"")],
            error,
        );
    }
    assert_mixer_errors(
        "mixer-bus-ids",
        "id = 7",
        &["id = 3"],
        "two buses have id 3",
    );
}

fn item(id: u64, offset: u64, length: Option<u64>, frames: u64) -> Item {
    Item {
        id,
        position: 0,
        offset,
        length,
        take: 0,
        takes: vec![Take {
            file: format!("takes/gtr-{id}.wav"),
            frames,
        }],
    }
}

fn project_with_items(dir: &Path, items: Vec<Item>) -> (PathBuf, Project) {
    let (folder, mut project) = Project::new(dir, "Song").unwrap();
    let mut track = Track::from_preset("Gtr", &Preset::parse("name = \"P\"\n").unwrap());
    project.next_item_id = items.iter().map(|item| item.id + 1).max().unwrap_or(1);
    track.items = items;
    project.tracks.push(track);
    (folder, project)
}

#[test]
fn item_ids_and_length_round_trip() {
    let dir = temp_dir("item-ids");
    let (folder, mut project) = project_with_items(
        &dir.0,
        vec![item(7, 10, Some(500), 1000), item(3, 0, None, 1000)],
    );
    project.next_item_id = 12;
    project.save(&folder).unwrap();
    let text = fs::read_to_string(folder.join(PROJECT_FILE)).unwrap();
    assert!(text.contains("next_item_id = 12"), "{text}");
    assert!(text.contains("id = 7") && text.contains("id = 3"), "{text}");
    assert_eq!(text.matches("length = ").count(), 1, "{text}");
    assert!(text.contains("length = 500"), "{text}");
    assert_eq!(Project::load(&folder).unwrap().0, project);
}

fn item_ids(project: &Project) -> Vec<u64> {
    project.tracks[0].items.iter().map(|item| item.id).collect()
}

#[test]
fn an_item_starting_at_or_past_the_end_of_its_take_is_dropped_on_load_with_a_warning() {
    let dir = temp_dir("offset-past-take");
    let mut short = item(7, 999, None, 1000);
    short.takes.push(Take {
        file: "takes/gtr-8.wav".to_string(),
        frames: 500,
    });
    let (folder, project) = project_with_items(&dir.0, vec![item(1, 0, None, 1000), short]);
    project.save(&folder).unwrap();
    let text = fs::read_to_string(folder.join(PROJECT_FILE)).unwrap();
    for offset in ["1000", "10000"] {
        for file in [PROJECT_FILE, AUTOSAVE_FILE] {
            let past = text.replace("offset = 999", &format!("offset = {offset}"));
            fs::write(folder.join(file), past).unwrap();
            let loaded = if file == AUTOSAVE_FILE {
                Project::load_autosave(&folder)
            } else {
                Project::load(&folder)
            };
            let (loaded, warnings) = loaded.unwrap();
            assert_eq!(item_ids(&loaded), [1], "{file}, offset {offset}");
            let warnings: Vec<String> = warnings.iter().map(ToString::to_string).collect();
            assert_eq!(
                warnings,
                [
                    "track 'Gtr': dropped item 7 (takes/gtr-7.wav, takes/gtr-8.wav): starts past the end of its take"
                ],
                "{file}, offset {offset}"
            );
        }
    }
    let (loaded, _) = Project::load(&folder).unwrap();
    loaded.save(&folder).unwrap();
    let (saved, warnings) = Project::load(&folder).unwrap();
    assert_eq!((item_ids(&saved), warnings.len()), (vec![1], 0));
}

fn loop_item(offset: u64, length: Option<u64>, short_frames: u64) -> Item {
    let take = |file: &str, frames| Take {
        file: file.to_string(),
        frames,
    };
    Item {
        id: 1,
        position: 0,
        offset,
        length,
        take: 1,
        takes: vec![
            take("takes/gtr-1.wav", 4800),
            take("takes/gtr-2.wav", 4800),
            take("takes/gtr-3.wav", short_frames),
        ],
    }
}

#[test]
fn an_item_whose_active_take_is_short_plays_its_newest_take_that_fits_on_load() {
    let dir = temp_dir("short-active-take");
    for (case, (offset, length, short_frames)) in [(200, None, 100), (200, Some(4000), 1000)]
        .into_iter()
        .enumerate()
    {
        let (folder, project) = project_with_items(
            &dir.0.join(case.to_string()),
            vec![loop_item(offset, length, short_frames)],
        );
        project.save(&folder).unwrap();
        let text = fs::read_to_string(folder.join(PROJECT_FILE)).unwrap();
        let short = text.replace("take = 1", "take = 2");
        for file in [PROJECT_FILE, AUTOSAVE_FILE] {
            fs::write(folder.join(file), &short).unwrap();
            let loaded = if file == AUTOSAVE_FILE {
                Project::load_autosave(&folder)
            } else {
                Project::load(&folder)
            };
            let (loaded, warnings) = loaded.unwrap();
            assert_eq!(
                loaded.tracks[0].items, project.tracks[0].items,
                "{case} {file}"
            );
            let warnings: Vec<String> = warnings.iter().map(ToString::to_string).collect();
            assert_eq!(
                warnings,
                [
                    "track 'Gtr': item 1 now plays takes/gtr-2.wav: takes/gtr-3.wav ends before the item starts"
                ],
                "{case} {file}"
            );
            loaded.save(&folder).unwrap();
        }
    }
}

#[test]
fn saving_an_item_starting_past_the_end_of_its_take_names_it_by_id() {
    let dir = temp_dir("save-offset-past-take");
    let (folder, project) = project_with_items(
        &dir.0,
        vec![item(1, 0, None, 1000), item(7, 1000, None, 1000)],
    );
    assert_eq!(
        project.save(&folder).unwrap_err().to_string(),
        "track 'Gtr' item 7 starts past the end of its take"
    );
}

#[test]
fn a_file_without_item_ids_gets_them_in_file_order() {
    let dir = temp_dir("old-ids");
    let bass_items = "arm = false\n\n[[tracks.items]]\nposition = 0\ntake = 0\n\n[[tracks.items.takes]]\nfile = \"takes/bass-1.wav\"\nframes = 10\n\n[[tracks.items]]\nposition = 20\ntake = 0\n\n[[tracks.items.takes]]\nfile = \"takes/bass-2.wav\"\nframes = 10\n";
    let text = format!(
        "{}{bass_items}",
        TWO_TRACKS.strip_suffix("arm = false\n").unwrap()
    );
    fs::write(dir.0.join(PROJECT_FILE), text).unwrap();
    let project = Project::load(&dir.0).unwrap().0;
    let ids: Vec<Vec<u64>> = project
        .tracks
        .iter()
        .map(|track| track.items.iter().map(|item| item.id).collect())
        .collect();
    assert_eq!(ids, [vec![1], vec![2, 3]]);
    assert_eq!(project.next_item_id, 4);
}

#[test]
fn item_length_and_ids_are_checked() {
    let dir = temp_dir("item-checks");
    let cases = [
        (
            vec![item(1, 10, Some(991), 1000)],
            "track 'Gtr' item 0 plays past the end of its take",
        ),
        (
            vec![item(1, 0, Some(0), 1000)],
            "track 'Gtr' item 0 length must be at least 1",
        ),
        (
            vec![item(1, 0, None, 1000), item(1, 0, None, 1000)],
            "two items have id 1",
        ),
    ];
    for (idx, (items, message)) in cases.into_iter().enumerate() {
        let (folder, project) = project_with_items(&dir.0.join(idx.to_string()), items);
        assert_eq!(project.save(&folder).unwrap_err().to_string(), message);
    }
    let (folder, mut project) =
        project_with_items(&dir.0.join("next"), vec![item(4, 0, None, 1000)]);
    project.next_item_id = 4;
    assert_eq!(
        project.save(&folder).unwrap_err().to_string(),
        "next_item_id must be above every item id"
    );
    project.next_item_id = 5;
    project.tracks[0].items[0].length = Some(1000);
    project.save(&folder).unwrap();
    for past_last in [(1 << 63) - 1, 1 << 63] {
        project.next_item_id = past_last;
        assert_eq!(
            project.save(&folder).unwrap_err().to_string(),
            "next_item_id must be below 9223372036854775807"
        );
    }
}

#[test]
fn item_end_is_the_active_take_less_offset_or_the_length() {
    let mut item = item(1, 200, None, 1000);
    item.position = 50;
    assert_eq!((item.played_frames(), item.end()), (800, 850));
    item.length = Some(300);
    assert_eq!((item.played_frames(), item.end()), (300, 350));
}
