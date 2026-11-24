//! Private Claude `SessionStart` hook used by integrated terminals.
//!
//! Claude owns the durable conversation identifier. The renderer UUID passed
//! to `--session-id` is only a launch request, because Claude can adopt a
//! different provider ID during recovery. A legacy recovery picker likewise needs
//! the ID the user selected. This helper receives Claude's supported hook
//! payload on stdin, durably records it in a host-private generation directory,
//! and asks the interactive CLI to emit a transition wake-up as a reserved OSC
//! title on the originating PTY. The terminal service reads and validates that
//! receipt independently; PTY bytes never provide session authority.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use interface_common::uuid::canonical_uuid_v4;
use interface_storage::{PrivateDirectory, read_bounded};
use interface_terminal::{CLAUDE_ACTIVITY_TITLE_PREFIX, CLAUDE_SESSION_BINDING_TITLE_PREFIX};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::capture_generation::{
    GenerationEntry, MAX_CAPTURE_GENERATION_ENTRIES, MAX_CAPTURE_OWNER_BYTES, MAX_RECEIPT_BYTES,
    Marker, RECEIPT_TEMP_PREFIX, RECEIPT_TEMP_SUFFIX, acquire_root_lock, acquire_writer_lock,
    classify, create_new_private_file, create_private_empty_file, generation_is_sealed,
    pending_marker_name, read_marker, receipt_file_id, sync_private_file,
};
#[cfg(test)]
use crate::capture_hook::parent_process_id_of;
use crate::capture_hook::{current_hook_process_identity, is_session_source};
pub(crate) use crate::capture_hook::{hook_process_identity, parent_process_id};

/// `SessionStart` receipts are small; a larger payload is never a receipt.
const MAX_HOOK_INPUT_BYTES: u64 = 16 * 1_024;
/// Turn-boundary payloads carry the whole prompt, tool input or final answer,
/// so they are streamed for their event name instead of being refused at the
/// receipt bound: a dropped `Stop` would leave the pane working forever.
const MAX_TURN_HOOK_INPUT_BYTES: u64 = 64 * 1_024 * 1_024;
pub(crate) const MAX_RECEIPTS_PER_GENERATION: usize = 256;
pub(crate) const RECEIPT_VERSION: u32 = 1;

#[derive(Deserialize)]
struct HookEventInput {
    hook_event_name: String,
    /// Kept loose: an unexpected shape must still end the turn as idle.
    #[serde(default)]
    background_tasks: serde_json::Value,
}

/// A `Stop` only ends Claude's foreground turn. Background agents and
/// workflows it started keep working, and their completion starts the turn
/// whose `Stop` finally reports idle. Background shells do not count: servers
/// and watchers may never exit, and Claude calls the turn done beside them.
pub(crate) fn stop_activity(background_tasks: &serde_json::Value) -> &'static str {
    let agents_running = background_tasks.as_array().is_some_and(|tasks| {
        tasks.iter().any(|task| {
            matches!(task["type"].as_str(), Some("subagent" | "workflow"))
                && task["status"] == "running"
        })
    });
    if agents_running { "working" } else { "idle" }
}

