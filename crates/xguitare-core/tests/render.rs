use std::collections::HashMap;
use std::f32::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hound::{SampleFormat, WavReader};
use xguitare_core::arrangement::ArrangementCommand;
use xguitare_core::build::build_arrangement;
use xguitare_core::project::{Bus, Item, LoopRegion, Master, Project, Send, Take, Track};
use xguitare_core::render::{
    RENDER_BLOCK, Render, RenderBits, RenderError, RenderJob, RenderRange, RenderState, RenderStep,
    run_render,
};
use xguitare_core::{Config, PresetModule};

const RATE: u32 = 48_000;
const HALF_SECOND: usize = RATE as usize / 2;
const TEN_SECONDS: usize = RATE as usize * 10;
const LSB_16: f32 = 1.0 / 32_767.0;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-render-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn config(dir: &Path) -> Config {
    let path = dir.join("config.toml");
    fs::write(&path, "").unwrap();
    Config::load(Some(&path)).unwrap()
}

fn track(name: &str, items: Vec<Item>) -> Track {
    Track {
        name: name.to_string(),
        input: None,
        volume_db: 0.0,
        pan: 0.0,
        mute: false,
        solo: false,
        arm: false,
        to_master: true,
        sends: Vec::new(),
        modules: Vec::new(),
        items,
    }
}

fn item(position: u64, file: &str, frames: usize) -> Item {
    Item {
        position,
        offset: 0,
        take: 0,
        takes: vec![Take {
            file: file.to_string(),
            frames: frames as u64,
        }],
    }
}

fn project(tracks: Vec<Track>) -> Project {
    Project {
        name: "Song".to_string(),
        sample_rate: RATE,
        tempo_bpm: 120.0,
        meter: (4, 4),
        loop_region: LoopRegion::default(),
        latency_offset_ms: 0.0,
        click: false,
        count_in: 0,
        tracks,
        buses: Vec::new(),
        master: Master::default(),
    }
}

fn tone(frames: usize, level: f32) -> Vec<f32> {
    (0..frames)
        .map(|n| level * (TAU * 440.0 * n as f32 / RATE as f32).sin())
        .collect()
}

fn takes(entries: &[(&str, Vec<f32>)]) -> HashMap<String, Arc<[f32]>> {
    entries
        .iter()
        .map(|(file, samples)| ((*file).to_string(), Arc::from(samples.as_slice())))
        .collect()
}

fn module(type_name: &str, params: &[(&str, f32)]) -> PresetModule {
    PresetModule {
        id: type_name.to_string(),
        type_name: type_name.to_string(),
        ir: None,
        bypass: false,
        params: params
            .iter()
            .map(|(name, value)| ((*name).to_string(), *value))
            .collect(),
    }
}

fn render_all(render: &mut Render) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (Vec::new(), Vec::new());
    while let RenderStep::Audio { left: l, right: r } = render.step(RENDER_BLOCK) {
        left.extend_from_slice(l);
        right.extend_from_slice(r);
    }
    (left, right)
}

fn new_render(
    test: &str,
    project: &Project,
    takes: &HashMap<String, Arc<[f32]>>,
    range: RenderRange,
) -> Render {
    let dir = temp_dir(test);
    Render::new(&config(&dir.0), project, takes, range, RenderBits::Pcm24).unwrap()
}

#[test]
fn project_range_ends_half_a_second_after_the_last_item_goes_quiet() {
    let takes = takes(&[("takes/a-1.wav", tone(4_800, 0.5))]);
    let song = project(vec![track("a", vec![item(1_000, "takes/a-1.wav", 4_800)])]);
    let mut render = new_render("project-range", &song, &takes, RenderRange::Project);
    let (left, right) = render_all(&mut render);
    assert_eq!(left.len(), 5_800 + HALF_SECOND);
    assert_eq!(right.len(), left.len());
    assert!(left[1_000..5_800].iter().any(|s| s.abs() > 0.1));
    assert!((render.progress() - 1.0).abs() < f32::EPSILON);
}

#[test]
fn project_range_tail_stops_at_ten_seconds_with_a_long_ringing_reverb() {
    let takes = takes(&[("takes/a-1.wav", tone(4_800, 0.9))]);
    let mut song = project(vec![track("a", vec![item(0, "takes/a-1.wav", 4_800)])]);
    song.master.modules = vec![module(
        "reverb",
        &[("size", 100.0), ("damping", 0.0), ("mix", 100.0)],
    )];
    let mut render = new_render("tail-cap", &song, &takes, RenderRange::Project);
    let (left, _) = render_all(&mut render);
    assert_eq!(left.len(), 4_800 + TEN_SECONDS);
    let end = &left[left.len() - RENDER_BLOCK..];
    assert!(
        end.iter().any(|s| s.abs() > 3.2e-5),
        "the reverb should still ring above -90 dBFS at the cap"
    );
}

