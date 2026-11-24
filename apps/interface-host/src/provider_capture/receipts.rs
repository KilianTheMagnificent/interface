//! Reading and verifying one generation's Claude receipts against the
//! pane process and the provider's transcript store.

use super::generation::lock_quiescent_generation;
use super::{
    ClaudeTerminalSessionReceipt, FileSnapshot, GenerationEntry, MAX_CAPTURE_GENERATION_ENTRIES,
    MAX_RECEIPT_BYTES, Marker, Path, PathBuf, PrivateDirectory, RECEIPT_VERSION,
    TerminalBackendError, TerminalProcessIdentity, TerminalProviderSession,
    TerminalProviderSessionTransition, canonical_uuid_v4, classify, fs, hook_process_identity,
    invalid_registry, read_bounded, read_capture_owner, read_marker,
    session_id_from_transcript_path, transcript_session_id, transition_for, valid_owner_path,
};

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ReceiptCandidate {
    pub(super) name: String,
    pub(super) snapshot: FileSnapshot,
    pub(super) reported_session_id: String,
    pub(super) transcript_id: String,
    pub(super) transcript_path: PathBuf,
    pub(super) cwd: PathBuf,
    pub(super) transition: TerminalProviderSessionTransition,
    pub(super) captured_at_unix_nanos: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ReceiptScan {
    pub(super) candidate: Option<ReceiptCandidate>,
    pub(super) pending_write: bool,
}

pub(super) fn read_final_verified_receipt(
    projects_root: &Path,
    receipts_root: &Path,
    capture_token: &str,
    process: &TerminalProcessIdentity,
    expected_cwd: Option<&Path>,
) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
    let Some(locked) = lock_quiescent_generation(receipts_root, capture_token)? else {
        return Ok(None);
    };
    locked.run(|_, _| {
        read_verified_receipt(
            projects_root,
            receipts_root,
            capture_token,
            process,
            None,
            expected_cwd,
        )
    })
}

pub(super) fn scan_receipts(
    generation: &PrivateDirectory,
    capture_token: &str,
    projects_root: &Path,
    process: &TerminalProcessIdentity,
) -> Result<ReceiptScan, TerminalBackendError> {
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;
    let mut latest_startup = None;
    let mut latest_transition = None;
    let mut pending_write = false;
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
        match classify(&name) {
            // Interrupted atomic-write prefixes and the other providers'
            // receipts: bounded, never interpreted or rejected.
            GenerationEntry::ReceiptTemp
            | GenerationEntry::OwnerTemp
            | GenerationEntry::ProviderReceipt => {
                read_bounded(generation, &name, MAX_RECEIPT_BYTES)
                    .map_err(|_| invalid_registry())?
                    .ok_or_else(invalid_registry)?;
                continue;
            }
            GenerationEntry::Owner => {
                // Reading the owner validates it against this capture token.
                read_capture_owner(generation, capture_token)?.ok_or_else(invalid_registry)?;
                continue;
            }
            GenerationEntry::Pending(owner) => {
                require_empty_marker(generation, &name, MAX_RECEIPT_BYTES)?;
                if hook_process_identity(owner.process_id).map_err(|_| invalid_registry())?
                    == Some(owner)
                {
                    pending_write = true;
                }
                continue;
            }
            GenerationEntry::WriterLock => {
                require_empty_marker(generation, &name, MAX_RECEIPT_BYTES)?;
                continue;
            }
            GenerationEntry::Seal => {
                require_empty_marker(generation, &name, 1)?;
                continue;
            }
            GenerationEntry::Unknown => return Err(invalid_registry()),
            GenerationEntry::Receipt => {}
        }
        let file = read_bounded(generation, &name, MAX_RECEIPT_BYTES)
            .map_err(|_| invalid_registry())?
            .ok_or_else(invalid_registry)?;
        let receipt: ClaudeTerminalSessionReceipt =
            serde_json::from_slice(file.bytes()).map_err(|_| invalid_registry())?;
        // Claude can inherit the hook settings into a forked/subagent process.
        // Its valid receipt belongs to that other process, not this PTY
        // generation; ignore it without poisoning the leader's receipts.
        if receipt.parent_process_id != process.process_id {
            continue;
        }
        let candidate = validate_receipt(
            name,
            file.snapshot().clone(),
            receipt,
            capture_token,
            projects_root,
        )?;
        let slot = if candidate.transition == TerminalProviderSessionTransition::Startup {
            &mut latest_startup
        } else {
            &mut latest_transition
        };
        if slot.as_ref().is_none_or(|current: &ReceiptCandidate| {
            (candidate.captured_at_unix_nanos, candidate.name.as_str())
                > (current.captured_at_unix_nanos, current.name.as_str())
        }) {
            *slot = Some(candidate);
        }
    }
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;
    Ok(ReceiptScan {
        candidate: latest_transition.or(latest_startup),
        pending_write,
    })
}

