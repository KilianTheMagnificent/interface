//! Private Codex `SessionStart` hook used by integrated terminals.
//!
//! Codex mints its own thread id and has no launch flag to be given one, so
//! the only exact handle is the `session_id` its `SessionStart` hook reports
//! once the first prompt creates the thread. Interface injects that hook per
//! pane as an inline `-c hooks.SessionStart=[…]` override whose command never
//! changes; the pane's capture token rides the environment instead, so
//! Codex's one-time hook-trust hash covers every pane. This helper appends a
//! receipt to the host-owned capture generation under its writer lock. The
//! terminal service reads the receipts back under the same lock and trusts
//! only entries whose reporting process is the pane's spawned Codex.
//!
//! ponytail: a Codex tool shell is also a child of that process and inherits
//! the token, so a prompt-injected command could file a receipt naming any
//! existing thread and re-point its own pane. That matches Claude's exposure
//! (its hook is a direct child too); tightening it needs Codex to tell the
//! hook runner apart from tool shells.

use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

use interface_common::uuid::{canonical_uuid, canonical_uuid_v4};
use interface_storage::{
    JsonLimits, PrivateDirectory, atomic_write_json, read_json, snapshot_target,
};
use interface_terminal::{
    TerminalBackendError, TerminalProcessIdentity, TerminalProviderSession,
    TerminalProviderSessionTransition,
};
use serde::{Deserialize, Serialize};

pub(crate) use crate::capture_generation::CODEX_RECEIPT_FILE;
use crate::capture_generation::{MAX_RECEIPT_BYTES, acquire_writer_lock, generation_is_sealed};
use crate::capture_hook::{hook_process_lineage, parent_process_id, transition_for};
const CODEX_RECEIPT_VERSION: u32 = 1;
const MAX_HOOK_INPUT_BYTES: u64 = 16 * 1_024;
/// Codex fires the hook again for `/new`, `/resume` and compaction. Keeping
/// the newest few lets the fold below still see the thread that was current.
const MAX_RECEIPT_ENTRIES: usize = 8;