#[test]
fn loop_range_is_exactly_the_loop_region() {
    let samples = tone(20_000, 0.5);
    let takes = takes(&[("takes/a-1.wav", samples.clone())]);
    let mut song = project(vec![track("a", vec![item(0, "takes/a-1.wav", 20_000)])]);
    song.loop_region = LoopRegion {
        start: 3_000,
        end: 11_000,
        on: true,
    };
    let mut render = new_render("loop-range", &song, &takes, RenderRange::Loop);
    let (left, _) = render_all(&mut render);
    assert_eq!(left.len(), 8_000);
    for (rendered, take) in left.iter().zip(&samples[3_000..11_000]) {
        assert!((rendered - take).abs() < 1e-5, "{rendered} != {take}");
    }
}

#[test]
fn refusals_name_what_is_missing() {
    let dir = temp_dir("refusals");
    let config = config(&dir.0);
    let empty = project(vec![track("a", Vec::new())]);
    let error = Render::new(
        &config,
        &empty,
        &HashMap::new(),
        RenderRange::Project,
        RenderBits::Pcm24,
    )
    .err()
    .unwrap();
    assert!(matches!(error, RenderError::NothingToRender));
    assert_eq!(error.to_string(), "nothing to render");
    let takes = takes(&[("takes/a-1.wav", tone(100, 0.5))]);
    let mut song = project(vec![track("a", vec![item(0, "takes/a-1.wav", 100)])]);
    song.loop_region = LoopRegion {
        start: 500,
        end: 500,
        on: false,
    };
    let error = Render::new(&config, &song, &takes, RenderRange::Loop, RenderBits::Pcm24)
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "no loop region");
}

/// A Project with an armed Track that has an Item, a Track Rack, a Bus with a reverb fed by a Send, and the click on.
fn mixed_song() -> (Project, HashMap<String, Arc<[f32]>>) {
    let takes = takes(&[
        ("takes/a-1.wav", tone(9_000, 0.4)),
        ("takes/b-1.wav", tone(6_000, 0.3)),
    ]);
    let mut a = track("a", vec![item(0, "takes/a-1.wav", 9_000)]);
    a.arm = true;
    a.modules = vec![module("gain", &[("level", -6.0)])];
    a.sends = vec![Send {
        bus: 0,
        level_db: -3.0,
        pre: false,
    }];
    let mut b = track("b", vec![item(2_000, "takes/b-1.wav", 6_000)]);
    b.pan = 0.5;
    let mut song = project(vec![a, b]);
    song.click = true;
    song.buses = vec![Bus {
        id: 0,
        name: "Verb".to_string(),
        volume_db: -2.0,
        pan: 0.0,
        mute: false,
        solo: false,
        modules: vec![module("reverb", &[("mix", 50.0)])],
    }];
    song.master.volume_db = -1.0;
    (song, takes)
}

#[test]
fn rendered_audio_equals_the_arrangement_played_offline_without_input_or_click() {
    let (song, takes) = mixed_song();
    let mut render = new_render("mixed", &song, &takes, RenderRange::Project);
    let (left, right) = render_all(&mut render);

    let dir = temp_dir("offline");
    let mut offline_song = song.clone();
    for track in &mut offline_song.tracks {
        track.arm = false;
    }
    let (mut arrangement, _) = build_arrangement(
        &config(&dir.0),
        &offline_song,
        &takes,
        RATE as f32,
        RENDER_BLOCK,
    )
    .unwrap();
    arrangement.apply(ArrangementCommand::Play);
    let loud_input = vec![0.9; RENDER_BLOCK];
    let (mut expected_l, mut expected_r) = (Vec::new(), Vec::new());
    while expected_l.len() < left.len() {
        let frames = RENDER_BLOCK.min(left.len() - expected_l.len());
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        arrangement.process(&[&loud_input, &loud_input], &mut l, &mut r);
        expected_l.extend(l);
        expected_r.extend(r);
    }
    assert!(left.len() > 9_000);
    assert!(left.iter().any(|s| s.abs() > 0.05));
    for (idx, (got, want)) in left
        .iter()
        .chain(&right)
        .zip(expected_l.iter().chain(&expected_r))
        .enumerate()
    {
        assert!((got - want).abs() < 1e-6, "sample {idx}: {got} != {want}");
    }
}

