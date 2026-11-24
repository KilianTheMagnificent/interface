//! Reads of provider-owned files that never follow a link out of their store.
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

/// A regular, singly linked file inside `root`, opened without following a
/// symbolic link at `path`. `None` for anything else.
pub(crate) fn safe_file(root: &Path, path: &Path) -> Option<File> {
    let root = root.canonicalize().ok()?;
    let canonical = path.canonicalize().ok()?;
    if !canonical.starts_with(&root) || fs::symlink_metadata(path).ok()?.file_type().is_symlink() {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    (meta.is_file() && meta.nlink() == 1).then_some(file)
}
