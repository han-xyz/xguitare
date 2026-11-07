use std::fs::{self, ReadDir};
use std::path::Path;

const IR_EXTENSION: &str = "wav";

/// Every `.wav` file (extension case-insensitive) under `dir`, recursively, as `/`-separated names relative to `dir`, sorted bytewise. Entries whose name starts with `.` or is not UTF-8 are skipped, and so are subdirectories that cannot be read; a symlink is followed, but not a symlink inside a directory reached through one. Fails with `ir_dir <dir>: <io error>` only when `dir` itself cannot be read.
pub fn list_irs(dir: &Path) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("ir_dir {}: {err}", dir.display()))?;
    let mut names = Vec::new();
    collect(entries, "", false, &mut names);
    names.sort();
    Ok(names)
}

fn collect(entries: ReadDir, prefix: &str, through_link: bool, names: &mut Vec<String>) {
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_link = file_type.is_symlink();
        if is_link && through_link {
            continue;
        }
        let Ok(meta) = fs::metadata(entry.path()) else {
            continue;
        };
        let relative = format!("{prefix}{name}");
        if meta.is_dir() {
            if let Ok(sub_entries) = fs::read_dir(entry.path()) {
                collect(
                    sub_entries,
                    &format!("{relative}/"),
                    through_link || is_link,
                    names,
                );
            }
        } else if meta.is_file() && is_ir_name(&name) {
            names.push(relative);
        }
    }
}

fn is_ir_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(IR_EXTENSION))
}