#[derive(Deserialize)]
struct SessionStartInput {
    session_id: String,
    transcript_path: String,
    cwd: String,
    hook_event_name: String,
    source: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ClaudeTerminalSessionReceipt {
    pub(crate) version: u32,
    pub(crate) capture_token: String,
    pub(crate) parent_process_id: u32,
    pub(crate) reported_session_id: String,
    pub(crate) transcript_path: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) source: String,
    pub(crate) captured_at_unix_nanos: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookOutput {
    terminal_sequence: String,
}

/// Read one bounded hook payload and emit a validated terminal sequence.
///
/// Invalid input is a silent success: session identity capture is a recovery
/// aid, never permission to interrupt Claude startup or leak hook input into
/// the transcript. The caller intentionally ignores I/O errors for the same
/// fail-open startup behavior.
///
/// # Errors
///
/// Returns an error when the input cannot be read or the validated JSON
/// response cannot be written.
pub fn run_claude_terminal_session_hook(
    capture_token: &str,
    receipts_root: &Path,
    mut input: impl Read,
    mut output: impl Write,
) -> io::Result<()> {
    if !canonical_uuid_v4(capture_token) || !receipts_root.is_absolute() {
        return Ok(());
    }
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(MAX_HOOK_INPUT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    let oversized = bytes.len() as u64 > MAX_HOOK_INPUT_BYTES;
    // The same command is installed on the turn boundaries. Those carry no
    // session authority: they only tell the pane's activity dot that a turn
    // started or ended, which Claude's static title never does.
    // PreToolUse also reports work: a turn Claude starts itself (a background
    // task or monitor finishing) never passes through UserPromptSubmit.
    // PermissionRequest fires the moment Claude shows a permission dialog or
    // an AskUserQuestion/plan-approval prompt (Claude Code 2.1.291; its
    // permission_prompt Notification lags about six seconds), so the pane says
    // it needs you until the answer, the next tool call or the turn's end.
    // A large one is parsed from the stream; serde skips the prompt, tool
    // input and answer text without keeping them.
    let event = if oversized {
        let remaining = MAX_TURN_HOOK_INPUT_BYTES.saturating_sub(bytes.len() as u64);
        // Buffered: serde reads a plain reader one byte per call.
        serde_json::from_reader::<_, HookEventInput>(BufReader::with_capacity(
            64 * 1024,
            bytes.as_slice().chain(input.take(remaining)),
        ))
        .ok()
    } else {
        serde_json::from_slice::<HookEventInput>(&bytes).ok()
    };
    let activity = event.and_then(|event| match event.hook_event_name.as_str() {
        "UserPromptSubmit" | "PreToolUse" => Some("working"),
        "PermissionRequest" => Some("waiting"),
        "Stop" => Some(stop_activity(&event.background_tasks)),
        _ => None,
    });
    if let Some(state) = activity {
        let response = HookOutput {
            terminal_sequence: format!("\u{1b}]2;{CLAUDE_ACTIVITY_TITLE_PREFIX}{state}\u{7}"),
        };
        return serde_json::to_writer(&mut output, &response).map_err(io::Error::other);
    }
    if oversized {
        return Ok(());
    }
    let Ok(payload) = serde_json::from_slice::<SessionStartInput>(&bytes) else {
        return Ok(());
    };
    let transcript_path = PathBuf::from(&payload.transcript_path);
    if transcript_session_id(&transcript_path).is_none() {
        return Ok(());
    }
    if payload.hook_event_name != "SessionStart"
        || !is_session_source(&payload.source)
        || !Path::new(&payload.cwd).is_absolute()
        || !Path::new(&payload.transcript_path).is_absolute()
        || !canonical_uuid_v4(&payload.session_id)
    {
        return Ok(());
    }
    let receipt = ClaudeTerminalSessionReceipt {
        version: RECEIPT_VERSION,
        capture_token: capture_token.to_owned(),
        parent_process_id: 0,
        reported_session_id: payload.session_id,
        transcript_path,
        cwd: PathBuf::from(payload.cwd),
        source: payload.source,
        // The writer serializes receipts and replaces this wall-clock sample
        // with a generation-monotonic value before committing it.
        captured_at_unix_nanos: 0,
    };
    if !write_receipt(receipts_root, capture_token, receipt)? {
        return Ok(());
    }
    let response = HookOutput {
        terminal_sequence: format!(
            "\u{1b}]2;{CLAUDE_SESSION_BINDING_TITLE_PREFIX}{capture_token}:receipt\u{7}"
        ),
    };
    serde_json::to_writer(&mut output, &response).map_err(io::Error::other)
}

pub(crate) fn transcript_session_id(path: &Path) -> Option<&str> {
    let file_name = path.file_name()?.to_str()?;
    let session_id = file_name.strip_suffix(".jsonl")?;
    canonical_uuid_v4(session_id).then_some(session_id)
}

#[allow(clippy::too_many_lines)] // One locked receipt commit validates and preserves every crash boundary.
fn write_receipt(
    receipts_root: &Path,
    capture_token: &str,
    mut receipt: ClaudeTerminalSessionReceipt,
) -> io::Result<bool> {
    // Sample the helper identity before publishing its durable pending marker.
    // Once that marker exists, finalization can distinguish a live delayed
    // writer from a crash remnant without trusting wall-clock age.
    let hook_process = current_hook_process_identity()?;
    let root = PrivateDirectory::open(receipts_root).map_err(io::Error::other)?;
    // This root-scoped gate is held only while a writer registers its pending
    // marker. Finalization holds the same gate while sealing a generation, so
    // it cannot miss a helper that was scheduled between its scan and cleanup.
    let registration = acquire_root_lock(&root)?;
    // The terminal core durably creates and owns this generation before the
    // provider can execute. A hook may append to that exact owner only; it may
    // never mint unbounded root entries from command-line input.
    let generation = root.open_child(capture_token).map_err(io::Error::other)?;
    if generation_is_sealed(&generation)? {
        registration.release(&root).map_err(io::Error::other)?;
        return Ok(false);
    }
    let pending_name = pending_marker_name(&hook_process, Uuid::new_v4());
    let pending_path = generation.path().join(&pending_name);
    create_private_empty_file(&pending_path)?;
    generation.sync().map_err(io::Error::other)?;
    registration.release(&root).map_err(io::Error::other)?;
    // The PPID sample intentionally follows durable marker publication. If
    // the PTY leader is already gone, this helper cannot later manufacture a
    // valid leader receipt; finalization will either wait for it or reclaim
    // the marker using the helper's own pid+lstart identity.
    receipt.parent_process_id = match parent_process_id() {
        Ok(parent) => parent,
        Err(error) => {
            let _ = remove_pending_marker(&generation, &pending_path);
            return Err(error);
        }
    };
    // Capture tokens are unique per terminal process generation. Locking the
    // generation lets many restored Claude panes publish concurrently while
    // still serializing duplicate transition hooks for one process.
    let writer = match acquire_writer_lock(&generation) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = remove_pending_marker(&generation, &pending_path);
            return Err(error);
        }
    };
    // Finalization acquires the generation lock before its brief root-gate
    // seal. A writer that registered first but lost that race must re-check
    // here and decline rather than publishing behind the seal.
    if generation_is_sealed(&generation)? {
        let cleanup = remove_pending_marker(&generation, &pending_path);
        let release = writer.release(&generation).map_err(io::Error::other);
        cleanup?;
        release?;
        return Ok(false);
    }
    let result = (|| -> io::Result<bool> {
        let prepared = prepare_generation(&generation, capture_token)?;
        let existing = prepared.lineages.get(&receipt.parent_process_id);
        if existing.is_none() && prepared.lineages.len() >= MAX_RECEIPTS_PER_GENERATION {
            return Ok(false);
        }
        if receipt.source == "startup"
            && existing.is_some_and(|lineage| lineage.transition.is_some())
        {
            return Ok(false);
        }
        let wall_clock = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
            .unwrap_or(0);
        let next_generation_timestamp =
            prepared.maximum_timestamp.checked_add(1).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Claude terminal session receipt timestamp overflowed",
                )
            })?;
        receipt.captured_at_unix_nanos = wall_clock.max(next_generation_timestamp);
        if receipt.captured_at_unix_nanos == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Claude terminal session receipt timestamp was unavailable",
            ));
        }
        let bytes = serde_json::to_vec(&receipt).map_err(io::Error::other)?;
        if bytes.is_empty() || bytes.len() > MAX_RECEIPT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Claude terminal session receipt exceeded its bound",
            ));
        }
        let event_id = Uuid::new_v4();
        let name = format!("{event_id}.json");
        let temporary_name = format!(
            "{RECEIPT_TEMP_PREFIX}{event_id}-{}{RECEIPT_TEMP_SUFFIX}",
            Uuid::new_v4()
        );
        let path = generation.path().join(&name);
        let temporary_path = generation.path().join(&temporary_name);
        let mut file = create_new_private_file(&temporary_path)?;
        let write_result = (|| {
            file.write_all(&bytes)?;
            sync_private_file(&file, bytes.len(), "receipt")?;
            drop(file);
            fs::rename(&temporary_path, &path)?;
            generation.sync().map_err(io::Error::other)?;
            let installed = read_bounded(&generation, &name, MAX_RECEIPT_BYTES)
                .map_err(io::Error::other)?
                .ok_or_else(|| io::Error::other("Claude terminal session receipt disappeared"))?;
            if installed.bytes() != bytes {
                return Err(io::Error::other(
                    "Claude terminal session receipt changed after installation",
                ));
            }
            generation.assert_unchanged().map_err(io::Error::other)?;
            root.assert_unchanged().map_err(io::Error::other)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        write_result?;
        prune_superseded_receipts(&generation, capture_token, receipt.parent_process_id, &name)?;
        Ok(true)
    })();
    let pending_cleanup = remove_pending_marker(&generation, &pending_path);
    let release = writer.release(&generation).map_err(io::Error::other);
    let committed = result?;
    pending_cleanup?;
    release?;
    Ok(committed)
}

