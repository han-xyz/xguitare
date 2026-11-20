use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Linux `EPERM`, which `hard_link` returns on a filesystem without hard links; `EACCES` shares its `ErrorKind`.
const EPERM: i32 = 1;

/// The name lowercased, ASCII letters and digits kept, every other run of characters one `-`, no `-` at either end.
pub(crate) fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// Replaces the file `path` names, through a symlink, by a temp file and rename beside it, keeping the old file's mode.
pub(crate) fn write_through(path: &Path, text: &str) -> io::Result<()> {
    let target = if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    let permissions = match fs::metadata(&target) {
        Ok(meta) => Some(meta.permissions()),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let tmp = temp_path(&target);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    if let Some(permissions) = &permissions {
        options.mode(permissions.mode());
    }
    let mut file = options.open(&tmp)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| match permissions {
            Some(permissions) => file.set_permissions(permissions),
            None => Ok(()),
        })
        .and_then(|()| fs::rename(&tmp, &target));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Creates the file `path` names with `text` by `link`ing a temp file beside it to `path` (`save` with `as` passes `fs::hard_link`), so a file or symlink already there fails with `AlreadyExists` and is left as it is. When the filesystem has no hard links (EPERM, EOPNOTSUPP, EXDEV), `path` is created and written directly, still never replacing a file.
///
/// # Errors
///
/// `AlreadyExists` when `path` already names a file or symlink; the I/O error when the temp file or `path` cannot be created or written.
pub fn write_new(
    path: &Path,
    text: &str,
    link: impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let tmp = temp_path(path);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| link(&tmp, path));
    let _ = fs::remove_file(&tmp);
    match written {
        Err(err) if has_no_hard_links(&err) => create_and_write(path, text),
        written => written,
    }
}

pub(crate) fn has_no_hard_links(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::Unsupported | ErrorKind::CrossesDevices
    ) || err.raw_os_error() == Some(EPERM)
}

fn create_and_write(path: &Path, text: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let written = file.write_all(text.as_bytes());
    if written.is_err() {
        let _ = fs::remove_file(path);
    }
    written
}

/// `<file name>.<pid>.<n>.tmp` beside `target`, unique within this process.
pub(crate) fn temp_path(target: &Path) -> PathBuf {
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let n = SAVES.fetch_add(1, Ordering::Relaxed);
    let mut name = target.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    target.with_file_name(name)
}
