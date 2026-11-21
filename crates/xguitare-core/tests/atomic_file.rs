use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use xguitare_core::atomic_file::replace;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!(
        "xguitare-atomic-file-{}-{test}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    entries
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn replace_replaces_content() {
    let dir = temp_dir("content");
    let file = dir.0.join("preset.toml");
    fs::write(&file, "old").unwrap();
    replace(&file, b"new", None).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"new");
}

#[test]
fn replace_leaves_no_temp_file() {
    let dir = temp_dir("no-temp");
    let file = dir.0.join("preset.toml");
    replace(&file, b"first", None).unwrap();
    replace(&file, b"second", None).unwrap();
    assert_eq!(entries(&dir.0), vec![file]);
}

#[test]
fn replace_onto_a_directory_fails_and_leaves_no_temp_file() {
    let dir = temp_dir("onto-directory");
    let target = dir.0.join("project.toml");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("inside"), "keep").unwrap();
    assert!(replace(&target, b"new", None).is_err());
    assert_eq!(entries(&target), vec![target.join("inside")]);
    assert_eq!(fs::read(target.join("inside")).unwrap(), b"keep");
    assert_eq!(entries(&dir.0), vec![target]);
}

#[test]
fn replace_in_a_read_only_directory_keeps_the_old_file() {
    let dir = temp_dir("read-only");
    let file = dir.0.join("state.toml");
    fs::write(&file, "old bytes").unwrap();
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o555)).unwrap();
    let result = replace(&file, b"new", None);
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(&file).unwrap(), b"old bytes");
    assert_eq!(entries(&dir.0), vec![file]);
}

#[test]
fn replace_sets_a_mode_the_umask_would_mask() {
    let dir = temp_dir("exact-mode");
    let file = dir.0.join("preset.toml");
    replace(&file, b"text", Some(0o666)).unwrap();
    assert_eq!(mode(&file), 0o666);
}

#[test]
fn replace_without_a_mode_gives_the_default() {
    let dir = temp_dir("default-mode");
    let file = dir.0.join("project.toml");
    let plain = dir.0.join("plain");
    fs::write(&plain, "").unwrap();
    replace(&file, b"text", None).unwrap();
    assert_eq!(mode(&file), mode(&plain));
}
