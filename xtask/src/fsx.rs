//! Files whose identity is checked before they are trusted. A path is examined
//! without following links, compared with what the caller already saw, opened
//! with `O_NOFOLLOW`, and the open descriptor compared once more, so a file
//! swapped in between the check and the open is refused rather than read.
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::Read as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::Path,
};

use anyhow::{Context, Result, ensure};

/// What identifies one file between two looks at it.
#[derive(Clone, Debug)]
pub struct FileState {
    device: u64,
    inode: u64,
    pub length: u64,
    links: u64,
}

impl FileState {
    pub fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            links: metadata.nlink(),
        }
    }

    pub fn matches(&self, metadata: &Metadata) -> bool {
        self.matches_state(&Self::from_metadata(metadata))
    }

    pub fn matches_state(&self, other: &Self) -> bool {
        self.length == other.length
            && self.device == other.device
            && self.inode == other.inode
            && self.links == other.links
    }
}

/// The metadata of a private regular file at `path`, never a link's target.
pub fn regular_metadata(path: &Path, label: &str) -> Result<Metadata> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("Missing {label}."))?;
    validate_regular_metadata(&metadata, label)?;
    Ok(metadata)
}

/// The identity of a private regular file at `path`.
pub fn checked_regular_file(path: &Path, label: &str) -> Result<FileState> {
    regular_metadata(path, label).map(|metadata| FileState::from_metadata(&metadata))
}

pub fn validate_regular_metadata(metadata: &Metadata, label: &str) -> Result<()> {
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "{label} must be a private regular file."
    );
    ensure!(metadata.nlink() == 1, "{label} must not be hard-linked.");
    Ok(())
}

/// Opens the file at `path` for reading only while it is still `expected`:
/// checked before the open, and again on the descriptor the open returned.
pub fn open_unchanged(path: &Path, expected: &FileState, label: &str) -> Result<File> {
    ensure!(
        checked_regular_file(path, label)?.matches_state(expected),
        "{label} changed before opening."
    );
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("could not open {label}"))?;
    let opened = file
        .metadata()
        .with_context(|| format!("could not inspect {label}"))?;
    ensure!(expected.matches(&opened), "{label} changed while opening.");
    Ok(file)
}

/// The whole of a private regular file of at most `maximum` bytes, refused if
/// it changes at any point between the first look and the last byte.
pub fn read_checked_file(path: &Path, label: &str, maximum: u64) -> Result<Vec<u8>> {
    let expected = checked_regular_file(path, label)?;
    ensure!(expected.length <= maximum, "{label} is unexpectedly large.");
    let mut file = open_unchanged(path, &expected, label)?;
    let mut bytes = Vec::with_capacity(usize::try_from(expected.length).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .with_context(|| format!("could not read {label}"))?;
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) == expected.length,
        "{label} changed while reading."
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn a_checked_file_is_opened_only_while_it_is_the_same_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("file");
        fs::write(&path, b"first").expect("file");
        let expected = checked_regular_file(&path, "fixture").expect("state");
        assert_eq!(
            read_checked_file(&path, "fixture", 5).expect("read"),
            b"first"
        );
        assert!(
            read_checked_file(&path, "fixture", 4)
                .unwrap_err()
                .to_string()
                .contains("unexpectedly large")
        );
        open_unchanged(&path, &expected, "fixture").expect("unchanged");

        // Replaced by another file of the same length: a new inode.
        fs::remove_file(&path).expect("remove");
        fs::write(&path, b"other").expect("replacement");
        assert!(
            open_unchanged(&path, &expected, "fixture")
                .unwrap_err()
                .to_string()
                .contains("changed before opening")
        );

        // A link is never followed, and a hard-linked file is refused.
        let link = temp.path().join("link");
        symlink(&path, &link).expect("symlink");
        assert!(checked_regular_file(&link, "fixture").is_err());
        assert!(read_checked_file(&link, "fixture", 5).is_err());
        fs::hard_link(&path, temp.path().join("hard")).expect("hard link");
        assert!(read_checked_file(&path, "fixture", 5).is_err());
    }
}
