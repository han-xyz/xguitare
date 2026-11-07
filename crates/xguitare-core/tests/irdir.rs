use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use xguitare_core::list_irs;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(test: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("xguitare-irdir-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn touch(dir: &Path, files: &[&str]) {
    for file in files {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
}

#[test]
fn list_irs_skips_dot_files_and_dot_dirs() {
    let dir = temp_dir("dot");
    touch(&dir.0, &["a.wav", ".b.wav", ".pack/c.wav", "pack/.d.wav"]);
    symlink(dir.0.join("a.wav"), dir.0.join(".link.wav")).unwrap();
    assert_eq!(list_irs(&dir.0).unwrap(), ["a.wav"]);
}

#[test]
fn list_irs_ignores_non_wav() {
    let dir = temp_dir("non-wav");
    touch(
        &dir.0,
        &[
            "a.wav",
            "b.WaV",
            "notes.txt",
            "c.wav.bak",
            "wav",
            "pack.wav/e.wav",
        ],
    );
    assert_eq!(
        list_irs(&dir.0).unwrap(),
        ["a.wav", "b.WaV", "pack.wav/e.wav"]
    );
}

#[test]
fn list_irs_skips_non_utf8_names() {
    let dir = temp_dir("non-utf8");
    touch(&dir.0, &["a.wav"]);
    fs::write(dir.0.join(OsStr::from_bytes(b"\xff.wav")), "").unwrap();
    assert_eq!(list_irs(&dir.0).unwrap(), ["a.wav"]);
}

#[test]
fn list_irs_sorted_bytewise() {
    let dir = temp_dir("sorted");
    touch(&dir.0, &["b.wav", "a/z.wav", "B.wav", "a.wav", "a-b.wav"]);
    assert_eq!(
        list_irs(&dir.0).unwrap(),
        ["B.wav", "a-b.wav", "a.wav", "a/z.wav", "b.wav"]
    );
}

#[test]
fn list_irs_follows_symlinks_to_files_and_dirs() {
    let dir = temp_dir("follow");
    let outside = temp_dir("follow-outside");
    touch(&outside.0, &["x.wav", "cabs/y.wav"]);
    symlink(outside.0.join("x.wav"), dir.0.join("linked.wav")).unwrap();
    symlink(outside.0.join("cabs"), dir.0.join("cabs")).unwrap();
    assert_eq!(list_irs(&dir.0).unwrap(), ["cabs/y.wav", "linked.wav"]);
}

#[test]
fn list_irs_skips_symlinks_inside_a_linked_dir() {
    let dir = temp_dir("nested-link");
    let outside = temp_dir("nested-link-outside");
    touch(&outside.0, &["pack/a.wav", "b.wav"]);
    symlink(outside.0.join("b.wav"), outside.0.join("pack/b.wav")).unwrap();
    symlink(outside.0.join("pack"), outside.0.join("pack/loop")).unwrap();
    symlink(outside.0.join("pack"), dir.0.join("pack")).unwrap();
    assert_eq!(list_irs(&dir.0).unwrap(), ["pack/a.wav"]);
}

#[test]
fn list_irs_skips_dangling_symlinks() {
    let dir = temp_dir("dangling");
    touch(&dir.0, &["a.wav"]);
    symlink(dir.0.join("absent.wav"), dir.0.join("gone.wav")).unwrap();
    assert_eq!(list_irs(&dir.0).unwrap(), ["a.wav"]);
}
