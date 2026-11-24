use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

use interface_storage::PrivateDirectory;

const UTF8_MAX_BYTES_PER_SCALAR: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StableFileMetadata {
    device: u64,
    inode: u64,
    length: u64,
    mode: u32,
    owner: u32,
    links: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

/// Reads the final `max_utf16_units` from an owner-private regular file.
///
/// The frozen JavaScript surfaces apply their limits after UTF-8 replacement
/// decoding and count UTF-16 code units. Reading four bytes per requested unit
/// plus one possible leading scalar is sufficient to reconstruct that suffix
/// without ever buffering an arbitrary-length log.
pub(crate) fn read_private_tail(
    directory: &PrivateDirectory,
    name: &str,
    max_utf16_units: usize,
) -> Result<Option<String>, String> {
    if max_utf16_units == 0 {
        return Err("tail character limit must be positive".into());
    }
    validate_name(name)?;
    let scan_bytes = max_utf16_units
        .checked_mul(UTF8_MAX_BYTES_PER_SCALAR)
        .and_then(|value| value.checked_add(UTF8_MAX_BYTES_PER_SCALAR - 1))
        .ok_or_else(|| "tail character limit is too large".to_owned())?;

    directory.assert_unchanged().map_err(display_error)?;
    let path = directory.path().join(name);
    let before = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            directory.assert_unchanged().map_err(display_error)?;
            return match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.to_string()),
                Ok(_) => Err("private tail target changed while absence was checked".into()),
            };
        }
        Err(error) => return Err(error.to_string()),
    };

    let expected_owner = fs::symlink_metadata(directory.path())
        .map_err(|error| error.to_string())?
        .uid();
    let before = validate_metadata(&path, &before, expected_owner)?;

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|error| error.to_string())?;

    let opened = validate_metadata(
        &path,
        &file.metadata().map_err(|error| error.to_string())?,
        expected_owner,
    )?;
    if opened != before {
        return Err("private tail target changed while it was opened".into());
    }

    let length = opened.length;
    let start = length.saturating_sub(u64::try_from(scan_bytes).unwrap_or(u64::MAX));
    file.seek(SeekFrom::Start(start))
        .map_err(|error| error.to_string())?;
    let expected_length = length.saturating_sub(start);
    let mut bytes = Vec::with_capacity(
        usize::try_from(expected_length)
            .unwrap_or(scan_bytes)
            .min(scan_bytes),
    );
    file.by_ref()
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_length {
        return Err("private tail target changed while it was read".into());
    }

    let after = validate_metadata(
        &path,
        &file.metadata().map_err(|error| error.to_string())?,
        expected_owner,
    )?;
    let named = validate_metadata(
        &path,
        &fs::symlink_metadata(&path).map_err(|error| error.to_string())?,
        expected_owner,
    )?;
    if after != opened || named != opened {
        return Err("private tail target changed while it was read".into());
    }
    directory.assert_unchanged().map_err(display_error)?;

    Ok(Some(tail_utf16_units(
        &String::from_utf8_lossy(&bytes),
        max_utf16_units,
    )))
}

fn tail_utf16_units(value: &str, max_units: usize) -> String {
    let total_units = value.encode_utf16().count();
    if total_units <= max_units {
        return value.to_owned();
    }
    let units_to_skip = total_units - max_units;
    let mut skipped = 0;
    for (byte_index, character) in value.char_indices() {
        let width = character.len_utf16();
        match (skipped + width).cmp(&units_to_skip) {
            std::cmp::Ordering::Less => skipped += width,
            std::cmp::Ordering::Equal => {
                return value[byte_index + character.len_utf8()..].to_owned();
            }
            std::cmp::Ordering::Greater => {
                let mut tail = String::from('\u{fffd}');
                tail.push_str(&value[byte_index + character.len_utf8()..]);
                return tail;
            }
        }
    }
    String::new()
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("private tail filename is invalid".into());
    }
    Ok(())
}

fn validate_metadata(
    path: &std::path::Path,
    metadata: &fs::Metadata,
    expected_owner: u32,
) -> Result<StableFileMetadata, String> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is not a real regular file",
            path.to_string_lossy()
        ));
    }
    if metadata.uid() != expected_owner {
        return Err(format!("{} has the wrong owner", path.to_string_lossy()));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return Err(format!("{} must have mode 0600", path.to_string_lossy()));
    }
    if metadata.nlink() != 1 {
        return Err(format!(
            "{} must not be hard linked",
            path.to_string_lossy()
        ));
    }
    Ok(StableFileMetadata {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
        mode: metadata.mode(),
        owner: metadata.uid(),
        links: metadata.nlink(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    fn directory() -> (tempfile::TempDir, PrivateDirectory) {
        let temporary = tempfile::tempdir().expect("tempdir");
        let root = temporary.path().join("logs");
        fs::create_dir(&root).expect("directory");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("directory mode");
        let directory = PrivateDirectory::open(&root).expect("private directory");
        (temporary, directory)
    }

    #[test]
    fn reads_only_the_requested_utf16_suffix() {
        assert_eq!(tail_utf16_units("abcdef", 3), "def");
        assert_eq!(tail_utf16_units("a😀b", 3), "😀b");
        assert_eq!(tail_utf16_units("a😀b", 2), "�b");
    }

    #[test]
    fn large_and_invalid_utf8_files_are_replacement_decoded_and_tailed() {
        let (_temporary, directory) = directory();
        let path = directory.path().join("worker.log");
        let mut bytes = vec![b'a'; 100_000];
        bytes.extend_from_slice(&[0xff, b'z']);
        fs::write(&path, bytes).expect("log");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("log mode");

        assert_eq!(
            read_private_tail(&directory, "worker.log", 2).expect("tail"),
            Some("�z".into())
        );
    }

    #[test]
    fn linked_tail_targets_fail_closed() {
        let (_temporary, directory) = directory();
        let outside = directory.path().parent().expect("parent").join("outside");
        fs::write(&outside, "secret").expect("outside");
        let path = directory.path().join("worker.log");
        symlink(&outside, &path).expect("symlink");

        assert!(read_private_tail(&directory, "worker.log", 32).is_err());
    }
}
