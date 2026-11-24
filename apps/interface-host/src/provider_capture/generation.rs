//! The locked-generation protocol of the Claude session lookup: exclusive
//! generation locks, pending-writer reclaim, seals, cleanup validation and
//! retirement, and pruning of receipts a checkpoint already covers.

use super::{
    CAPTURE_CLAIMS_SEALED, ClaudeTerminalSessionReceipt, Duration, GenerationEntry,
    MAX_CAPTURE_GENERATION_ENTRIES, MAX_RECEIPT_BYTES, Marker, Path, PrivateDirectory,
    RECEIPT_SEALED, RECEIPT_VERSION, RECEIPT_WRITER_LOCK, StoredCaptureOwner,
    StoredProcessIdentity, TerminalBackendError, TerminalLaunchSessionV1, WriterLock,
    acquire_root_lock_for_finalization, acquire_writer_lock_for_finalization, canonical_uuid_v4,
    classify, create_private_empty_file, fs, hook_process_identity, invalid_registry, read_bounded,
    read_marker, receipt_file_id, thread, transcript_session_id, valid_owner_path,
    validate_capture_owner,
};

const FINALIZATION_PENDING_ATTEMPTS: usize = 100;
const FINALIZATION_PENDING_RETRY_DELAY: Duration = Duration::from_millis(50);

pub(super) fn generation_receipts_are_checkpointed(
    generation: &PrivateDirectory,
    capture_token: &str,
    process: &StoredProcessIdentity,
    _expected_cwd: &Path,
    checkpoint: Option<&TerminalLaunchSessionV1>,
) -> Result<bool, TerminalBackendError> {
    validate_cleanup_entries(generation, capture_token)?;
    let mut effective = None::<(bool, u64, String, String)>;
    for entry in fs::read_dir(generation.path()).map_err(|_| invalid_registry())? {
        let name = entry
            .map_err(|_| invalid_registry())?
            .file_name()
            .into_string()
            .map_err(|_| invalid_registry())?;
        if receipt_file_id(&name).is_none() {
            continue;
        }
        let file = read_bounded(generation, &name, MAX_RECEIPT_BYTES)
            .map_err(|_| invalid_registry())?
            .ok_or_else(invalid_registry)?;
        let receipt: ClaudeTerminalSessionReceipt =
            serde_json::from_slice(file.bytes()).map_err(|_| invalid_registry())?;
        if receipt.parent_process_id != process.process_id {
            // A receipt from an inherited subagent hook cannot be proven by
            // the leader's checkpoint and must never be erased as if it were.
            return Ok(false);
        }
        if receipt
            .cwd
            .to_str()
            .is_none_or(|cwd| !valid_owner_path(cwd))
        {
            return Ok(false);
        }
        let transcript_id = transcript_session_id(&receipt.transcript_path)
            .ok_or_else(invalid_registry)?
            .to_owned();
        let transition = receipt.source != "startup";
        let candidate = (
            transition,
            receipt.captured_at_unix_nanos,
            name,
            transcript_id,
        );
        if effective.as_ref().is_none_or(|current| {
            (candidate.0 && !current.0)
                || (candidate.0 == current.0
                    && (candidate.1, candidate.2.as_str()) > (current.1, current.2.as_str()))
        }) {
            effective = Some(candidate);
        }
    }
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;
    let Some((_, _, _, transcript_id)) = effective else {
        return Ok(true);
    };
    Ok(checkpoint.and_then(|session| session.id.as_deref()) == Some(transcript_id.as_str()))
}

pub(super) fn prune_completed_receipts_except(
    generation: &PrivateDirectory,
    capture_token: &str,
    retained_name: Option<&str>,
) -> Result<(), TerminalBackendError> {
    validate_cleanup_entries(generation, capture_token)?;
    let mut changed = false;
    for entry in fs::read_dir(generation.path()).map_err(|_| invalid_registry())? {
        let entry = entry.map_err(|_| invalid_registry())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid_registry())?;
        if receipt_file_id(&name).is_none() || retained_name == Some(name.as_str()) {
            continue;
        }
        validate_cleanup_entry(generation, &name, capture_token)?;
        fs::remove_file(generation.path().join(name)).map_err(|_| invalid_registry())?;
        changed = true;
    }
    if changed {
        generation.sync().map_err(|_| invalid_registry())?;
    }
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())
}