fn job(dir: &Path, name: &str) -> (RenderJob, PathBuf) {
    let file = dir.join(format!("{name}.wav"));
    (
        RenderJob::new(name.to_string(), RenderRange::Loop, dir.to_path_buf()),
        file,
    )
}

fn loop_song() -> (Project, HashMap<String, Arc<[f32]>>) {
    let mut samples = vec![0.0; 4_800];
    samples.extend(tone(4_800, 0.5));
    let takes = takes(&[("takes/a-1.wav", samples)]);
    let mut song = project(vec![track("a", vec![item(0, "takes/a-1.wav", 9_600)])]);
    song.loop_region = LoopRegion {
        start: 0,
        end: 9_600,
        on: false,
    };
    (song, takes)
}

fn render_file(dir: &Path, bits: RenderBits, name: &str) -> PathBuf {
    let (song, takes) = loop_song();
    let render = Render::new(&config(dir), &song, &takes, RenderRange::Loop, bits).unwrap();
    let (job, file) = job(dir, name);
    run_render(render, &job);
    assert_eq!(job.state(), RenderState::Done, "{:?}", job.error());
    assert!(!dir.join(format!("{name}.wav.part")).exists());
    file
}

#[test]
fn each_bit_depth_writes_a_stereo_wav_at_the_project_rate() {
    let dir = temp_dir("bits");
    for (bits, name, depth, format) in [
        (RenderBits::Pcm16, "sixteen", 16, SampleFormat::Int),
        (RenderBits::Pcm24, "twenty-four", 24, SampleFormat::Int),
        (RenderBits::Float32, "float", 32, SampleFormat::Float),
    ] {
        let reader = WavReader::open(render_file(&dir.0, bits, name)).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 2, "{name}");
        assert_eq!(spec.sample_rate, RATE, "{name}");
        assert_eq!(spec.bits_per_sample, depth, "{name}");
        assert_eq!(spec.sample_format, format, "{name}");
        assert_eq!(reader.duration(), 9_600, "{name}");
    }
}

#[test]
fn sixteen_bit_is_dithered_on_silence_and_follows_the_24_bit_render_elsewhere() {
    let dir = temp_dir("dither");
    let read = |file: PathBuf, scale: f32| -> Vec<f32> {
        WavReader::open(file)
            .unwrap()
            .into_samples::<i32>()
            .map(|sample| sample.unwrap() as f32 * scale)
            .collect()
    };
    let sixteen = read(
        render_file(&dir.0, RenderBits::Pcm16, "sixteen"),
        1.0 / 32_767.0,
    );
    let twenty_four = read(
        render_file(&dir.0, RenderBits::Pcm24, "twenty-four"),
        1.0 / 8_388_607.0,
    );
    let silence = &sixteen[..2 * 4_800];
    assert!(silence.iter().any(|&s| s != 0.0), "silence carries dither");
    assert!(silence.iter().all(|s| s.abs() <= LSB_16));
    for (a, b) in sixteen.iter().zip(&twenty_four) {
        assert!((a - b).abs() < 1.5 * LSB_16, "{a} vs {b}");
    }
}

#[test]
fn a_cancelled_render_leaves_no_file() {
    let dir = temp_dir("cancel");
    let (song, takes) = loop_song();
    let render = Render::new(
        &config(&dir.0),
        &song,
        &takes,
        RenderRange::Loop,
        RenderBits::Pcm24,
    )
    .unwrap();
    let (job, file) = job(&dir.0, "song");
    job.cancel();
    run_render(render, &job);
    assert_eq!(job.state(), RenderState::Cancelled);
    assert!(!file.exists());
    assert!(!dir.0.join("song.wav.part").exists());
}

#[test]
fn a_failed_write_reports_the_error_and_leaves_no_part_file() {
    let dir = temp_dir("failed");
    let (song, takes) = loop_song();
    let render = Render::new(
        &config(&dir.0),
        &song,
        &takes,
        RenderRange::Loop,
        RenderBits::Pcm24,
    )
    .unwrap();
    let (job, file) = job(&dir.0.join("missing"), "song");
    run_render(render, &job);
    assert_eq!(job.state(), RenderState::Failed);
    assert!(job.error().unwrap().starts_with("cannot write "));
    assert!(!file.exists());
}