#[derive(Deserialize)]
struct SessionStartInput {
    session_id: String,
    transcript_path: Option<String>,
    cwd: String,
    hook_event_name: String,
    source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CodexTerminalSessionReceipts {
    pub(crate) version: u32,
    pub(crate) capture_token: String,
    pub(crate) entries: Vec<CodexTerminalSessionReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CodexTerminalSessionReceipt {
    pub(crate) session_id: String,
    pub(crate) transcript_path: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) source: String,
    /// The hook's parent and grandparent. Codex runs hook commands through a
    /// shell that may or may not exec the command in place, so either can be
    /// the pane's Codex process; the reader accepts whichever matches.
    pub(crate) reporters: Vec<ReportingProcess>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReportingProcess {
    pub(crate) process_id: u32,
    pub(crate) process_start: String,
}

/// Codex mints RFC 4122 version-7 thread ids; Interface and Claude mint v4.
pub(crate) fn codex_thread_id(value: &str) -> bool {
    canonical_uuid(value, b'7')
}

fn receipt_limits() -> io::Result<JsonLimits> {
    JsonLimits::new(MAX_RECEIPT_BYTES, 8, 2_048).map_err(io::Error::other)
}

/// Read one bounded hook payload and append it to the pane's capture
/// generation. The hook writes nothing to Codex.
///
/// Every refusal is a silent success: identity capture is a recovery aid and
/// must never interrupt Codex or leak into its transcript.
///
/// # Errors
///
/// Returns an error when the receipt store cannot be read or written.
pub fn run_codex_terminal_session_hook(
    capture_token: &str,
    receipts_root: &Path,
    input: impl Read,
) -> io::Result<()> {
    if !canonical_uuid_v4(capture_token) || !receipts_root.is_absolute() {
        return Ok(());
    }
    let mut bytes = Vec::new();
    input
        .take(MAX_HOOK_INPUT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_HOOK_INPUT_BYTES {
        return Ok(());
    }
    let Ok(payload) = serde_json::from_slice::<SessionStartInput>(&bytes) else {
        return Ok(());
    };
    let Some(mut entry) = validated_entry(payload) else {
        return Ok(());
    };
    entry.reporters = reporting_processes();
    if entry.reporters.is_empty() {
        return Ok(());
    }
    let root = PrivateDirectory::open(receipts_root).map_err(io::Error::other)?;
    // The terminal core creates and owns this generation before Codex can
    // execute. The hook may append to that exact owner only.
    let Ok(generation) = root.open_child(capture_token) else {
        return Ok(());
    };
    if generation_is_sealed(&generation)? {
        return Ok(());
    }
    let writer = acquire_writer_lock(&generation)?;
    let result = (|| -> io::Result<bool> {
        if generation_is_sealed(&generation)? {
            return Ok(false);
        }
        let mut receipts = match read_json::<CodexTerminalSessionReceipts>(
            &generation,
            CODEX_RECEIPT_FILE,
            receipt_limits()?,
        )
        .map_err(io::Error::other)?
        {
            Some(loaded) => loaded.into_parts().0,
            None => CodexTerminalSessionReceipts {
                version: CODEX_RECEIPT_VERSION,
                capture_token: capture_token.to_owned(),
                entries: Vec::new(),
            },
        };
        if receipts.version != CODEX_RECEIPT_VERSION || receipts.capture_token != capture_token {
            return Ok(false);
        }
        receipts.entries.push(entry);
        // Keep the oldest entry: it is the pane's own first thread, which
        // the fold must still see after a run of subagent startups.
        while receipts.entries.len() > MAX_RECEIPT_ENTRIES {
            receipts.entries.remove(1);
        }
        let expected = snapshot_target(&generation, CODEX_RECEIPT_FILE, MAX_RECEIPT_BYTES)
            .map_err(io::Error::other)?;
        atomic_write_json(
            &generation,
            &writer,
            CODEX_RECEIPT_FILE,
            &receipts,
            &expected,
            receipt_limits()?,
        )
        .map(|_| true)
        .map_err(io::Error::other)
    })();
    let release = writer.release(&generation).map_err(io::Error::other);
    result?;
    release?;
    Ok(())
}

fn validated_entry(payload: SessionStartInput) -> Option<CodexTerminalSessionReceipt> {
    let transcript_path = PathBuf::from(payload.transcript_path?);
    let expected_suffix = format!("-{}.jsonl", payload.session_id);
    let transcript_name = transcript_path.file_name()?.to_str()?;
    if payload.hook_event_name != "SessionStart"
        || transition_for(&payload.source).is_none()
        || !codex_thread_id(&payload.session_id)
        || !transcript_path.is_absolute()
        || !transcript_name.starts_with("rollout-")
        || !transcript_name.ends_with(&expected_suffix)
        || !Path::new(&payload.cwd).is_absolute()
    {
        return None;
    }
    Some(CodexTerminalSessionReceipt {
        session_id: payload.session_id,
        transcript_path,
        cwd: PathBuf::from(payload.cwd),
        source: payload.source,
        reporters: Vec::new(),
    })
}

fn reporting_processes() -> Vec<ReportingProcess> {
    let mut reporters = Vec::new();
    // This hook's own parent comes from getppid; only the grandparent, a
    // foreign pid, costs a `ps` lookup.
    // One listing names each process and its parent, so two processes cost
    // two `ps` lookups.
    let mut next = parent_process_id().ok();
    while let Some(process_id) = next {
        let Ok(Some((identity, parent))) = hook_process_lineage(process_id) else {
            break;
        };
        reporters.push(ReportingProcess {
            process_id: identity.process_id,
            process_start: identity.process_start,
        });
        if reporters.len() == 2 {
            break;
        }
        next = parent;
    }
    reporters
}

/// Fold one generation's receipts into the session the pane's Codex process
/// currently owns.
///
/// Mirrors the service's replacement rule: a later `startup` (a subagent
/// thread, for example) never displaces the pane's own thread, while
/// `/resume`, `/new`-style clears and compaction move the binding.
///
/// # Errors
///
/// Returns an error when the receipt file exists but is malformed or names
/// another generation.
pub(crate) fn read_codex_receipts(
    generation: &PrivateDirectory,
    capture_token: &str,
    process: &TerminalProcessIdentity,
) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
    let invalid =
        || TerminalBackendError::new("Codex terminal session receipt is unsafe or malformed");
    let limits = receipt_limits().map_err(|_| invalid())?;
    let Some(loaded) =
        read_json::<CodexTerminalSessionReceipts>(generation, CODEX_RECEIPT_FILE, limits)
            .map_err(|_| invalid())?
    else {
        return Ok(None);
    };
    let receipts = loaded.into_parts().0;
    if receipts.version != CODEX_RECEIPT_VERSION
        || receipts.capture_token != capture_token
        || receipts.entries.len() > MAX_RECEIPT_ENTRIES
    {
        return Err(invalid());
    }
    let mut current: Option<TerminalProviderSession> = None;
    for entry in &receipts.entries {
        let transition = transition_for(&entry.source).ok_or_else(invalid)?;
        if !codex_thread_id(&entry.session_id)
            || !entry.transcript_path.is_absolute()
            || !entry.cwd.is_absolute()
            || entry.reporters.is_empty()
        {
            return Err(invalid());
        }
        let reported_by_pane = entry.reporters.iter().any(|reporter| {
            reporter.process_id == process.process_id
                && reporter.process_start == process.process_start
        });
        if !reported_by_pane {
            continue;
        }
        if current.is_none() || transition != TerminalProviderSessionTransition::Startup {
            current = Some(TerminalProviderSession {
                id: entry.session_id.clone(),
                transition,
            });
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };

    use super::*;

    fn process(process_id: u32) -> TerminalProcessIdentity {
        TerminalProcessIdentity {
            process_id,
            process_group_id: process_id,
            terminal_device: Some("ttys001".into()),
            process_start: "Wed Sep 3 17:51:14 2026".into(),
        }
    }

    fn entry(session_id: &str, source: &str, reporter: u32) -> CodexTerminalSessionReceipt {
        CodexTerminalSessionReceipt {
            session_id: session_id.into(),
            transcript_path: PathBuf::from(format!("/tmp/rollout-{session_id}.jsonl")),
            cwd: PathBuf::from("/tmp/project"),
            source: source.into(),
            reporters: vec![ReportingProcess {
                process_id: reporter,
                process_start: "Wed Sep 3 17:51:14 2026".into(),
            }],
        }
    }

    const THREAD_A: &str = "01a06978-1e6e-7911-bec8-2df09b45b2b6";
    const THREAD_B: &str = "01a0697a-29c2-7341-a864-2b99a6623598";
    const TOKEN: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn codex_thread_ids_are_canonical_v7_only() {
        assert!(codex_thread_id(THREAD_A));
        assert!(!codex_thread_id(TOKEN));
        assert!(!codex_thread_id(&THREAD_A.to_uppercase()));
        assert!(!codex_thread_id("not-a-uuid"));
    }

    #[test]
    fn valid_session_receipts_are_recorded_for_every_source() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private root");
        let root = PrivateDirectory::open(temporary.path()).expect("root");
        std::fs::create_dir(temporary.path().join(TOKEN)).expect("create generation");
        std::fs::set_permissions(
            temporary.path().join(TOKEN),
            std::fs::Permissions::from_mode(0o700),
        )
        .expect("private generation");
        let generation = root.open_child(TOKEN).expect("generation");
        for source in ["startup", "resume", "clear", "compact", "fork"] {
            let payload = serde_json::json!({
                "session_id": THREAD_A,
                "transcript_path": format!("/tmp/rollout-{THREAD_A}.jsonl"),
                "cwd": "/tmp/project",
                "hook_event_name": "SessionStart",
                "source": source
            })
            .to_string();
            run_codex_terminal_session_hook(TOKEN, temporary.path(), payload.as_bytes())
                .expect("hook");
            let loaded = read_json::<CodexTerminalSessionReceipts>(
                &generation,
                CODEX_RECEIPT_FILE,
                receipt_limits().expect("limits"),
            )
            .expect("receipt read")
            .expect("written receipt")
            .into_parts()
            .0;
            let receipt = loaded.entries.last().expect("latest receipt");
            assert_eq!(receipt.session_id, THREAD_A);
            assert_eq!(receipt.source, source);
            assert!(!receipt.reporters.is_empty());
        }
    }

    #[test]
    fn claude_after_codex_in_one_shell_generation_is_still_captured() {
        const CLAUDE_SESSION: &str = "22222222-2222-4222-8222-222222222222";
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private root");
        let generation_path = temporary.path().join(TOKEN);
        std::fs::create_dir(&generation_path).expect("create generation");
        std::fs::set_permissions(&generation_path, std::fs::Permissions::from_mode(0o700))
            .expect("private generation");
        // A manual shell pane runs `codex` first under the pane's one capture token.
        let codex = serde_json::json!({
            "session_id": THREAD_A,
            "transcript_path": format!("/tmp/rollout-{THREAD_A}.jsonl"),
            "cwd": "/tmp/project",
            "hook_event_name": "SessionStart",
            "source": "startup"
        })
        .to_string();
        run_codex_terminal_session_hook(TOKEN, temporary.path(), codex.as_bytes())
            .expect("Codex hook");
        // OpenCode observes into the same generation.
        let opencode = generation_path.join(interface_terminal::OPENCODE_SESSION_RECEIPT_FILE);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&opencode)
            .and_then(|mut file| file.write_all(b"{}"))
            .expect("OpenCode observation");

        // Then `claude` in the same shell: its writer must accept both entries.
        let claude = serde_json::json!({
            "session_id": CLAUDE_SESSION,
            "transcript_path": format!("/tmp/project/{CLAUDE_SESSION}.jsonl"),
            "cwd": "/tmp/project",
            "hook_event_name": "SessionStart",
            "source": "startup"
        })
        .to_string();
        let mut output = Vec::new();
        crate::claude_terminal_hook::run_claude_terminal_session_hook(
            TOKEN,
            temporary.path(),
            claude.as_bytes(),
            &mut output,
        )
        .expect("Claude hook");

        let names = std::fs::read_dir(&generation_path)
            .expect("generation entries")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("UTF-8")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names
                .iter()
                .filter(|name| crate::capture_generation::receipt_file_id(name).is_some())
                .count(),
            1,
            "{names:?}"
        );
        let decoded: serde_json::Value = serde_json::from_slice(&output).expect("hook output");
        assert_eq!(
            decoded["terminalSequence"],
            format!(
                "\u{1b}]2;{}{TOKEN}:receipt\u{7}",
                interface_terminal::CLAUDE_SESSION_BINDING_TITLE_PREFIX
            )
        );
        // The other providers' entries are left exactly as they were.
        let generation = PrivateDirectory::open(&generation_path).expect("generation");
        let codex_receipts = read_json::<CodexTerminalSessionReceipts>(
            &generation,
            CODEX_RECEIPT_FILE,
            receipt_limits().expect("limits"),
        )
        .expect("Codex receipt read")
        .expect("Codex receipt kept")
        .into_parts()
        .0;
        assert_eq!(codex_receipts.entries.len(), 1);
        assert_eq!(codex_receipts.entries[0].session_id, THREAD_A);
        assert_eq!(std::fs::read(&opencode).expect("OpenCode kept"), b"{}");
    }