pub(super) fn remove_generation_receipts(
    receipts_root: &Path,
    capture_token: &str,
) -> Result<(), TerminalBackendError> {
    let Some(locked) = lock_quiescent_generation(receipts_root, capture_token)? else {
        return Ok(());
    };
    clear_locked_generation(locked, capture_token)
}

pub(super) struct LockedGeneration {
    root: PrivateDirectory,
    root_gate: WriterLock,
    generation: PrivateDirectory,
    writer: WriterLock,
}

impl LockedGeneration {
    /// Runs `operation` on the locked generation, then releases its writer
    /// and the root gate. Both are always released; the operation's error
    /// wins, then the writer's, then the root's.
    pub(super) fn run<T>(
        self,
        operation: impl FnOnce(&PrivateDirectory, &WriterLock) -> Result<T, TerminalBackendError>,
    ) -> Result<T, TerminalBackendError> {
        let Self {
            root,
            root_gate,
            generation,
            writer,
        } = self;
        let result = operation(&generation, &writer);
        let release_writer = writer.release(&generation).map_err(|_| invalid_registry());
        let release_root = root_gate.release(&root).map_err(|_| invalid_registry());
        let value = result?;
        release_writer?;
        release_root?;
        Ok(value)
    }
}

pub(super) fn lock_quiescent_generation(
    receipts_root: &Path,
    capture_token: &str,
) -> Result<Option<LockedGeneration>, TerminalBackendError> {
    lock_generation_for_exclusive_update(
        receipts_root,
        capture_token,
        GenerationLockMode::FinalizeReceipts,
    )
}

pub(super) fn lock_claims_sealed_generation(
    receipts_root: &Path,
    capture_token: &str,
) -> Result<Option<LockedGeneration>, TerminalBackendError> {
    lock_generation_for_exclusive_update(
        receipts_root,
        capture_token,
        GenerationLockMode::SealClaims,
    )
}

