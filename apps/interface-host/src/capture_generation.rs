//! The on-disk protocol of a provider capture generation, shared by the
//! Claude and Codex `SessionStart` hooks and the host's session lookup.
//!
//! A capture generation is one host-private directory per terminal process
//! generation, named by its capture token under the receipts root. The
//! terminal core creates it and its owner record before the provider runs;
//! hooks may only append to it. This module owns what may live there — the
//! entry names, their bounds and [`classify`] — together with the root and
//! writer locks, the pending-writer marker codec and owner-only file
//! creation. Every directory walker matches on [`GenerationEntry`], so a new
//! entry kind is added here once.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::thread;
use std::time::Duration;

use interface_common::uuid::canonical_uuid_v4;
use interface_storage::{
    PrivateDirectory, StorageError, WriterLock, is_atomic_temp_name_for, read_bounded,
};
use uuid::Uuid;

use crate::opencode_terminal_session::is_opencode_receipt_entry_name;

/// The Codex hook's receipt file in a shared generation.
pub(crate) const CODEX_RECEIPT_FILE: &str = ".interface-codex-receipts.json";

/// The Codex receipt file or an interrupted atomic write of it.
pub(crate) fn is_codex_receipt_entry_name(name: &str) -> bool {
    name == CODEX_RECEIPT_FILE || is_atomic_temp_name_for(name, CODEX_RECEIPT_FILE)
}

pub(crate) const MAX_RECEIPT_BYTES: usize = 20 * 1_024;
pub(crate) const MAX_CAPTURE_GENERATIONS: usize = 256;
pub(crate) const MAX_CAPTURE_GENERATION_ENTRIES: usize = 1_024;
pub(crate) const MAX_CAPTURE_OWNER_BYTES: usize = 8 * 1_024;
pub(crate) const CAPTURE_OWNER_FILE: &str = ".interface-claude-capture-owner.json";
pub(crate) const RECEIPT_TEMP_PREFIX: &str = ".interface-claude-receipt-";
pub(crate) const RECEIPT_TEMP_SUFFIX: &str = ".tmp";
pub(crate) const RECEIPT_PENDING_PREFIX: &str = ".interface-claude-pending-v1-";
pub(crate) const RECEIPT_PENDING_SUFFIX: &str = ".lock";
pub(crate) const RECEIPT_ROOT_LOCK: &str = ".interface-claude-receipts-root.lock";
pub(crate) const CAPTURE_CLAIMS_SEALED: &str = ".interface-claude-capture-claims-sealed";
pub(crate) const RECEIPT_SEALED: &str = ".interface-claude-receipts-sealed";
pub(crate) const RECEIPT_WRITER_LOCK: &str = ".interface-claude-receipts.lock";
const RECEIPT_LOCK_ATTEMPTS: usize = 400;
const RECEIPT_FINALIZATION_LOCK_ATTEMPTS: usize = 700;
const RECEIPT_LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);

/// One entry a capture generation may hold, by its name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GenerationEntry {
    /// `<uuid>.json`: one Claude `SessionStart` receipt.
    Receipt,
    /// An interrupted write of a Claude receipt.
    ReceiptTemp,
    /// The host's capture owner record.
    Owner,
    /// An interrupted atomic write of the owner record.
    OwnerTemp,
    /// A registered hook writer's empty marker, naming its process.
    Pending(HookProcessIdentity),
    /// The empty generation writer lock.
    WriterLock,
    /// The empty capture-claims or receipts seal.
    Seal,
    /// The Codex receipt file or its atomic write, or the `OpenCode`
    /// observation. A manual shell pane shares one generation between its
    /// Claude and Codex wrappers, and `OpenCode` observes into it too; the
    /// Claude writer and readers bound these entries but never interpret
    /// or prune them.
    ProviderReceipt,
    /// Anything else, which every walker refuses.
    Unknown,
}

/// Classifies one generation entry name. The kinds are disjoint, so the
/// order of the checks never decides a name.
pub(crate) fn classify(name: &str) -> GenerationEntry {
    if receipt_file_id(name).is_some() {
        GenerationEntry::Receipt
    } else if is_receipt_temp_name(name) {
        GenerationEntry::ReceiptTemp
    } else if name == CAPTURE_OWNER_FILE {
        GenerationEntry::Owner
    } else if is_atomic_temp_name_for(name, CAPTURE_OWNER_FILE) {
        GenerationEntry::OwnerTemp
    } else if let Some(identity) = pending_marker_identity(name) {
        GenerationEntry::Pending(identity)
    } else if name == RECEIPT_WRITER_LOCK {
        GenerationEntry::WriterLock
    } else if name == CAPTURE_CLAIMS_SEALED || name == RECEIPT_SEALED {
        GenerationEntry::Seal
    } else if is_codex_receipt_entry_name(name)
        || is_opencode_receipt_entry_name(name)
        || name == crate::provider_capture::native::FILE
        || is_atomic_temp_name_for(name, crate::provider_capture::native::FILE)
    {
        GenerationEntry::ProviderReceipt
    } else {
        GenerationEntry::Unknown
    }
}

