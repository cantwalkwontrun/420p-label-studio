//! Save label projects without truncating an existing file in place.
//!
//! Staging and temporary recovery data stay in a new dedicated directory beside
//! the target, so replacement is on the same volume. The caller remains
//! responsible for reporting save failures and keeping unsaved edits available.
use std::{fs, io::{self, Read, Write}, path::{Path, PathBuf}, sync::atomic::{AtomicU64, Ordering}};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub const NATIVE_LIMIT: usize = 16 * 1024 * 1024;
pub const NATIVE_BASE64_LIMIT: usize = NATIVE_LIMIT.div_ceil(3) * 4;
// A project may embed a complete native label as base64, plus editing settings.
pub const PROJECT_LIMIT: usize = 24 * 1024 * 1024;

pub fn read_label(path: &Path) -> io::Result<Vec<u8>> {
    let native = path.extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("dymo") || s.eq_ignore_ascii_case("label"));
    read_bounded(path, if native { NATIVE_LIMIT } else { PROJECT_LIMIT })
}

pub fn read_bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("File exceeds the {} MiB limit.", limit / (1024 * 1024))));
    }
    Ok(bytes)
}

/// Write a new project, or replace an existing project when `overwrite` is true.
/// Callers with an earlier read of the file should pass it to write_checked.
pub fn write_project(path: &Path, bytes: &[u8], overwrite: bool) -> Result<(), String> {
    let previous = if overwrite {
        Some(read_bounded(path, PROJECT_LIMIT).map_err(|e| format!("Could not read the existing file: {e}"))?)
    } else { None };
    write_checked(path, bytes, previous.as_deref(), |_| Ok(()))
}

/// Keep writers out while checking this file. Share deletion so ReplaceFileW
/// can move it. A concurrent rename can still substitute a different file.
fn matching_file(path: &Path, expected: &[u8]) -> io::Result<fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_DELETE: u32 = 4;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let mut file = fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("The destination is a reparse point."));
    }
    let mut actual = Vec::new();
    (&mut file).take(expected.len() as u64 + 1).read_to_end(&mut actual)?;
    if actual != expected { return Err(io::Error::other("The file contents changed.")); }
    Ok(file)
}

/// Check the staged file and replace only the revision the caller agreed to overwrite.
pub fn write_checked(path: &Path, bytes: &[u8], previous: Option<&[u8]>, validate: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    if path.file_name().is_none() { return Err("Choose a project file name.".into()); }
    if previous.is_some() {
        let meta = fs::symlink_metadata(path).map_err(|e| format!("Could not inspect the existing project: {e}"))?;
        if !meta.file_type().is_file() { return Err("The existing project is not a regular file.".into()); }
    }
    let staging = create_staging_dir(parent)?;
    let replacement = staging.join("new").with_extension(path.extension().unwrap_or_default());
    let backup = staging.join("previous").with_extension(path.extension().unwrap_or_default());
    let prepared = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&replacement)
            .map_err(|e| format!("Could not prepare the project: {e}"))?;
        file.write_all(bytes).map_err(|e| format!("Could not write the project: {e}"))?;
        file.sync_all().map_err(|e| format!("Could not finish writing the project: {e}"))?;
        drop(file);
        validate(&replacement)
    })();
    if let Err(error) = prepared {
        return Err(cleanup_error(error, &replacement, &staging));
    }

    let Some(previous) = previous else {
        if let Err(error) = move_new(&replacement, path) {
            return Err(cleanup_error(format!("Could not create the project without overwriting an existing file: {error}"), &replacement, &staging));
        }
        return fs::remove_dir(&staging).map_err(|e| format!("Project saved, but the empty temporary folder could not be removed ({}): {e}", staging.display()));
    };

    let original_guard = matching_file(path, previous).map_err(|e| cleanup_error(
        format!("The destination changed or could not be checked before saving: {e} Choose another file to preserve your edits."),
        &replacement, &staging))?;
    if let Err(error) = replace_existing(path, &replacement, &backup) {
        // ReplaceFileW may move the old project to the backup even on failure.
        // Restore only if the original name is absent; never overwrite a file.
        if !path.exists() && backup.exists() {
            if let Err(restore_error) = move_new(&backup, path) {
                return Err(format!("Could not save the project: {error}. The original is retained at {}; recovery to {} failed: {restore_error}. The new version is at {}.", backup.display(), path.display(), replacement.display()));
            }
        }
        if backup.exists() || !path.exists() {
            return Err(format!("Could not save the project: {error}. Recovery files were retained in {}; check them before retrying.", staging.display()));
        }
        return Err(cleanup_error(format!("Could not save the project; the existing file was retained: {error}"), &replacement, &staging));
    }

    // The path may have been replaced between opening it and ReplaceFileW.
    // Check the actual file that was displaced before deleting any recovery data.
    verify_replacement(path, bytes, &backup, previous)?;
    drop(original_guard);
    fs::remove_dir(&staging).map_err(|e| format!("The project was saved and verified, but its empty temporary folder could not be removed ({}): {e}", staging.display()))?;
    Ok(())
}