pub(super) fn lock_reusable_generation(
    receipts_root: &Path,
    capture_token: &str,
) -> Result<Option<LockedGeneration>, TerminalBackendError> {
    lock_generation_for_exclusive_update(receipts_root, capture_token, GenerationLockMode::Reusable)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GenerationLockMode {
    Reusable,
    SealClaims,
    FinalizeReceipts,
}

pub(super) fn lock_generation_for_exclusive_update(
    receipts_root: &Path,
    capture_token: &str,
    mode: GenerationLockMode,
) -> Result<Option<LockedGeneration>, TerminalBackendError> {
    if !canonical_uuid_v4(capture_token) {
        return Err(invalid_registry());
    }
    for attempt in 0..FINALIZATION_PENDING_ATTEMPTS {
        let root = PrivateDirectory::open(receipts_root).map_err(|_| invalid_registry())?;
        let generation_path = root.path().join(capture_token);
        match fs::symlink_metadata(&generation_path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(invalid_registry()),
        }
        let generation = root
            .open_child(capture_token)
            .map_err(|_| invalid_registry())?;
        let writer =
            acquire_writer_lock_for_finalization(&generation).map_err(|_| invalid_registry())?;
        // Writers release the root registration gate before waiting on this
        // generation lock. Taking the locks in the opposite order here avoids
        // holding a global gate behind one slow terminal while still excluding
        // every new marker during the final scan and seal.
        let root_gate =
            acquire_root_lock_for_finalization(&root).map_err(|_| invalid_registry())?;
        if !reclaim_or_wait_for_pending_writes(&generation, capture_token)? {
            let update = match mode {
                GenerationLockMode::Reusable => {
                    if generation_has_empty_marker(&generation, CAPTURE_CLAIMS_SEALED)?
                        || generation_has_empty_marker(&generation, RECEIPT_SEALED)?
                    {
                        Err(invalid_registry())
                    } else {
                        Ok(())
                    }
                }
                GenerationLockMode::SealClaims => {
                    ensure_generation_marker(&generation, CAPTURE_CLAIMS_SEALED)
                }
                GenerationLockMode::FinalizeReceipts => {
                    ensure_generation_marker(&generation, CAPTURE_CLAIMS_SEALED)?;
                    ensure_generation_marker(&generation, RECEIPT_SEALED)
                }
            };
            if let Err(error) = update {
                writer
                    .release(&generation)
                    .map_err(|_| invalid_registry())?;
                root_gate.release(&root).map_err(|_| invalid_registry())?;
                return Err(error);
            }
            return Ok(Some(LockedGeneration {
                root,
                root_gate,
                generation,
                writer,
            }));
        }
        writer
            .release(&generation)
            .map_err(|_| invalid_registry())?;
        root_gate.release(&root).map_err(|_| invalid_registry())?;
        if attempt + 1 < FINALIZATION_PENDING_ATTEMPTS {
            thread::sleep(FINALIZATION_PENDING_RETRY_DELAY);
        }
    }
    Err(invalid_registry())
}

pub(super) fn reclaim_or_wait_for_pending_writes(
    generation: &PrivateDirectory,
    capture_token: &str,
) -> Result<bool, TerminalBackendError> {
    validate_cleanup_entries(generation, capture_token)?;
    let mut live_pending = false;
    let mut removed = false;
    for entry in fs::read_dir(generation.path()).map_err(|_| invalid_registry())? {
        let name = entry
            .map_err(|_| invalid_registry())?
            .file_name()
            .into_string()
            .map_err(|_| invalid_registry())?;
        match classify(&name) {
            GenerationEntry::ReceiptTemp | GenerationEntry::OwnerTemp => {
                // Acquiring the generation writer proves that no cooperating hook
                // can still own one of its atomic-write temporaries.
                fs::remove_file(generation.path().join(name)).map_err(|_| invalid_registry())?;
                removed = true;
            }
            GenerationEntry::Pending(owner) => {
                match hook_process_identity(owner.process_id).map_err(|_| invalid_registry())? {
                    Some(observed) if observed == owner => live_pending = true,
                    Some(_) | None => {
                        // PID reuse is stale too: the marker carries normalized lstart
                        // so a different process can never keep an interrupted hook
                        // alive indefinitely.
                        fs::remove_file(generation.path().join(name))
                            .map_err(|_| invalid_registry())?;
                        removed = true;
                    }
                }
            }
            GenerationEntry::Receipt
            | GenerationEntry::Owner
            | GenerationEntry::WriterLock
            | GenerationEntry::Seal
            | GenerationEntry::ProviderReceipt
            | GenerationEntry::Unknown => {}
        }
    }
    if removed {
        generation.sync().map_err(|_| invalid_registry())?;
    }
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;
    Ok(live_pending)
}

pub(super) fn generation_has_empty_marker(
    generation: &PrivateDirectory,
    name: &str,
) -> Result<bool, TerminalBackendError> {
    match read_marker(generation, name, 1).map_err(|_| invalid_registry())? {
        Marker::Empty => Ok(true),
        Marker::NotEmpty => Err(invalid_registry()),
        Marker::Absent => Ok(false),
    }
}

pub(super) fn ensure_generation_marker(
    generation: &PrivateDirectory,
    name: &str,
) -> Result<(), TerminalBackendError> {
    if generation_has_empty_marker(generation, name)? {
        return Ok(());
    }
    let seal = generation.path().join(name);
    create_private_empty_file(&seal).map_err(|_| invalid_registry())?;
    generation.sync().map_err(|_| invalid_registry())?;
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())
}

