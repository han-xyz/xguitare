use std::fs;
use std::path::PathBuf;

use xguitare_core::{Bank, BankEntry};

fn bank_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xguitare-bank-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("02-b.toml"), "name = \"B\"\n").unwrap();
    fs::write(dir.join("01-a.toml"), "name = \"A\"\n").unwrap();
    fs::write(dir.join("03-c.toml"), "name = \n").unwrap();
    fs::write(dir.join("notes.txt"), "not a preset").unwrap();
    dir
}

fn name(entry: &BankEntry) -> &str {
    match entry {
        BankEntry::Valid { preset, .. } => &preset.name,
        BankEntry::Invalid { .. } => "<invalid>",
    }
}

#[test]
fn sorted_by_filename_with_invalid_kept() {
    let dir = bank_dir("sorted");
    let bank = Bank::load_dir(&dir).unwrap();
    let names: Vec<&str> = bank.entries.iter().map(name).collect();
    assert_eq!(names, ["A", "B", "<invalid>"]);
    match &bank.entries[2] {
        BankEntry::Invalid { file, error } => {
            assert_eq!(file, &dir.join("03-c.toml"));
            assert!(error.starts_with("line 1:"), "{error}");
        }
        BankEntry::Valid { .. } => panic!("03-c.toml should be invalid"),
    }
}

#[test]
fn by_name_or_number() {
    let bank = Bank::load_dir(&bank_dir("lookup")).unwrap();
    let (index, entry) = bank.by_name_or_number("1").unwrap();
    assert_eq!((index, name(entry)), (1, "B"));
    let (index, entry) = bank.by_name_or_number("A").unwrap();
    assert_eq!((index, name(entry)), (0, "A"));
    assert!(bank.by_name_or_number("9").is_none());
    assert!(bank.by_name_or_number("zzz").is_none());
}
