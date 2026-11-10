use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use xguitare_core::Preset;
use xguitare_core::project::{
    AUTOSAVE_FILE, Item, LoopRegion, PROJECT_FILE, Project, Take, Track, autosave_due,
    autosave_newer,
};

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
    let project = Project::load(&dir.0).unwrap();

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
            position: 48_000,
            offset: 0,
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
    assert_eq!(Project::load(&copy.0).unwrap(), project);
}

#[test]
fn save_leaves_no_temp_file() {
    let dir = temp_dir("save-clean");
    fs::write(dir.0.join(PROJECT_FILE), TWO_TRACKS).unwrap();
    let project = Project::load(&dir.0).unwrap();
    project.save(&dir.0).unwrap();
    let names: Vec<_> = fs::read_dir(&dir.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![PROJECT_FILE]);
}

#[test]
fn new_creates_slugged_folder_with_takes() {
    let dir = temp_dir("new");
    let (folder, project) = Project::new(&dir.0, "  My First Song! ").unwrap();
    assert_eq!(folder, dir.0.join("my-first-song"));
    assert_eq!(project.name, "My First Song!");
    assert_eq!(project.tracks, []);
    assert!(folder.join("takes").is_dir());
    assert_eq!(Project::load(&folder).unwrap(), project);
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
    assert_eq!(Project::load_autosave(&folder).unwrap(), project);
    assert_eq!(Project::load(&folder).unwrap().tempo_bpm, 120.0);
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
    assert!(!Project::load(&dir.0).unwrap().loop_region.on);
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
        position: 0,
        offset: 0,
        take: 0,
        takes: vec![Take {
            file: "project.toml".to_string(),
            frames: 1,
        }],
    });
    stray.tracks.push(track);
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
        position: 0,
        offset: 256,
        take: 0,
        takes: vec![Take {
            file: "takes/gtr-1.wav".to_string(),
            frames: 1000,
        }],
    });
    project.tracks.push(track);
    project.save(&folder).unwrap();
    assert!(
        fs::read_to_string(folder.join(PROJECT_FILE))
            .unwrap()
            .contains("offset = 256")
    );
    assert_eq!(Project::load(&folder).unwrap(), project);
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
    let project = Project::load(&dir.0).unwrap();
    assert_eq!((project.click, project.count_in), (false, 0));
    assert_load_errors(
        "count-in",
        "tempo_bpm = 96.5",
        &["tempo_bpm = 96.5\ncount_in = 3"],
        "count-in must be 0, 1 or 2 bars",
    );
}