pub(super) fn clear_locked_generation(
    locked: LockedGeneration,
    capture_token: &str,
) -> Result<(), TerminalBackendError> {
    let LockedGeneration {
        root,
        root_gate,
        generation,
        writer,
    } = locked;
    let writer_path = writer.path().to_path_buf();
    validate_cleanup_entries(&generation, capture_token)?;
    for entry in fs::read_dir(generation.path()).map_err(|_| invalid_registry())? {
        let entry = entry.map_err(|_| invalid_registry())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid_registry())?;
        if name == RECEIPT_WRITER_LOCK || name == RECEIPT_SEALED {
            continue;
        }
        validate_cleanup_entry(&generation, &name, capture_token)?;
        fs::remove_file(generation.path().join(name)).map_err(|_| invalid_registry())?;
    }
    generation.sync().map_err(|_| invalid_registry())?;
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;
    root.assert_unchanged().map_err(|_| invalid_registry())?;
    writer
        .release(&generation)
        .map_err(|_| invalid_registry())?;
    // The root registration gate is still held and the final scan proved that
    // no registered hook helper remains. That is the narrow condition under
    // which removing this generation's otherwise-persistent lock inode cannot
    // split advisory-lock ownership: there is no waiter holding the old inode,
    // and no new writer can register before the directory is gone.
    fs::remove_file(writer_path).map_err(|_| invalid_registry())?;
    generation.sync().map_err(|_| invalid_registry())?;
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;
    let seal = generation.path().join(RECEIPT_SEALED);
    fs::remove_file(&seal).map_err(|_| invalid_registry())?;
    if fs::remove_dir(generation.path()).is_err() {
        // Preserve the retired state if an unexpected directory-removal
        // failure occurs after the seal unlink. A later bounded cleanup may
        // retry, but no hook may publish into this generation meanwhile.
        let _ = create_private_empty_file(&seal);
        let _ = generation.sync();
        return Err(invalid_registry());
    }
    root.sync().map_err(|_| invalid_registry())?;
    root.assert_unchanged().map_err(|_| invalid_registry())?;
    root_gate.release(&root).map_err(|_| invalid_registry())?;
    root.assert_unchanged().map_err(|_| invalid_registry())
}

pub(super) fn validate_cleanup_entries(
    generation: &PrivateDirectory,
    capture_token: &str,
) -> Result<(), TerminalBackendError> {
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;
    let mut entry_count = 0_usize;
    for entry in fs::read_dir(generation.path()).map_err(|_| invalid_registry())? {
        entry_count = entry_count.saturating_add(1);
        if entry_count > MAX_CAPTURE_GENERATION_ENTRIES {
            return Err(invalid_registry());
        }
        let entry = entry.map_err(|_| invalid_registry())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid_registry())?;
        validate_cleanup_entry(generation, &name, capture_token)?;
    }
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())
}

pub(super) fn validate_cleanup_entry(
    generation: &PrivateDirectory,
    name: &str,
    capture_token: &str,
) -> Result<(), TerminalBackendError> {
    let file = read_bounded(generation, name, MAX_RECEIPT_BYTES)
        .map_err(|_| invalid_registry())?
        .ok_or_else(invalid_registry)?;
    match classify(name) {
        // An interrupted atomic write may leave any bounded prefix here. The
        // held generation writer proves no cooperating helper is still using
        // it, so cleanup may reclaim it without interpreting its bytes. The
        // other providers' receipts are likewise never interpreted.
        GenerationEntry::ReceiptTemp
        | GenerationEntry::OwnerTemp
        | GenerationEntry::ProviderReceipt => Ok(()),
        GenerationEntry::Owner => {
            let owner: StoredCaptureOwner =
                serde_json::from_slice(file.bytes()).map_err(|_| invalid_registry())?;
            validate_capture_owner(&owner, capture_token)
        }
        GenerationEntry::Pending(_) | GenerationEntry::WriterLock | GenerationEntry::Seal => file
            .bytes()
            .is_empty()
            .then_some(())
            .ok_or_else(invalid_registry),
        GenerationEntry::Receipt => {
            let receipt: ClaudeTerminalSessionReceipt =
                serde_json::from_slice(file.bytes()).map_err(|_| invalid_registry())?;
            if receipt.version != RECEIPT_VERSION || receipt.capture_token != capture_token {
                return Err(invalid_registry());
            }
            Ok(())
        }
        GenerationEntry::Unknown => Err(invalid_registry()),
    }
}
