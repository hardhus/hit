//! Private, atomic file writes.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
};

/// Creates `dir` (and parents). On Unix the final directory is `0700`.
pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        // `mode` is ignored for directories that already exist.
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Writes `contents` to `path` atomically: a private temp file in the same
/// directory is written, synced and renamed over the target. On Unix the
/// file is `0600` from the moment it is created.
pub(crate) fn write_private_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        name.to_string_lossy(),
        std::process::id()
    ));

    let result = (|| {
        let mut file = open_private(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn open_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// On Unix, whether group or others can access `path`.
#[cfg(unix)]
pub(crate) fn is_too_open(path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path)?.permissions().mode() & 0o077 != 0)
}

/// Windows: files under the user profile are protected by its ACL.
#[cfg(not(unix))]
pub(crate) fn is_too_open(path: &Path) -> io::Result<bool> {
    fs::metadata(path).map(|_| false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        write_private_atomic(&path, b"one").unwrap();
        write_private_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temp file left behind");
        assert!(!is_too_open(&path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn private_dir_and_file_modes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("tokens");
        create_private_dir(&sub).unwrap();
        let mode = fs::metadata(&sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let file = sub.join("t");
        write_private_atomic(&file, b"x").unwrap();
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
