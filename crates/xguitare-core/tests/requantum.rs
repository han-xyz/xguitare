use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use xguitare_core::control::{EngineState, Reprepare, Request, Response, handle, reprepare};
use xguitare_core::engine::{AudioQueues, Command, EngineShared, engine_queues};
use xguitare_core::{Bank, Config, Preset, PresetModule};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cabinet(id: &str) -> PresetModule {
    PresetModule {
        id: id.to_string(),
        type_name: "cabinet".to_string(),
        ir: None,
        params: Vec::new(),
    }
}

fn state_with_unbuildable_preset(dir: &TempDir) -> (EngineState, AudioQueues) {
    let preset = Preset {
        name: "Two Cabs".to_string(),
        modules: vec![cabinet("cab1"), cabinet("cab2")],
    };
    let mut config = Config::load(Some(&dir.0.join("absent.toml"))).unwrap();
    config.bank_dir = dir.0.clone();
    let (queues, audio) = engine_queues(64);
    let state = EngineState {
        bank: Bank::load_dir(&dir.0).unwrap(),
        active: Some((dir.0.join("two-cabs.toml"), preset)),
        shared: Arc::new(EngineShared::default()),
        queues,
        config,
        connections: Vec::new(),
        sample_rate: 48_000.0,
        max_frames: 128,
    };
    (state, audio)
}

#[test]
fn build_failure_falls_back_to_passthrough() {
    let dir = TempDir(std::env::temp_dir().join(format!(
        "xguitare-requantum-{}-fallback",
        std::process::id()
    )));
    fs::create_dir_all(&dir.0).unwrap();
    let (mut state, mut audio) = state_with_unbuildable_preset(&dir);
    let outcome = reprepare(&mut state, 256).unwrap();
    assert_eq!(
        outcome,
        Reprepare::Passthrough {
            preset: "Two Cabs".to_string(),
            error: "more than one cabinet".to_string(),
        }
    );
    let Ok(Command::SwapRack(rack)) = audio.commands.pop() else {
        panic!("expected SwapRack");
    };
    assert_eq!(rack.max_frames(), 256);
    assert!(rack.modules().is_empty());
    let Response::Ok(status) = handle(&mut state, Request::Status).0 else {
        panic!("status failed");
    };
    assert_eq!(status["preset"], serde_json::Value::Null);
    assert_eq!(status["prepared_frames"], 256);
}