    #[test]
    fn invalid_hook_input_writes_no_receipt() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        for payload in [
            "invalid",
            r#"{"hook_event_name":"UserPromptSubmit"}"#,
            r#"{"session_id":"invalid","transcript_path":"/tmp/rollout-invalid.jsonl","cwd":"/tmp","hook_event_name":"SessionStart","source":"startup"}"#,
        ] {
            run_codex_terminal_session_hook(TOKEN, temporary.path(), payload.as_bytes())
                .expect("invalid hook input is ignored");
            assert!(!temporary.path().join(TOKEN).exists());
        }
    }

    #[test]
    fn receipts_fold_to_the_pane_process_and_keep_the_first_startup() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private generation");
        let generation = PrivateDirectory::open(temporary.path()).expect("generation");
        let write = |entries: Vec<CodexTerminalSessionReceipt>| {
            let receipts = CodexTerminalSessionReceipts {
                version: CODEX_RECEIPT_VERSION,
                capture_token: TOKEN.into(),
                entries,
            };
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(temporary.path().join(CODEX_RECEIPT_FILE))
                .expect("open receipts");
            file.write_all(&serde_json::to_vec(&receipts).expect("json"))
                .expect("write receipts");
        };
        assert_eq!(
            read_codex_receipts(&generation, TOKEN, &process(7)),
            Ok(None)
        );

        // A subagent's later startup does not displace the pane's thread; a
        // resume does. Entries reported by another process are ignored.
        write(vec![
            entry(THREAD_A, "startup", 7),
            entry(THREAD_B, "startup", 7),
            entry(THREAD_B, "startup", 9),
        ]);
        assert_eq!(
            read_codex_receipts(&generation, TOKEN, &process(7)),
            Ok(Some(TerminalProviderSession {
                id: THREAD_A.into(),
                transition: TerminalProviderSessionTransition::Startup,
            }))
        );
        assert_eq!(
            read_codex_receipts(&generation, TOKEN, &process(8)),
            Ok(None)
        );
        write(vec![
            entry(THREAD_A, "startup", 7),
            entry(THREAD_B, "resume", 7),
        ]);
        assert_eq!(
            read_codex_receipts(&generation, TOKEN, &process(7))
                .expect("receipts")
                .map(|session| session.id),
            Some(THREAD_B.into())
        );

        write(vec![entry(TOKEN, "startup", 7)]);
        assert!(read_codex_receipts(&generation, TOKEN, &process(7)).is_err());
    }
}
