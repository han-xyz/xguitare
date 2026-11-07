use std::fs;
use std::io;
use std::path::Path;

const IR_EXTENSION: &str = "wav";

/// Every `.wav` file (extension case-insensitive) under `dir`, recursively, as `/`-separated names relative to `dir`, sorted bytewise. Entries whose name starts with `.` are skipped; a symlink is followed, but not a symlink inside a directory reached through one. An unreadable directory fails with `ir_dir <dir>: <io error>`.
pub fn list_irs(dir: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    collect(dir, "", false, &mut names)
        .map_err(|err| format!("ir_dir {}: {err}", dir.display()))?;
    names.sort();
    Ok(names)
}

fn collect(
    dir: &Path,
    prefix: &str,
    through_link: bool,
    names: &mut Vec<String>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_link = entry.file_type()?.is_symlink();
        if is_link && through_link {
            continue;
        }
        let Ok(meta) = fs::metadata(entry.path()) else {
            continue;
        };
        let relative = format!("{prefix}{name}");
        if meta.is_dir() {
            collect(
                &entry.path(),
                &format!("{relative}/"),
                through_link || is_link,
                names,
            )?;
        } else if meta.is_file() && is_ir_name(&name) {
            names.push(relative);
        }
    }
    Ok(())
}

fn is_ir_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(IR_EXTENSION))
}