/// What reading one must-be-empty marker found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Marker {
    Absent,
    Empty,
    NotEmpty,
}

/// Reads the marker `name` within `bound` bytes.
///
/// # Errors
///
/// Returns the storage error when the entry cannot be read safely.
pub(crate) fn read_marker(
    generation: &PrivateDirectory,
    name: &str,
    bound: usize,
) -> Result<Marker, StorageError> {
    Ok(match read_bounded(generation, name, bound)? {
        None => Marker::Absent,
        Some(file) if file.bytes().is_empty() => Marker::Empty,
        Some(_) => Marker::NotEmpty,
    })
}

/// Whether finalization has sealed this generation against new receipts.
pub(crate) fn generation_is_sealed(generation: &PrivateDirectory) -> io::Result<bool> {
    match read_marker(generation, RECEIPT_SEALED, 1).map_err(io::Error::other)? {
        Marker::Empty => Ok(true),
        Marker::NotEmpty => Err(io::Error::other(
            "Claude terminal session receipt seal was malformed",
        )),
        Marker::Absent => Ok(false),
    }
}

/// Creates `path` as a new owner-only file, refusing a symlink or any
/// existing entry.
pub(crate) fn create_new_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

/// Syncs a file made by [`create_new_private_file`] and proves it is still
/// one owner-only regular file of `len` bytes. `what` names it in the
/// refusal.
pub(crate) fn sync_private_file(file: &File, len: usize, what: &str) -> io::Result<()> {
    file.sync_all()?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.len() != len as u64 {
        return Err(io::Error::other(format!(
            "Claude terminal session {what} was unsafe"
        )));
    }
    if metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "Claude terminal session {what} permissions were unsafe"
        )));
    }
    Ok(())
}

/// Creates one empty marker: a pending-writer marker or a seal.
pub(crate) fn create_private_empty_file(path: &Path) -> io::Result<()> {
    let file = create_new_private_file(path)?;
    sync_private_file(&file, 0, "pending marker")
}

pub(crate) fn acquire_root_lock(root: &PrivateDirectory) -> io::Result<WriterLock> {
    acquire_named_writer_lock(root, RECEIPT_ROOT_LOCK, RECEIPT_LOCK_ATTEMPTS)
}

pub(crate) fn acquire_root_lock_for_finalization(
    root: &PrivateDirectory,
) -> io::Result<WriterLock> {
    acquire_named_writer_lock(root, RECEIPT_ROOT_LOCK, RECEIPT_FINALIZATION_LOCK_ATTEMPTS)
}

pub(crate) fn acquire_writer_lock(generation: &PrivateDirectory) -> io::Result<WriterLock> {
    acquire_named_writer_lock(generation, RECEIPT_WRITER_LOCK, RECEIPT_LOCK_ATTEMPTS)
}

pub(crate) fn acquire_writer_lock_for_finalization(
    generation: &PrivateDirectory,
) -> io::Result<WriterLock> {
    acquire_named_writer_lock(
        generation,
        RECEIPT_WRITER_LOCK,
        RECEIPT_FINALIZATION_LOCK_ATTEMPTS,
    )
}

fn acquire_named_writer_lock(
    directory: &PrivateDirectory,
    name: &str,
    attempts: usize,
) -> io::Result<WriterLock> {
    for attempt in 0..attempts {
        match WriterLock::acquire(directory, name) {
            Ok(writer) => return Ok(writer),
            Err(StorageError::LockHeld { .. }) if attempt + 1 < attempts => {
                thread::sleep(RECEIPT_LOCK_RETRY_DELAY);
            }
            Err(error) => return Err(io::Error::other(error)),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "Claude terminal session receipt writer remained busy",
    ))
}

/// A hook process, as its pending marker names it: pid plus normalized
/// `lstart`, so a reused pid never keeps an interrupted hook alive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HookProcessIdentity {
    pub(crate) process_id: u32,
    pub(crate) process_start: String,
}

pub(crate) fn receipt_file_id(name: &str) -> Option<&str> {
    let id = name.strip_suffix(".json")?;
    canonical_uuid_v4(id).then_some(id)
}