fn verify_replacement(path: &Path, bytes: &[u8], backup: &Path, previous: &[u8]) -> Result<(), String> {
    let _displaced_guard = matching_file(backup, previous).map_err(|e| format!(
        "Concurrent save conflict: {e} The displaced file is kept at {}. The requested save went to {}. Check both files before retrying.",
        backup.display(), path.display()))?;
    let _saved_guard = matching_file(path, bytes).map_err(|e| format!(
        "Could not verify the saved project: {e} The previous version is retained at {}.", backup.display()))?;
    fs::remove_file(backup).map_err(|e| format!("The project was saved and verified, but its temporary previous copy could not be removed ({}): {e}", backup.display()))
}

pub(crate) fn create_staging_dir(parent: &Path) -> Result<PathBuf, String> {
    for _ in 0..32 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!(".dymo-save-{}-{id}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Could not create a temporary project folder: {e}")),
        }
    }
    Err("Could not allocate a unique temporary project folder.".into())
}

fn cleanup_error(error: String, replacement: &Path, staging: &Path) -> String {
    // Only these exact files/directories were created by this save operation.
    let file_error = match fs::remove_file(replacement) {
        Ok(()) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(e),
    };
    let directory_error = fs::remove_dir(staging).err();
    if file_error.is_some() || directory_error.is_some() {
        format!("{error}. Temporary recovery data may remain in {}.", staging.display())
    } else { error }
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn move_new(from: &Path, to: &Path) -> std::io::Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" { fn MoveFileW(existing: *const u16, new: *const u16) -> i32; }
    let (from, to) = (wide(from), wide(to));
    // SAFETY: Both pointers refer to live, NUL-terminated UTF-16 buffers.
    if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

#[cfg(windows)]
fn replace_existing(target: &Path, replacement: &Path, backup: &Path) -> std::io::Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" { fn ReplaceFileW(replaced: *const u16, replacement: *const u16, backup: *const u16, flags: u32, exclude: *mut std::ffi::c_void, reserved: *mut std::ffi::c_void) -> i32; }
    let (target, replacement, backup) = (wide(target), wide(replacement), wide(backup));
    // SAFETY: The three paths are valid, live, NUL-terminated UTF-16 buffers;
    // the final two reserved arguments must be null. The backup is on the same volume.
    let result = unsafe { ReplaceFileW(target.as_ptr(), replacement.as_ptr(), backup.as_ptr(), 0, std::ptr::null_mut(), std::ptr::null_mut()) };
    if result == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let base = std::env::temp_dir();
        create_staging_dir(&base).expect("create isolated test folder")
    }
    #[test]
    fn save_new_and_replace_verify_contents_and_clean_staging() {
        let dir = fixture();
        let path = dir.join("label.420p");
        write_project(&path, b"first", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        assert!(write_project(&path, b"should not overwrite", false).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        write_project(&path, b"second", true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn failed_staged_validation_keeps_the_previous_file_and_leaves_new_names_free_for_retry() {
        let dir = fixture();
        let existing = dir.join("old.dymo");
        let fresh = dir.join("new.dymo");
        fs::write(&existing, b"original").unwrap();
        for (path, previous) in [(&fresh, None), (&existing, Some(b"original".as_slice()))] {
            let error = write_checked(path, b"candidate", previous, |staged| {
                assert_eq!(staged.extension(), path.extension());
                assert_eq!(fs::read(staged).unwrap(), b"candidate");
                Err("unusable label".into())
            }).unwrap_err();
            assert!(error.contains("unusable label"));
        }
        assert!(!fresh.exists());
        assert_eq!(fs::read(&existing).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        write_project(&fresh, b"usable", false).unwrap();
        fs::remove_file(fresh).unwrap();
        fs::remove_file(existing).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn a_revision_changed_during_staging_is_kept_in_place() {
        let dir = fixture();
        let path = dir.join("draft.420p");
        fs::write(&path, b"previous").unwrap();
        let result = write_checked(&path, b"Studio edit", Some(b"previous"), |_| {
            fs::write(&path, b"concurrent revision").unwrap(); Ok(())
        });
        assert!(result.unwrap_err().contains("changed"));
        assert_eq!(fs::read(&path).unwrap(), b"concurrent revision");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn raced_rename_retains_both_revisions_when_the_displaced_file_differs() {
        let dir = fixture();
        let path = dir.join("label.dymo");
        let previous = dir.join("previous.dymo");
        let candidate = dir.join("new.dymo");
        let original = dir.join("original.dymo");
        fs::write(&path, b"baseline").unwrap();
        let guard = matching_file(&path, b"baseline").unwrap();
        fs::rename(&path, &original).unwrap();
        fs::write(&path, b"external revision").unwrap();
        fs::write(&candidate, b"studio revision").unwrap();
        replace_existing(&path, &candidate, &previous).unwrap();
        let error = verify_replacement(&path, b"studio revision", &previous, b"baseline").unwrap_err();
        assert!(error.contains("Concurrent save conflict"));
        assert_eq!(fs::read(&path).unwrap(), b"studio revision");
        assert_eq!(fs::read(&previous).unwrap(), b"external revision");
        drop(guard);
        for file in [path, previous, original] { fs::remove_file(file).unwrap(); }
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn overwrite_requires_an_existing_regular_file() {
        let dir = fixture();
        assert!(write_project(&dir.join("missing.420p"), b"content", true).is_err());
        assert!(write_project(&dir, b"content", true).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir(dir).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn locked_original_remains_intact_when_replacement_fails() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = fixture();
        let path = dir.join("label.420p");
        fs::write(&path, b"old").unwrap();
        let locked = fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        assert!(write_project(&path, b"new", true).is_err());
        drop(locked);
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