fn remove_pending_marker(generation: &PrivateDirectory, path: &Path) -> io::Result<()> {
    fs::remove_file(path)?;
    generation.sync().map_err(io::Error::other)
}

#[derive(Clone)]
struct RetainedReceipt {
    name: String,
    captured_at_unix_nanos: u64,
}

#[derive(Default)]
struct ParentLineage {
    startup: Option<RetainedReceipt>,
    transition: Option<RetainedReceipt>,
}

impl ParentLineage {
    fn effective(&self) -> Option<&RetainedReceipt> {
        self.transition.as_ref().or(self.startup.as_ref())
    }
}

struct PreparedGeneration {
    maximum_timestamp: u64,
    lineages: BTreeMap<u32, ParentLineage>,
}

#[allow(clippy::too_many_lines)] // One bounded scan classifies every permitted generation entry.
fn prepare_generation(
    generation: &PrivateDirectory,
    capture_token: &str,
) -> io::Result<PreparedGeneration> {
    generation.assert_unchanged().map_err(io::Error::other)?;
    let mut maximum_timestamp = 0_u64;
    let mut lineages = BTreeMap::<u32, ParentLineage>::new();
    let mut temporary_names = Vec::new();
    let mut entry_count = 0_usize;
    for entry in fs::read_dir(generation.path())? {
        entry_count = entry_count.saturating_add(1);
        if entry_count > MAX_CAPTURE_GENERATION_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Claude receipt generation exceeded its entry bound",
            ));
        }
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| io::Error::other("Claude receipt name was not UTF-8"))?;
        match classify(&name) {
            GenerationEntry::Receipt => {
                let file = read_bounded(generation, &name, MAX_RECEIPT_BYTES)
                    .map_err(io::Error::other)?
                    .ok_or_else(|| io::Error::other("Claude receipt disappeared"))?;
                let receipt: ClaudeTerminalSessionReceipt =
                    serde_json::from_slice(file.bytes()).map_err(io::Error::other)?;
                validate_existing_receipt(&receipt, capture_token)?;
                maximum_timestamp = maximum_timestamp.max(receipt.captured_at_unix_nanos);
                let retained = RetainedReceipt {
                    name,
                    captured_at_unix_nanos: receipt.captured_at_unix_nanos,
                };
                let lineage = lineages.entry(receipt.parent_process_id).or_default();
                let slot = if receipt.source == "startup" {
                    &mut lineage.startup
                } else {
                    &mut lineage.transition
                };
                if slot.as_ref().is_none_or(|current: &RetainedReceipt| {
                    (retained.captured_at_unix_nanos, retained.name.as_str())
                        > (current.captured_at_unix_nanos, current.name.as_str())
                }) {
                    *slot = Some(retained);
                }
            }
            GenerationEntry::ReceiptTemp | GenerationEntry::OwnerTemp => {
                read_bounded(generation, &name, MAX_RECEIPT_BYTES)
                    .map_err(io::Error::other)?
                    .ok_or_else(|| io::Error::other("Claude receipt temporary disappeared"))?;
                temporary_names.push(name);
            }
            GenerationEntry::Owner => {
                read_bounded(generation, &name, MAX_CAPTURE_OWNER_BYTES)
                    .map_err(io::Error::other)?
                    .ok_or_else(|| io::Error::other("Claude capture owner disappeared"))?;
            }
            GenerationEntry::Pending(_) => {
                require_empty_marker(generation, &name, MAX_RECEIPT_BYTES, "pending marker")?;
            }
            GenerationEntry::WriterLock => {
                require_empty_marker(generation, &name, MAX_RECEIPT_BYTES, "writer lock")?;
            }
            GenerationEntry::Seal => require_empty_marker(generation, &name, 1, "seal")?,
            GenerationEntry::ProviderReceipt => {
                // The readers treat the other providers' entries as opaque;
                // so does the writer: it bounds them but neither interprets
                // nor prunes them.
                read_bounded(generation, &name, MAX_RECEIPT_BYTES)
                    .map_err(io::Error::other)?
                    .ok_or_else(|| io::Error::other("provider receipt disappeared"))?;
            }
            GenerationEntry::Unknown => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Claude receipt generation contained an unknown entry",
                ));
            }
        }
    }
    generation.assert_unchanged().map_err(io::Error::other)?;

    if lineages.len() > MAX_RECEIPTS_PER_GENERATION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Claude receipt generation exceeded its lineage bound",
        ));
    }

    // A receipt may only supersede another receipt from the same direct
    // provider parent. Keeping one effective record per parent prevents
    // inherited subagent hooks from evicting the PTY leader's binding while
    // retaining a hard bound on independently effective lineages.
    let retained_names = lineages
        .values()
        .filter_map(ParentLineage::effective)
        .map(|receipt| receipt.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut changed = false;
    for entry in fs::read_dir(generation.path())? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| io::Error::other("Claude receipt name was not UTF-8"))?;
        let remove = receipt_file_id(&name).is_some() && !retained_names.contains(name.as_str())
            || temporary_names.iter().any(|temporary| temporary == &name);
        if !remove {
            continue;
        }
        // The writer lock proves no cooperating hook is publishing this temp.
        // Re-read it through the safe-file primitive before deleting by name.
        read_bounded(generation, &name, MAX_RECEIPT_BYTES)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("Claude receipt disappeared while pruning"))?;
        fs::remove_file(generation.path().join(&name))?;
        changed = true;
    }
    if changed {
        generation.sync().map_err(io::Error::other)?;
    }
    generation.assert_unchanged().map_err(io::Error::other)?;
    Ok(PreparedGeneration {
        maximum_timestamp,
        lineages,
    })
}