pub(super) fn read_verified_receipt(
    projects_root: &Path,
    receipts_root: &Path,
    capture_token: &str,
    process: &TerminalProcessIdentity,
    expected_reported_session_id: Option<&str>,
    _expected_cwd: Option<&Path>,
) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
    if !canonical_uuid_v4(capture_token) {
        return Err(invalid_registry());
    }
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
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;

    let first_scan = scan_receipts(&generation, capture_token, projects_root, process)?;
    root.assert_unchanged().map_err(|_| invalid_registry())?;
    if first_scan.pending_write {
        return Ok(None);
    }
    let Some(candidate) = first_scan.candidate else {
        return Ok(None);
    };
    if expected_reported_session_id
        .is_some_and(|expected| candidate.reported_session_id != expected)
    {
        return Ok(None);
    }

    let transcript_id =
        match session_id_from_transcript_path(projects_root, &candidate.transcript_path) {
            Ok(session_id) => session_id,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(invalid_registry()),
        };
    if transcript_id != candidate.transcript_id {
        return Err(invalid_registry());
    }
    let stable_receipt = read_bounded(&generation, &candidate.name, MAX_RECEIPT_BYTES)
        .map_err(|_| invalid_registry())?
        .ok_or_else(invalid_registry)?;
    if stable_receipt.snapshot() != &candidate.snapshot {
        return Err(invalid_registry());
    }
    let second_scan = scan_receipts(&generation, capture_token, projects_root, process)?;
    if second_scan.pending_write || second_scan.candidate.as_ref() != Some(&candidate) {
        return Ok(None);
    }
    generation
        .assert_unchanged()
        .map_err(|_| invalid_registry())?;
    root.assert_unchanged().map_err(|_| invalid_registry())?;
    Ok(Some(TerminalProviderSession {
        id: transcript_id,
        transition: candidate.transition,
    }))
}

pub(super) fn validate_receipt(
    name: String,
    snapshot: FileSnapshot,
    receipt: ClaudeTerminalSessionReceipt,
    capture_token: &str,
    projects_root: &Path,
) -> Result<ReceiptCandidate, TerminalBackendError> {
    let transition = transition_for(&receipt.source).ok_or_else(invalid_registry)?;
    let transcript_id = transcript_session_id(&receipt.transcript_path)
        .ok_or_else(invalid_registry)?
        .to_owned();
    let cwd = receipt.cwd.to_str().ok_or_else(invalid_registry)?;
    let expected_project = claude_project_dir_name(cwd);
    let transcript_project = receipt
        .transcript_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or_else(invalid_registry)?;
    // Claude can report a fresh invocation UUID during `/resume` while it
    // continues writing the original durable transcript. The exact safe
    // transcript filename is therefore the authority; the reported UUID is
    // retained and validated only as provider evidence, never as the binding.
    if receipt.version != RECEIPT_VERSION
        || receipt.capture_token != capture_token
        || !canonical_uuid_v4(&receipt.reported_session_id)
        || !receipt.transcript_path.is_absolute()
        || receipt.transcript_path.parent().and_then(Path::parent) != Some(projects_root)
        || transcript_project != expected_project
        || !valid_owner_path(cwd)
        || receipt.captured_at_unix_nanos == 0
    {
        return Err(invalid_registry());
    }
    Ok(ReceiptCandidate {
        name,
        snapshot,
        reported_session_id: receipt.reported_session_id,
        transcript_id,
        transcript_path: receipt.transcript_path,
        cwd: receipt.cwd,
        transition,
        captured_at_unix_nanos: receipt.captured_at_unix_nanos,
    })
}

pub(super) fn claude_project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// Refuses a must-be-empty marker that is gone or holds bytes.
pub(super) fn require_empty_marker(
    generation: &PrivateDirectory,
    name: &str,
    bound: usize,
) -> Result<(), TerminalBackendError> {
    match read_marker(generation, name, bound).map_err(|_| invalid_registry())? {
        Marker::Empty => Ok(()),
        Marker::Absent | Marker::NotEmpty => Err(invalid_registry()),
    }
}
