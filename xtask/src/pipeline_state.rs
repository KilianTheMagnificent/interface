//! Private, atomic receipts shared by the worktrees of one repository.
use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// The user's home directory, which every per-user pipeline path hangs off.
pub(crate) fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is unavailable")
}

pub fn location(root: &Path) -> Result<PathBuf> {
    let common = crate::gate::git_capture(root, &["rev-parse", "--git-common-dir"])?;
    let path = root
        .join(common.trim())
        .canonicalize()?
        .join("interface-pipeline-v1");
    Ok(path)
}

pub fn directory(root: &Path) -> Result<PathBuf> {
    let path = location(root)?;
    private_directory(&path)?;
    Ok(path)
}

pub fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
    }
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "{} must be a real directory",
        path.display()
    );
    Ok(())
}

pub fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        meta.is_file() && meta.nlink() == 1 && meta.len() <= 1024 * 1024,
        "{} is not a bounded regular receipt",
        path.display()
    );
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file()
            && opened.nlink() == 1
            && opened.len() <= 1024 * 1024
            && opened.dev() == meta.dev()
            && opened.ino() == meta.ino(),
        "receipt changed while opening"
    );
    Ok(Some(
        serde_json::from_reader(file.take(1024 * 1024 + 1))
            .with_context(|| format!("invalid receipt {}", path.display()))?,
    ))
}

pub fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("receipt needs a parent")?;
    private_directory(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipts_reject_links_and_publication_lock_releases_after_failure() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original.json");
        write(&original, &"one").unwrap();
        let linked = temp.path().join("linked.json");
        std::os::unix::fs::symlink(&original, &linked).unwrap();
        assert!(read::<String>(&linked).is_err());
        let result: Result<()> =
            crate::release::with_pipeline_lock(temp.path(), ".test.lock", || {
                assert!(
                    crate::release::with_pipeline_lock(temp.path(), ".test.lock", || Ok(()))
                        .is_err()
                );
                anyhow::bail!("interrupted operation")
            });
        assert!(result.is_err());
        crate::release::with_pipeline_lock(temp.path(), ".test.lock", || Ok(())).unwrap();
    }
}