/// Refuses a must-be-empty marker that is gone or holds bytes.
fn require_empty_marker(
    generation: &PrivateDirectory,
    name: &str,
    bound: usize,
    what: &str,
) -> io::Result<()> {
    match read_marker(generation, name, bound).map_err(io::Error::other)? {
        Marker::Empty => Ok(()),
        Marker::Absent => Err(io::Error::other(format!(
            "Claude receipt {what} disappeared"
        ))),
        Marker::NotEmpty => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Claude receipt {what} was not empty"),
        )),
    }
}

fn prune_superseded_receipts(
    generation: &PrivateDirectory,
    capture_token: &str,
    parent_process_id: u32,
    installed_name: &str,
) -> io::Result<()> {
    let prepared = prepare_generation(generation, capture_token)?;
    let retained = prepared
        .lineages
        .get(&parent_process_id)
        .and_then(ParentLineage::effective)
        .map(|receipt| receipt.name.as_str());
    if retained != Some(installed_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Claude receipt install did not become its parent lineage head",
        ));
    }
    Ok(())
}

fn validate_existing_receipt(
    receipt: &ClaudeTerminalSessionReceipt,
    capture_token: &str,
) -> io::Result<()> {
    if receipt.version != RECEIPT_VERSION
        || receipt.capture_token != capture_token
        || receipt.parent_process_id == 0
        || !canonical_uuid_v4(&receipt.reported_session_id)
        || transcript_session_id(&receipt.transcript_path).is_none()
        || !receipt.transcript_path.is_absolute()
        || !receipt.cwd.is_absolute()
        || !is_session_source(&receipt.source)
        || receipt.captured_at_unix_nanos == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Claude receipt generation contained malformed data",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "claude_terminal_hook_tests.rs"]
mod tests;