fn is_receipt_temp_name(name: &str) -> bool {
    let Some(middle) = name
        .strip_prefix(RECEIPT_TEMP_PREFIX)
        .and_then(|value| value.strip_suffix(RECEIPT_TEMP_SUFFIX))
    else {
        return false;
    };
    middle.len() == 73
        && middle.as_bytes().get(36) == Some(&b'-')
        && middle.get(..36).is_some_and(canonical_uuid_v4)
        && middle.get(37..).is_some_and(canonical_uuid_v4)
}

pub(crate) fn pending_marker_name(identity: &HookProcessIdentity, nonce: Uuid) -> String {
    format!(
        "{RECEIPT_PENDING_PREFIX}{}-{}-{}{RECEIPT_PENDING_SUFFIX}",
        identity.process_id,
        hex_encode(identity.process_start.as_bytes()),
        nonce.hyphenated()
    )
}

fn pending_marker_identity(name: &str) -> Option<HookProcessIdentity> {
    let middle = name
        .strip_prefix(RECEIPT_PENDING_PREFIX)?
        .strip_suffix(RECEIPT_PENDING_SUFFIX)?;
    if middle.len() < 38 {
        return None;
    }
    let nonce_at = middle.len().checked_sub(36)?;
    if nonce_at == 0 || middle.as_bytes().get(nonce_at - 1) != Some(&b'-') {
        return None;
    }
    let nonce = middle.get(nonce_at..)?;
    if !canonical_uuid_v4(nonce) {
        return None;
    }
    let (process_id, encoded_start) = middle.get(..nonce_at - 1)?.split_once('-')?;
    let process_id = process_id.parse::<u32>().ok().filter(|value| *value > 0)?;
    let process_start = String::from_utf8(hex_decode(encoded_start)?).ok()?;
    if process_start.is_empty()
        || process_start.len() > 128
        || !process_start.is_ascii()
        || process_start.chars().any(char::is_control)
        || process_start
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            != process_start
    {
        return None;
    }
    Some(HookProcessIdentity {
        process_id,
        process_start,
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.is_empty() || value.len() > 256 || value.len() % 2 != 0 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some(hex_nibble(pair[0])? << 4 | hex_nibble(pair[1])?))
        .collect()
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use interface_storage::atomic_temp_digest_prefix;
    use interface_terminal::OPENCODE_SESSION_RECEIPT_FILE;

    use super::*;

    const ID: &str = "11111111-1111-4111-8111-111111111111";
    const NONCE: &str = "22222222-2222-4222-8222-222222222222";

    fn atomic_temp(target: &str) -> String {
        format!(
            ".interface-atomic-{}-{NONCE}.tmp",
            atomic_temp_digest_prefix(target)
        )
    }

    #[test]
    fn every_generation_entry_name_has_exactly_one_kind() {
        let identity = HookProcessIdentity {
            process_id: 42,
            process_start: "Sat Sep 27 11:00:00 2026".to_owned(),
        };
        let pending = pending_marker_name(&identity, Uuid::new_v4());
        assert_eq!(classify(&format!("{ID}.json")), GenerationEntry::Receipt);
        assert_eq!(
            classify(&format!(
                "{RECEIPT_TEMP_PREFIX}{ID}-{NONCE}{RECEIPT_TEMP_SUFFIX}"
            )),
            GenerationEntry::ReceiptTemp
        );
        assert_eq!(classify(CAPTURE_OWNER_FILE), GenerationEntry::Owner);
        assert_eq!(
            classify(&atomic_temp(CAPTURE_OWNER_FILE)),
            GenerationEntry::OwnerTemp
        );
        assert_eq!(classify(&pending), GenerationEntry::Pending(identity));
        assert_eq!(classify(RECEIPT_WRITER_LOCK), GenerationEntry::WriterLock);
        assert_eq!(classify(CAPTURE_CLAIMS_SEALED), GenerationEntry::Seal);
        assert_eq!(classify(RECEIPT_SEALED), GenerationEntry::Seal);
        for provider in [
            CODEX_RECEIPT_FILE.to_owned(),
            atomic_temp(CODEX_RECEIPT_FILE),
            OPENCODE_SESSION_RECEIPT_FILE.to_owned(),
            format!("{OPENCODE_SESSION_RECEIPT_FILE}.pending"),
        ] {
            assert_eq!(classify(&provider), GenerationEntry::ProviderReceipt);
        }
        for unknown in [
            // Not a v4 receipt id, a malformed pending marker, the root lock
            // (which lives above generations) and a stray file.
            "11111111-1111-1111-8111-111111111111.json".to_owned(),
            pending.replace("-v1-42-", "-v1-0-"),
            RECEIPT_ROOT_LOCK.to_owned(),
            "notes.txt".to_owned(),
        ] {
            assert_eq!(classify(&unknown), GenerationEntry::Unknown, "{unknown}");
        }
    }
}
