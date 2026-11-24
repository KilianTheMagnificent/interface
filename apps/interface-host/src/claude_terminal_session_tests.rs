use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

use super::*;

#[test]
fn process_observation_bounds_inherited_pipes_and_rejects_excess_output() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    for script in [
        "sleep 2 & exit 0",
        "sleep 2 & wait",
        "yes output | head -c 8192",
    ] {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        let start = Instant::now();
        assert!(bounded_process_observation(&mut command, Duration::from_millis(50)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "printf identity"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    assert_eq!(
        bounded_process_observation(&mut command, Duration::from_secs(1)).unwrap(),
        b"identity"
    );
}
use crate::claude_terminal_hook::{
    MAX_RECEIPTS_PER_GENERATION, parent_process_id, run_claude_terminal_session_hook,
};

const PID: u32 = 42_424;
const CAPTURE_TOKEN: &str = "11111111-1111-4111-8111-111111111111";
const SESSION_ID: &str = "12345678-1234-4234-8234-123456789abc";
const PROCESS_START: &str = "Wed Sep 2 03:30:39 2026";
const TERMINAL_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const RECEIPT_CWD: &str = "/private/tmp/interface-claude-worktree";

struct Fixture {
    root: tempfile::TempDir,
    projects: PathBuf,
    receipts: PathBuf,
    sessions: PathBuf,
    lookup: ClaudeTerminalSessionLookup,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp root");
        let config = root.path().join(".claude");
        let projects = config.join("projects");
        let sessions = config.join("sessions");
        let receipts = root.path().join("receipts");
        fs::create_dir_all(&projects).expect("projects root");
        fs::create_dir_all(&sessions).expect("sessions root");
        fs::create_dir(&receipts).expect("receipts root");
        fs::set_permissions(&receipts, fs::Permissions::from_mode(0o700))
            .expect("private receipts root");
        let lookup = ClaudeTerminalSessionLookup::new(&projects, &receipts).expect("lookup");
        Self {
            root,
            projects,
            receipts,
            sessions,
            lookup,
        }
    }

    fn path(&self) -> PathBuf {
        self.sessions.join(format!("{PID}.json"))
    }

    fn write(&self, value: &serde_json::Value) {
        let path = self.path();
        fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("record");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private mode");
    }

    fn write_process_record(
        &self,
        process: &TerminalProcessIdentity,
        session_id: &str,
        cwd: &Path,
    ) {
        let path = self.sessions.join(format!("{}.json", process.process_id));
        let value = serde_json::json!({
            "version": "2.1.258",
            "pid": process.process_id,
            "sessionId": session_id,
            "cwd": cwd,
            "procStart": process.process_start,
            "kind": "interactive"
        });
        fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("record");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private mode");
    }

    fn read(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<Option<String>, TerminalBackendError> {
        read_current_session_with(&self.sessions, process, |_| Ok(()))
    }

    fn write_transcript(&self, session_id: &str) -> PathBuf {
        self.write_transcript_at(Path::new(RECEIPT_CWD), session_id)
    }

    fn write_transcript_at(&self, cwd: &Path, session_id: &str) -> PathBuf {
        let project = self
            .projects
            .join(claude_project_dir_name(&cwd.to_string_lossy()));
        fs::create_dir_all(&project).expect("Claude project directory");
        let path = project.join(format!("{session_id}.jsonl"));
        fs::write(&path, b"{\"type\":\"user\"}\n").expect("Claude transcript");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("private transcript mode");
        path
    }

    fn write_receipt(
        &self,
        reported_session_id: &str,
        transcript_path: PathBuf,
        source: &str,
        captured_at_unix_nanos: u64,
    ) {
        self.write_receipt_for_parent(
            reported_session_id,
            transcript_path,
            source,
            captured_at_unix_nanos,
            PID,
        );
    }

    fn write_receipt_for_parent(
        &self,
        reported_session_id: &str,
        transcript_path: PathBuf,
        source: &str,
        captured_at_unix_nanos: u64,
        parent_process_id: u32,
    ) {
        self.write_receipt_for_parent_at_cwd(
            reported_session_id,
            transcript_path,
            source,
            captured_at_unix_nanos,
            parent_process_id,
            Path::new(RECEIPT_CWD),
        );
    }

    fn write_receipt_for_parent_at_cwd(
        &self,
        reported_session_id: &str,
        transcript_path: PathBuf,
        source: &str,
        captured_at_unix_nanos: u64,
        parent_process_id: u32,
        cwd: &Path,
    ) {
        let generation = self.receipts.join(CAPTURE_TOKEN);
        if !generation.exists() {
            fs::create_dir(&generation).expect("receipt generation");
            fs::set_permissions(&generation, fs::Permissions::from_mode(0o700))
                .expect("private generation mode");
        }
        let receipt = ClaudeTerminalSessionReceipt {
            version: RECEIPT_VERSION,
            capture_token: CAPTURE_TOKEN.to_owned(),
            parent_process_id,
            reported_session_id: reported_session_id.to_owned(),
            transcript_path,
            cwd: cwd.to_path_buf(),
            source: source.to_owned(),
            captured_at_unix_nanos,
        };
        let path = generation.join(format!("{}.json", uuid::Uuid::new_v4()));
        fs::write(&path, serde_json::to_vec(&receipt).expect("receipt JSON")).expect("receipt");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("private receipt mode");
    }

    fn binding(&self) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        self.lookup
            .current_session_with(&process(), CAPTURE_TOKEN, |_| Ok(()))
    }

    fn begin_capture_at(
        &self,
        root: &str,
        cwd: &str,
        prior_launch_session: Option<TerminalLaunchSessionV1>,
    ) {
        self.lookup
            .begin_capture(
                TerminalLaunchProfileId::Claude,
                &TerminalProviderSessionCaptureOwner {
                    terminal_id: terminal_id(),
                    terminal_generation: TerminalGeneration::initial(),
                    capture_token: CAPTURE_TOKEN.to_owned(),
                    root: root.to_owned(),
                    cwd: cwd.to_owned(),
                    prior_launch_session,
                    nested_shell: false,
                },
            )
            .expect("begin capture owner");
    }

    fn record_process(&self) {
        self.lookup
            .record_process(TerminalLaunchProfileId::Claude, CAPTURE_TOKEN, &process())
            .expect("record capture process");
    }

    /// Give a retained-shell owner its provider process and cwd the way the
    /// trusted wrapper does in production. The predecessor probe answers
    /// "still alive", so seeding never replaces an existing owner.
    fn claim_nested_process(
        &self,
        process: &TerminalProcessIdentity,
        cwd: &Path,
    ) -> Result<(), TerminalBackendError> {
        claim_manual_nested_capture_process_with(
            &self.receipts,
            CAPTURE_TOKEN,
            process,
            cwd,
            |_| Ok(()),
            |_| Ok(false),
        )
    }
}

fn terminal_id() -> TerminalId {
    TerminalId::new(uuid::Uuid::parse_str(TERMINAL_ID).expect("terminal UUID"))
}

#[test]
fn opencode_selection_is_process_scoped_and_survives_crash_recovery() {
    let fixture = Fixture::new();
    fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, None);
    fixture.record_process();
    let path = fixture
        .receipts
        .join(CAPTURE_TOKEN)
        .join(interface_terminal::OPENCODE_SESSION_RECEIPT_FILE);
    let write = |session_id: &str| {
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version":1,
            "captureToken":CAPTURE_TOKEN,"processId":PID,"cwd":RECEIPT_CWD,"sessionId":session_id}))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    };
    write("ses_existing111");
    assert_eq!(
        fixture
            .lookup
            .opencode_session_with(&process(), CAPTURE_TOKEN, |_| Ok(()))
            .unwrap()
            .unwrap()
            .id,
        "ses_existing111"
    );
    write("ses_selected222");
    assert_eq!(
        fixture
            .lookup
            .opencode_session_with(&process(), CAPTURE_TOKEN, |_| Ok(()))
            .unwrap()
            .unwrap()
            .id,
        "ses_selected222"
    );
    assert!(
        fixture
            .lookup
            .opencode_session_with(
                &process_with(PID, "different process"),
                CAPTURE_TOKEN,
                |_| Ok(())
            )
            .is_err()
    );

    // A fresh host has no claim cache. The last provider observation is enough
    // to recover the exact selected chat before the workspace roster reopens.
    let lookup = ClaudeTerminalSessionLookup::new(&fixture.projects, &fixture.receipts).unwrap();
    let mut recovered = Vec::new();
    let sweep = lookup
        .reconcile_startup_receipts_with(
            |receipt| {
                recovered.push((receipt.profile_id, receipt.launch_session.id.clone()));
                StartupReceiptDisposition::Durable
            },
            |_| Ok(true),
            Duration::ZERO,
        )
        .unwrap();
    assert_eq!(
        recovered,
        vec![(
            TerminalLaunchProfileId::Opencode,
            Some("ses_selected222".into())
        )]
    );
    assert_eq!(sweep.retryable, 0);
}

fn process() -> TerminalProcessIdentity {
    process_with(PID, PROCESS_START)
}

fn process_with(process_id: u32, process_start: &str) -> TerminalProcessIdentity {
    TerminalProcessIdentity {
        process_id,
        process_group_id: process_id,
        #[cfg(target_os = "macos")]
        terminal_device: Some("ttys999".to_owned()),
        #[cfg(not(target_os = "macos"))]
        terminal_device: None,
        process_start: process_start.to_owned(),
    }
}

fn record() -> serde_json::Value {
    serde_json::json!({
        "version": "2.1.258",
        "pid": PID,
        "sessionId": SESSION_ID,
        "cwd": "/private/tmp/interface-claude-test",
        "procStart": "Wed   Sep 2  03:30:39 2026",
        "kind": "interactive",
        "ignoredFutureField": true
    })
}

#[test]
fn capture_owner_is_durable_before_spawn_and_accepts_unicode_paths() {
    let fixture = Fixture::new();
    let prior = TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: Some(SESSION_ID.to_owned()),
        resume: true,
    };
    fixture.begin_capture_at(
        "/private/tmp/Interface-工作区",
        "/private/tmp/Interface-工作区/项目",
        Some(prior.clone()),
    );

    let prepared = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
        .expect("read prepared owner")
        .expect("prepared owner");
    assert_eq!(prepared.terminal_id, terminal_id());
    assert_eq!(prepared.prior_launch_session, Some(prior));
    assert_eq!(prepared.process, None);

    fixture.record_process();
    let spawned = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
        .expect("read spawned owner")
        .expect("spawned owner");
    assert_eq!(
        spawned.process,
        Some(StoredProcessIdentity::from(&process()))
    );
}

#[test]
fn nested_capture_records_the_foreground_child_cwd_atomically() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    let child = root.join("nested");
    fs::create_dir_all(&child).expect("nested project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    let child = fs::canonicalize(child).expect("canonical provider cwd");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&process(), &child)
        .expect("record nested process");

    let owner = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
        .expect("read nested owner")
        .expect("nested owner");
    assert!(owner.nested_shell);
    assert_eq!(owner.cwd, child.to_string_lossy());
    assert_eq!(owner.process, Some(StoredProcessIdentity::from(&process())));
}

#[test]
fn durable_nested_owner_exposes_its_exact_process_identity() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&process(), &root)
        .expect("record provider");

    assert_eq!(
        fixture
            .lookup
            .capture_process(TerminalLaunchProfileId::Claude, CAPTURE_TOKEN),
        Ok(Some(TerminalProviderSessionCaptureProcess {
            process: process(),
            cwd: root,
        }))
    );
}

#[test]
fn sealing_a_nested_capture_blocks_every_later_wrapper_claim() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&process(), &root)
        .expect("record provider");

    assert_eq!(
        fixture
            .lookup
            .seal_capture(TerminalLaunchProfileId::Claude, CAPTURE_TOKEN),
        Ok(Some(TerminalProviderSessionCaptureProcess {
            process: process(),
            cwd: root.clone(),
        }))
    );
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    assert!(generation.join(CAPTURE_CLAIMS_SEALED).exists());
    assert!(
        !generation.join(RECEIPT_SEALED).exists(),
        "freezing owner changes must not prematurely seal the live provider's receipts"
    );
    assert!(
        claim_manual_nested_capture_process_with(
            &fixture.receipts,
            CAPTURE_TOKEN,
            &process_with(PID + 1, "Wed Sep 2 03:30:40 2026"),
            &root,
            |_| Ok(()),
            |_| Ok(true),
        )
        .is_err(),
        "a wrapper racing terminal teardown must not claim a sealed generation"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One scenario pins claim sealing, receipt acceptance, and final sealing.
fn claim_seal_allows_the_owned_process_receipt_until_final_receipt_seal() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    let owned_process = process_with(
        parent_process_id().expect("test parent process"),
        PROCESS_START,
    );
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&owned_process, &root)
        .expect("record owned provider");

    assert_eq!(
        fixture
            .lookup
            .seal_capture(TerminalLaunchProfileId::Claude, CAPTURE_TOKEN),
        Ok(Some(TerminalProviderSessionCaptureProcess {
            process: owned_process.clone(),
            cwd: root.clone(),
        }))
    );
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    assert!(generation.join(CAPTURE_CLAIMS_SEALED).exists());
    assert!(!generation.join(RECEIPT_SEALED).exists());

    let transcript = fixture.write_transcript_at(&root, SESSION_ID);
    let input = serde_json::to_vec(&serde_json::json!({
        "session_id": SESSION_ID,
        "transcript_path": transcript,
        "cwd": root,
        "hook_event_name": "SessionStart",
        "source": "startup",
    }))
    .expect("hook input");
    let mut output = Vec::new();
    run_claude_terminal_session_hook(
        CAPTURE_TOKEN,
        &fixture.receipts,
        input.as_slice(),
        &mut output,
    )
    .expect("owned provider hook");
    assert!(
        !output.is_empty(),
        "the already-owned provider must still be able to publish its delayed receipt"
    );
    assert_eq!(
        read_verified_receipt(
            &fixture.projects,
            &fixture.receipts,
            CAPTURE_TOKEN,
            &owned_process,
            None,
            Some(&root),
        ),
        Ok(Some(TerminalProviderSession {
            id: SESSION_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        }))
    );
    let receipt_count = || {
        fs::read_dir(&generation)
            .expect("receipt generation")
            .map(|entry| entry.expect("receipt entry").file_name())
            .filter(|name| receipt_file_id(&name.to_string_lossy()).is_some())
            .count()
    };
    assert_eq!(receipt_count(), 1);

    assert_eq!(
        read_final_verified_receipt(
            &fixture.projects,
            &fixture.receipts,
            CAPTURE_TOKEN,
            &owned_process,
            Some(&root),
        ),
        Ok(Some(TerminalProviderSession {
            id: SESSION_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        }))
    );
    assert!(generation.join(RECEIPT_SEALED).exists());

    let mut rejected_output = Vec::new();
    run_claude_terminal_session_hook(
        CAPTURE_TOKEN,
        &fixture.receipts,
        input.as_slice(),
        &mut rejected_output,
    )
    .expect("post-finalization hook");
    assert!(rejected_output.is_empty());
    assert_eq!(receipt_count(), 1);
}

#[test]
fn settled_nested_receipt_stays_reusable_until_its_checkpoint() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&process(), &root)
        .expect("record provider");
    let transcript = fixture.write_transcript_at(&root, SESSION_ID);
    fixture.write_receipt_for_parent_at_cwd(
        SESSION_ID,
        transcript,
        "startup",
        1,
        process().process_id,
        &root,
    );

    assert!(
        fixture
            .lookup
            .settled_session_with(&process(), CAPTURE_TOKEN, |_| Ok(false))
            .is_err(),
        "an active or ambiguous process cannot be settled"
    );
    assert_eq!(
        fixture
            .lookup
            .settled_session_with(&process(), CAPTURE_TOKEN, |_| Ok(true)),
        Ok(Some(TerminalProviderSession {
            id: SESSION_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        }))
    );
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    assert!(
        !generation.join(RECEIPT_SEALED).exists(),
        "settling a nested child must not seal its retained shell generation"
    );
    assert_eq!(
        capture_owner(&fixture.receipts, CAPTURE_TOKEN)
            .expect("owner")
            .expect("owner")
            .process,
        Some(StoredProcessIdentity::from(&process()))
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One scenario pins predecessor rollover and exact ownership transfer.
fn trusted_wrapper_claim_and_proven_rollover_preserve_the_exact_predecessor() {
    const NEXT_SESSION: &str = "22222222-2222-4222-8222-222222222222";

    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    let prior = TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: Some(SESSION_ID.to_owned()),
        resume: true,
    };
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: Some(prior.clone()),
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&process(), &root)
        .expect("record first provider");
    let old_transcript = fixture.write_transcript_at(&root, SESSION_ID);
    fixture.write_receipt_for_parent_at_cwd(
        SESSION_ID,
        old_transcript,
        "resume",
        1,
        process().process_id,
        &root,
    );

    let replacement = process_with(PID + 1, "Wed Sep 2 03:30:40 2026");
    claim_manual_nested_capture_process_with(
        &fixture.receipts,
        CAPTURE_TOKEN,
        &replacement,
        &root,
        |_| Ok(()),
        |_| Ok(true),
    )
    .expect("wrapper claims replacement before exec");
    let claimed = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
        .expect("claimed owner")
        .expect("owner");
    assert_eq!(
        claimed.process,
        Some(StoredProcessIdentity::from(&replacement))
    );
    assert_eq!(claimed.prior_launch_session, Some(prior.clone()));
    assert!(claimed.nested_replacement);
    assert_eq!(
        fs::read_dir(fixture.receipts.join(CAPTURE_TOKEN))
            .expect("generation")
            .filter_map(Result::ok)
            .filter(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
            .count(),
        0,
        "the old invocation lineage must be pruned before exec"
    );

    fixture.write_process_record(&replacement, NEXT_SESSION, &root);
    let next_transcript = fixture.write_transcript_at(&root, NEXT_SESSION);
    fixture.write_receipt_for_parent_at_cwd(
        NEXT_SESSION,
        next_transcript,
        "startup",
        2,
        replacement.process_id,
        &root,
    );
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    let owner_before_replace =
        fs::read(generation.join(CAPTURE_OWNER_FILE)).expect("claimed owner bytes");
    assert!(
        replace_nested_capture_process_with(
            fixture.projects.parent().expect("config root"),
            &fixture.sessions,
            &fixture.projects,
            &fixture.receipts,
            CAPTURE_TOKEN,
            &process(),
            &replacement,
            &root,
            Some(&prior),
            |_| Ok(()),
            |_| Ok(false),
        )
        .is_err(),
        "replacement adoption must re-prove full predecessor-group absence"
    );
    assert_eq!(
        fs::read(generation.join(CAPTURE_OWNER_FILE)).expect("retained claimed owner bytes"),
        owner_before_replace
    );
    let adopted = replace_nested_capture_process_with(
        fixture.projects.parent().expect("config root"),
        &fixture.sessions,
        &fixture.projects,
        &fixture.receipts,
        CAPTURE_TOKEN,
        &process(),
        &replacement,
        &root,
        Some(&prior),
        |_| Ok(()),
        |_| Ok(true),
    )
    .expect("prove replacement receipt")
    .expect("replacement session");
    assert_eq!(adopted.id, NEXT_SESSION);
    assert_eq!(
        adopted.transition,
        TerminalProviderSessionTransition::Startup
    );
    let adopted_owner = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
        .expect("adopted owner")
        .expect("owner");
    assert_eq!(adopted_owner.prior_launch_session, Some(prior));
    assert!(!adopted_owner.nested_replacement);
}

#[test]
fn trusted_wrapper_refuses_to_delete_an_uncheckpointed_predecessor_receipt() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&process(), &root)
        .expect("record first provider");
    let transcript = fixture.write_transcript_at(&root, SESSION_ID);
    fixture.write_receipt_for_parent_at_cwd(
        SESSION_ID,
        transcript,
        "startup",
        1,
        process().process_id,
        &root,
    );
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    let receipt = fs::read_dir(&generation)
        .expect("generation")
        .filter_map(Result::ok)
        .find(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
        .expect("predecessor receipt");
    let receipt_name = receipt.file_name();
    let receipt_bytes = fs::read(receipt.path()).expect("predecessor receipt bytes");

    let replacement = process_with(PID + 1, "Wed Sep 2 03:30:40 2026");
    assert!(
        claim_manual_nested_capture_process_with(
            &fixture.receipts,
            CAPTURE_TOKEN,
            &replacement,
            &root,
            |_| Ok(()),
            |_| Ok(true),
        )
        .is_err(),
        "replacement must wait for predecessor persistence and checkpoint"
    );

    let owner = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
        .expect("read retained owner")
        .expect("retained owner");
    assert_eq!(owner.process, Some(StoredProcessIdentity::from(&process())));
    assert_eq!(owner.prior_launch_session, None);
    assert!(!owner.nested_replacement);
    assert_eq!(
        fs::read(generation.join(receipt_name)).expect("retained predecessor receipt"),
        receipt_bytes
    );
}

#[test]
fn trusted_wrapper_preserves_a_checkpointed_leaderless_but_live_predecessor_group() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    let predecessor = process();
    fixture
        .claim_nested_process(&predecessor, &root)
        .expect("record predecessor");
    let transcript = fixture.write_transcript_at(&root, SESSION_ID);
    fixture.write_receipt_for_parent_at_cwd(
        SESSION_ID,
        transcript,
        "startup",
        1,
        predecessor.process_id,
        &root,
    );
    fixture
        .lookup
        .checkpoint_capture(
            TerminalLaunchProfileId::Claude,
            CAPTURE_TOKEN,
            &TerminalLaunchSessionV1 {
                version: TERMINAL_LAUNCH_SESSION_VERSION,
                id: Some(SESSION_ID.to_owned()),
                resume: true,
            },
        )
        .expect("checkpoint predecessor");

    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    let owner_before = fs::read(generation.join(CAPTURE_OWNER_FILE)).expect("owner bytes");
    let receipt = fs::read_dir(&generation)
        .expect("generation")
        .filter_map(Result::ok)
        .find(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
        .expect("predecessor receipt");
    let receipt_name = receipt.file_name();
    let receipt_before = fs::read(receipt.path()).expect("receipt bytes");
    let replacement = process_with(PID + 1, "Wed Sep 2 03:30:40 2026");

    assert!(
        claim_manual_nested_capture_process_with(
            &fixture.receipts,
            CAPTURE_TOKEN,
            &replacement,
            &root,
            |_| Ok(()),
            |observed| {
                assert_eq!(observed, &StoredProcessIdentity::from(&predecessor));
                // The leader PID is absent, but a descendant still owns
                // the captured PGID, so the full-group probe says the old
                // generation is not gone.
                Ok(false)
            },
        )
        .is_err(),
        "a leaderless live predecessor group must block wrapper rollover"
    );
    assert_eq!(
        fs::read(generation.join(CAPTURE_OWNER_FILE)).expect("retained owner bytes"),
        owner_before
    );
    assert_eq!(
        fs::read(generation.join(&receipt_name)).expect("retained receipt bytes"),
        receipt_before
    );

    claim_manual_nested_capture_process_with(
        &fixture.receipts,
        CAPTURE_TOKEN,
        &replacement,
        &root,
        |_| Ok(()),
        |_| Ok(true),
    )
    .expect("roll over after full process-group absence");
    assert_eq!(
        capture_owner(&fixture.receipts, CAPTURE_TOKEN)
            .expect("replacement owner")
            .expect("owner")
            .process,
        Some(StoredProcessIdentity::from(&replacement))
    );
    assert!(!generation.join(receipt_name).exists());
}

#[test]
#[allow(clippy::too_many_lines)] // One scenario pins receipt durability across failed rearm.
fn rearm_refuses_to_delete_an_uncheckpointed_own_receipt() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&process(), &root)
        .expect("record provider");
    let transcript = fixture.write_transcript_at(&root, SESSION_ID);
    fixture.write_receipt_for_parent_at_cwd(
        SESSION_ID,
        transcript,
        "startup",
        1,
        process().process_id,
        &root,
    );

    assert!(
        rearm_nested_capture_process_with(
            &fixture.receipts,
            CAPTURE_TOKEN,
            &process(),
            &root,
            |_| Ok(true),
        )
        .is_err(),
        "shell rearm must wait for provider persistence and checkpoint"
    );
    assert_eq!(
        capture_owner(&fixture.receipts, CAPTURE_TOKEN)
            .expect("owner")
            .expect("owner")
            .process,
        Some(StoredProcessIdentity::from(&process()))
    );
    assert_eq!(
        fs::read_dir(fixture.receipts.join(CAPTURE_TOKEN))
            .expect("generation")
            .filter_map(Result::ok)
            .filter(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
            .count(),
        1
    );

    fixture
        .lookup
        .checkpoint_capture(
            TerminalLaunchProfileId::Claude,
            CAPTURE_TOKEN,
            &TerminalLaunchSessionV1 {
                version: TERMINAL_LAUNCH_SESSION_VERSION,
                id: Some(SESSION_ID.to_owned()),
                resume: true,
            },
        )
        .expect("checkpoint durable receipt");
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    let owner_before_rearm =
        fs::read(generation.join(CAPTURE_OWNER_FILE)).expect("checkpointed owner bytes");
    assert!(
        rearm_nested_capture_process_with(
            &fixture.receipts,
            CAPTURE_TOKEN,
            &process(),
            &root,
            |_| Ok(false),
        )
        .is_err(),
        "rearm must wait for full provider process-group absence"
    );
    assert_eq!(
        fs::read(generation.join(CAPTURE_OWNER_FILE)).expect("retained owner bytes"),
        owner_before_rearm
    );
    assert_eq!(
        fs::read_dir(&generation)
            .expect("generation")
            .filter_map(Result::ok)
            .filter(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
            .count(),
        1
    );
    rearm_nested_capture_process_with(&fixture.receipts, CAPTURE_TOKEN, &process(), &root, |_| {
        Ok(true)
    })
    .expect("rearm checkpointed provider");
    let owner = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
        .expect("rearmed owner")
        .expect("owner");
    assert_eq!(owner.process, None);
    assert_eq!(owner.prior_launch_session, None);
    assert_eq!(
        fs::read_dir(fixture.receipts.join(CAPTURE_TOKEN))
            .expect("generation")
            .filter_map(Result::ok)
            .filter(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
            .count(),
        0
    );
}

#[test]
fn rearm_refuses_a_foreign_committed_receipt_instead_of_erasing_it() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    fixture
        .claim_nested_process(&process(), &root)
        .expect("record provider");
    let transcript = fixture.write_transcript_at(&root, SESSION_ID);
    fixture.write_receipt_for_parent_at_cwd(SESSION_ID, transcript, "startup", 1, PID + 1, &root);

    assert!(
        rearm_nested_capture_process_with(
            &fixture.receipts,
            CAPTURE_TOKEN,
            &process(),
            &root,
            |_| Ok(true),
        )
        .is_err()
    );
    assert_eq!(
        capture_owner(&fixture.receipts, CAPTURE_TOKEN)
            .expect("owner")
            .expect("owner")
            .process,
        Some(StoredProcessIdentity::from(&process()))
    );
    assert_eq!(
        fs::read_dir(fixture.receipts.join(CAPTURE_TOKEN))
            .expect("generation")
            .filter_map(Result::ok)
            .filter(|entry| receipt_file_id(&entry.file_name().to_string_lossy()).is_some())
            .count(),
        1
    );
}

#[test]
fn repeated_nested_rearms_prune_old_lineages_below_the_hook_bound() {
    let fixture = Fixture::new();
    let root = fixture.root.path().join("project");
    fs::create_dir(&root).expect("project directory");
    let root = fs::canonicalize(root).expect("canonical project root");
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: CAPTURE_TOKEN.to_owned(),
                root: root.to_string_lossy().into_owned(),
                cwd: root.to_string_lossy().into_owned(),
                prior_launch_session: None,
                nested_shell: true,
            },
        )
        .expect("begin nested capture");
    let transcript = fixture.write_transcript_at(&root, SESSION_ID);

    for index in 0..=MAX_RECEIPTS_PER_GENERATION {
        let pid = 50_000 + u32::try_from(index).expect("bounded test index");
        let process = process_with(pid, &format!("Wed Sep 2 03:{:02}:00 2026", index % 60));
        fixture
            .claim_nested_process(&process, &root)
            .expect("record repeated provider");
        fixture.write_receipt_for_parent_at_cwd(
            SESSION_ID,
            transcript.clone(),
            "startup",
            u64::try_from(index).expect("timestamp") + 1,
            pid,
            &root,
        );
        fixture
            .lookup
            .checkpoint_capture(
                TerminalLaunchProfileId::Claude,
                CAPTURE_TOKEN,
                &TerminalLaunchSessionV1 {
                    version: TERMINAL_LAUNCH_SESSION_VERSION,
                    id: Some(SESSION_ID.to_owned()),
                    resume: true,
                },
            )
            .expect("checkpoint repeated provider");
        rearm_nested_capture_process_with(
            &fixture.receipts,
            CAPTURE_TOKEN,
            &process,
            &root,
            |_| Ok(true),
        )
        .expect("rearm repeated provider");
    }
    let entries = fs::read_dir(fixture.receipts.join(CAPTURE_TOKEN))
        .expect("generation")
        .collect::<Result<Vec<_>, _>>()
        .expect("entries");
    assert!(entries.len() < MAX_CAPTURE_GENERATION_ENTRIES);
    assert!(
        entries
            .iter()
            .all(|entry| { receipt_file_id(&entry.file_name().to_string_lossy()).is_none() })
    );
}

#[test]
fn unresolved_capture_owner_blocks_a_second_generation_for_the_same_terminal() {
    const SECOND_TOKEN: &str = "22222222-2222-4222-8222-222222222222";

    let fixture = Fixture::new();
    fixture.begin_capture_at(
        "/private/tmp/interface-claude-worktree",
        "/private/tmp/interface-claude-worktree",
        None,
    );
    let second = TerminalProviderSessionCaptureOwner {
        terminal_id: terminal_id(),
        terminal_generation: TerminalGeneration(2),
        capture_token: SECOND_TOKEN.to_owned(),
        root: RECEIPT_CWD.to_owned(),
        cwd: RECEIPT_CWD.to_owned(),
        prior_launch_session: None,
        nested_shell: false,
    };

    assert!(
        fixture
            .lookup
            .begin_capture(TerminalLaunchProfileId::Claude, &second)
            .is_err()
    );
    assert!(!fixture.receipts.join(SECOND_TOKEN).exists());
    assert!(fixture.receipts.join(CAPTURE_TOKEN).is_dir());
}

#[test]
fn startup_retires_a_processless_owner_receipt_and_unblocks_the_terminal_id() {
    const SECOND_TOKEN: &str = "22222222-2222-4222-8222-222222222222";
    const RECEIPT_SESSION: &str = "77777777-7777-4777-8777-777777777777";

    let fixture = Fixture::new();
    fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, None);
    let transcript = fixture.write_transcript(RECEIPT_SESSION);
    fixture.write_receipt(RECEIPT_SESSION, transcript, "startup", 1_000_000_000);

    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |_| panic!("a processless receipt must never be applied"),
            |_| panic!("a processless owner must never be probed"),
            Duration::ZERO,
        )
        .expect("retire unauthenticated generation");
    assert_eq!(sweep.examined, 1);
    assert_eq!(sweep.retired, 1);
    assert_eq!(sweep.retryable, 0);
    assert!(!fixture.receipts.join(CAPTURE_TOKEN).exists());

    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration(2),
                capture_token: SECOND_TOKEN.to_owned(),
                root: RECEIPT_CWD.to_owned(),
                cwd: RECEIPT_CWD.to_owned(),
                prior_launch_session: None,
                nested_shell: false,
            },
        )
        .expect("same terminal can start a fresh capture");
    assert!(fixture.receipts.join(SECOND_TOKEN).is_dir());
}

#[test]
fn startup_retires_a_processless_owner_without_receipts() {
    let fixture = Fixture::new();
    fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, None);

    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |_| panic!("a processless owner has no applicable session"),
            |_| panic!("a processless owner must never be probed"),
            Duration::ZERO,
        )
        .expect("retire processless owner");
    assert_eq!(sweep.examined, 1);
    assert_eq!(sweep.retired, 1);
    assert_eq!(sweep.retryable, 0);
    assert!(!fixture.receipts.join(CAPTURE_TOKEN).exists());
}

#[test]
fn capture_checkpoint_advances_the_crash_recovery_predecessor() {
    const PRIOR: &str = "33333333-3333-4333-8333-333333333333";
    const CHECKPOINT: &str = "44444444-4444-4444-8444-444444444444";
    const RECOVERED: &str = "55555555-5555-4555-8555-555555555555";

    let fixture = Fixture::new();
    fixture.begin_capture_at(
        RECEIPT_CWD,
        RECEIPT_CWD,
        Some(TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: Some(PRIOR.to_owned()),
            resume: true,
        }),
    );
    fixture.record_process();
    fixture
        .lookup
        .checkpoint_capture(
            TerminalLaunchProfileId::Claude,
            CAPTURE_TOKEN,
            &TerminalLaunchSessionV1 {
                version: TERMINAL_LAUNCH_SESSION_VERSION,
                id: Some(CHECKPOINT.to_owned()),
                resume: true,
            },
        )
        .expect("checkpoint durable provider transition");
    let transcript = fixture.write_transcript(RECOVERED);
    fixture.write_receipt(RECOVERED, transcript, "clear", 2_000_000_000);

    let mut reconciled = false;
    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |recovery| {
                reconciled = true;
                assert_eq!(
                    recovery
                        .prior_launch_session
                        .as_ref()
                        .and_then(|session| session.id.as_deref()),
                    Some(CHECKPOINT)
                );
                assert_eq!(recovery.launch_session.id.as_deref(), Some(RECOVERED));
                StartupReceiptDisposition::Durable
            },
            |_| Ok(true),
            Duration::ZERO,
        )
        .expect("recover post-checkpoint transition");
    assert!(reconciled);
    assert_eq!(sweep.retired, 1);
    assert!(!fixture.receipts.join(CAPTURE_TOKEN).exists());
}

#[test]
fn startup_sweep_retries_then_durably_retires_a_dead_process_receipt() {
    const DURABLE_ID: &str = "22222222-2222-4222-8222-222222222222";

    let fixture = Fixture::new();
    fixture.begin_capture_at(
        "/private/tmp/interface-claude-worktree",
        "/private/tmp/interface-claude-worktree",
        Some(TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: Some(SESSION_ID.to_owned()),
            resume: true,
        }),
    );
    fixture.record_process();
    let transcript = fixture.write_transcript(DURABLE_ID);
    fixture.write_receipt(SESSION_ID, transcript, "resume", 1_000_000_000);

    let mut observed = Vec::new();
    let retryable = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |recovered| {
                observed.push(recovered.clone());
                StartupReceiptDisposition::Retryable
            },
            |_| Ok(true),
            Duration::ZERO,
        )
        .expect("retryable startup sweep");
    assert_eq!(retryable.examined, 1);
    assert_eq!(retryable.retryable, 1);
    assert!(fixture.receipts.join(CAPTURE_TOKEN).is_dir());
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].terminal_id, terminal_id());
    assert_eq!(observed[0].launch_session.id.as_deref(), Some(DURABLE_ID));

    let durable = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |_| StartupReceiptDisposition::Durable,
            |_| Ok(true),
            Duration::ZERO,
        )
        .expect("durable startup sweep");
    assert_eq!(durable.examined, 1);
    assert_eq!(durable.retired, 1);
    assert!(!fixture.receipts.join(CAPTURE_TOKEN).exists());
}

#[test]
fn startup_sweep_keeps_a_capture_whose_exact_process_may_still_be_alive() {
    let fixture = Fixture::new();
    fixture.begin_capture_at(
        "/private/tmp/interface-claude-worktree",
        "/private/tmp/interface-claude-worktree",
        None,
    );
    fixture.record_process();

    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |_| panic!("an active process must not be reconciled"),
            |_| Ok(false),
            Duration::ZERO,
        )
        .expect("active startup sweep");
    assert_eq!(sweep.active, 1);
    assert_eq!(sweep.retired, 0);
    assert!(fixture.receipts.join(CAPTURE_TOKEN).is_dir());
}

#[test]
fn startup_keeps_a_leaderless_live_group_registered_for_a_delayed_hook_receipt() {
    let fixture = Fixture::new();
    fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, None);
    fixture.record_process();

    let active = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |_| panic!("a leaderless live group must not be reconciled"),
            |_| {
                // Models an absent leader PID with a surviving process in
                // the same captured group.
                Ok(false)
            },
            Duration::ZERO,
        )
        .expect("retain active process group");
    assert_eq!(active.active, 1);
    assert_eq!(active.retired, 0);
    assert!(fixture.receipts.join(CAPTURE_TOKEN).is_dir());

    // A hook that was already scheduled by the surviving group can still
    // publish after the first startup probe. Once the complete group is
    // absent, a later sweep applies and retires that durable evidence.
    let transcript = fixture.write_transcript(SESSION_ID);
    fixture.write_receipt(SESSION_ID, transcript, "startup", 1_000_000_000);
    let mut recovered = false;
    let settled = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |session| {
                recovered = true;
                assert_eq!(session.launch_session.id.as_deref(), Some(SESSION_ID));
                StartupReceiptDisposition::Durable
            },
            |_| Ok(true),
            Duration::ZERO,
        )
        .expect("reconcile after full group absence");
    assert!(recovered);
    assert_eq!(settled.retired, 1);
    assert!(!fixture.receipts.join(CAPTURE_TOKEN).exists());
}

#[test]
fn startup_sweep_waits_boundedly_for_an_immediate_restart_child_exit() {
    let fixture = Fixture::new();
    fixture.begin_capture_at(
        "/private/tmp/interface-claude-worktree",
        "/private/tmp/interface-claude-worktree",
        None,
    );
    fixture.record_process();
    let transcript = fixture.write_transcript(SESSION_ID);
    fixture.write_receipt(SESSION_ID, transcript, "startup", 1_000_000_000);
    let mut probes = 0_usize;

    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |_| StartupReceiptDisposition::Durable,
            |_| {
                probes = probes.saturating_add(1);
                Ok(probes > 1)
            },
            Duration::ZERO,
        )
        .expect("immediate restart sweep");
    assert_eq!(probes, 2);
    assert_eq!(sweep.active, 0);
    assert_eq!(sweep.retired, 1);
    assert!(!fixture.receipts.join(CAPTURE_TOKEN).exists());
}

#[test]
fn startup_settle_rounds_do_not_starve_a_later_exiting_generation() {
    const SECOND_TOKEN: &str = "22222222-2222-4222-8222-222222222222";
    const SECOND_SESSION: &str = "66666666-6666-4666-8666-666666666666";
    const SECOND_TERMINAL: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

    let fixture = Fixture::new();
    fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, None);
    fixture.record_process();
    let second_process = TerminalProcessIdentity {
        process_id: PID + 1,
        process_group_id: PID + 1,
        #[cfg(target_os = "macos")]
        terminal_device: Some("ttys998".to_owned()),
        #[cfg(not(target_os = "macos"))]
        terminal_device: None,
        process_start: "Wed Sep 2 03:30:40 2026".to_owned(),
    };
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: TerminalId::new(
                    uuid::Uuid::parse_str(SECOND_TERMINAL).expect("second terminal UUID"),
                ),
                terminal_generation: TerminalGeneration::initial(),
                capture_token: SECOND_TOKEN.to_owned(),
                root: RECEIPT_CWD.to_owned(),
                cwd: RECEIPT_CWD.to_owned(),
                prior_launch_session: None,
                nested_shell: false,
            },
        )
        .expect("second capture owner");
    fixture
        .lookup
        .record_process(
            TerminalLaunchProfileId::Claude,
            SECOND_TOKEN,
            &second_process,
        )
        .expect("second process identity");
    let transcript = fixture.write_transcript(SECOND_SESSION);
    let second_generation = fixture.receipts.join(SECOND_TOKEN);
    let receipt = ClaudeTerminalSessionReceipt {
        version: RECEIPT_VERSION,
        capture_token: SECOND_TOKEN.to_owned(),
        parent_process_id: second_process.process_id,
        reported_session_id: SECOND_SESSION.to_owned(),
        transcript_path: transcript,
        cwd: PathBuf::from(RECEIPT_CWD),
        source: "startup".to_owned(),
        captured_at_unix_nanos: 3_000_000_000,
    };
    let receipt_path = second_generation.join(format!("{}.json", uuid::Uuid::new_v4()));
    fs::write(
        &receipt_path,
        serde_json::to_vec(&receipt).expect("second receipt JSON"),
    )
    .expect("second receipt");
    fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o600))
        .expect("private receipt mode");
    let mut second_probes = 0_usize;

    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |_| StartupReceiptDisposition::Durable,
            |process| {
                if process.process_id == PID {
                    Ok(false)
                } else {
                    second_probes = second_probes.saturating_add(1);
                    Ok(second_probes > 1)
                }
            },
            Duration::ZERO,
        )
        .expect("fair startup settle rounds");
    assert_eq!(second_probes, 2);
    assert_eq!(sweep.active, 1);
    assert_eq!(sweep.retired, 1);
    assert!(fixture.receipts.join(CAPTURE_TOKEN).is_dir());
    assert!(!fixture.receipts.join(SECOND_TOKEN).exists());
}

#[test]
fn receipt_root_generation_scan_is_hard_bounded() {
    let fixture = Fixture::new();
    for _ in 0..=MAX_CAPTURE_GENERATIONS {
        let generation = fixture.receipts.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&generation).expect("generation directory");
        fs::set_permissions(&generation, fs::Permissions::from_mode(0o700))
            .expect("private generation mode");
    }
    let root = PrivateDirectory::open(&fixture.receipts).expect("receipt root");
    assert!(bounded_generation_names(&root).is_err());
}

#[test]
fn reads_a_valid_owned_interactive_record() {
    let fixture = Fixture::new();
    fixture.write(&record());

    assert_eq!(fixture.read(&process()), Ok(Some(SESSION_ID.to_owned())));
    assert_eq!(
        fixture
            .lookup
            .current_session(TerminalLaunchProfileId::Codex, &process(), CAPTURE_TOKEN),
        Ok(None)
    );
}

#[test]
fn moved_project_receipt_recovers_on_restart_and_unblocks_the_saved_pane() {
    const SECOND_TOKEN: &str = "22222222-2222-4222-8222-222222222222";
    let fixture = Fixture::new();
    let prior = Some(TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: None,
        resume: false,
    });
    fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, prior.clone());
    fixture.record_process();
    fixture.write(&record());
    let transcript = fixture.write_transcript(SESSION_ID);
    let project = transcript.parent().unwrap();
    let old_project = fixture.projects.join("-old-project");
    fs::rename(project, &old_project).unwrap();
    symlink(old_project.file_name().unwrap(), project).unwrap();
    fixture.write_receipt(SESSION_ID, transcript, "startup", 1_000_000_000);

    assert_eq!(
        fixture.binding().unwrap().unwrap().id,
        SESSION_ID,
        "the running pane must retain its exact conversation"
    );
    let restarted = ClaudeTerminalSessionLookup::new(&fixture.projects, &fixture.receipts)
        .expect("restarted lookup");
    let mut recovered = None;
    let sweep = restarted
        .reconcile_startup_receipts_with(
            |receipt| {
                assert_eq!(receipt.terminal_id, terminal_id());
                assert_eq!(receipt.prior_launch_session, prior);
                recovered = Some(receipt.launch_session.clone());
                StartupReceiptDisposition::Durable
            },
            |_| Ok(true),
            Duration::ZERO,
        )
        .unwrap();
    assert_eq!(sweep.retired, 1);
    assert_eq!(sweep.retryable, 0);
    assert_eq!(recovered.as_ref().unwrap().id.as_deref(), Some(SESSION_ID));
    assert!(recovered.as_ref().unwrap().resume);
    assert!(!fixture.receipts.join(CAPTURE_TOKEN).exists());
    restarted
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration(2),
                capture_token: SECOND_TOKEN.to_owned(),
                root: RECEIPT_CWD.to_owned(),
                cwd: RECEIPT_CWD.to_owned(),
                prior_launch_session: recovered,
                nested_shell: false,
            },
        )
        .expect("the recovered pane can reopen after restart");
}

#[test]
fn worktree_transcript_receipt_wins_over_the_launcher_sidecar_id() {
    const DURABLE_ID: &str = "22222222-2222-4222-8222-222222222222";

    let fixture = Fixture::new();
    fixture.write(&record());
    let transcript = fixture.write_transcript(DURABLE_ID);
    fixture.write_receipt(SESSION_ID, transcript, "startup", 1_000_000_000);

    assert_eq!(
        fixture.binding(),
        Ok(Some(TerminalProviderSession {
            id: DURABLE_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        }))
    );
}

#[test]
fn receipt_cannot_select_a_transcript_from_another_claude_project() {
    let fixture = Fixture::new();
    fixture.write(&record());
    let wrong_project = fixture.projects.join("-private-tmp-another-worktree");
    fs::create_dir_all(&wrong_project).expect("other Claude project directory");
    let transcript = wrong_project.join(format!("{SESSION_ID}.jsonl"));
    fs::write(&transcript, b"{\"type\":\"user\"}\n").expect("other transcript");
    fs::set_permissions(&transcript, fs::Permissions::from_mode(0o600))
        .expect("private transcript mode");
    fixture.write_receipt(SESSION_ID, transcript, "startup", 1_000_000_000);

    assert!(fixture.binding().is_err());
}

#[test]
fn reaped_final_receipt_survives_provider_sidecar_removal() {
    const DURABLE_ID: &str = "22222222-2222-4222-8222-222222222222";

    let fixture = Fixture::new();
    let transcript = fixture.write_transcript(DURABLE_ID);
    fixture.write_receipt(SESSION_ID, transcript, "resume", 1_000_000_000);

    assert!(!fixture.path().exists(), "provider PID sidecar is absent");
    assert_eq!(
        fixture
            .lookup
            .final_session(TerminalLaunchProfileId::Claude, &process(), CAPTURE_TOKEN,),
        Ok(Some(TerminalProviderSession {
            id: DURABLE_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Resume,
        }))
    );
    fixture
        .lookup
        .end_capture(TerminalLaunchProfileId::Claude, CAPTURE_TOKEN)
        .expect("retire final receipt");
    assert!(!fixture.receipts.join(CAPTURE_TOKEN).exists());
}

#[test]
fn reaped_finalization_waits_for_a_pending_hook_before_retiring() {
    let fixture = Fixture::new();
    let transcript = fixture.write_transcript(SESSION_ID);
    fixture.write_receipt(SESSION_ID, transcript, "startup", 1_000_000_000);
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    let helper = hook_process_identity(std::process::id())
        .expect("helper process probe")
        .expect("live helper process");
    let pending = generation.join(pending_marker_name(&helper, uuid::Uuid::new_v4()));
    fs::write(&pending, []).expect("pending marker");
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o600))
        .expect("private pending marker mode");
    let pending_for_writer = pending.clone();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        fs::remove_file(pending_for_writer).expect("finish pending hook");
    });

    assert_eq!(
        fixture
            .lookup
            .final_session(TerminalLaunchProfileId::Claude, &process(), CAPTURE_TOKEN,),
        Ok(Some(TerminalProviderSession {
            id: SESSION_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        }))
    );
    writer.join().expect("pending hook thread");
    fixture
        .lookup
        .end_capture(TerminalLaunchProfileId::Claude, CAPTURE_TOKEN)
        .expect("retire final receipt");
    assert!(!generation.exists());
}

#[test]
fn newest_nonstartup_receipt_is_a_durability_barrier() {
    const STARTUP_ID: &str = "22222222-2222-4222-8222-222222222222";
    const CLEAR_ID: &str = "33333333-3333-4333-8333-333333333333";

    let fixture = Fixture::new();
    fixture.write(&record());
    let startup = fixture.write_transcript(STARTUP_ID);
    let clear = startup.with_file_name(format!("{CLEAR_ID}.jsonl"));
    fixture.write_receipt(STARTUP_ID, startup, "startup", 1_000_000_000);
    fixture.write_receipt(CLEAR_ID, clear.clone(), "clear", 2_000_000_000);
    let mut current = record();
    current["sessionId"] = serde_json::json!(CLEAR_ID);
    fixture.write(&current);

    assert_eq!(fixture.binding(), Ok(None));
    fs::write(&clear, b"{\"type\":\"user\"}\n").expect("clear transcript");
    fs::set_permissions(&clear, fs::Permissions::from_mode(0o600))
        .expect("private transcript mode");
    assert_eq!(
        fixture.binding(),
        Ok(Some(TerminalProviderSession {
            id: CLEAR_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Clear,
        }))
    );
}

#[test]
fn nonstartup_receipt_outranks_a_duplicate_late_startup() {
    const RESUME_ID: &str = "22222222-2222-4222-8222-222222222222";
    const PROVISIONAL_ID: &str = "33333333-3333-4333-8333-333333333333";

    let fixture = Fixture::new();
    fixture.write(&record());
    let resume = fixture.write_transcript(RESUME_ID);
    let provisional = fixture.write_transcript(PROVISIONAL_ID);
    fixture.write_receipt(RESUME_ID, resume, "resume", 1_000_000_000);
    fixture.write_receipt(PROVISIONAL_ID, provisional, "startup", 2_000_000_000);
    let mut current = record();
    current["sessionId"] = serde_json::json!(RESUME_ID);
    fixture.write(&current);

    assert_eq!(
        fixture.binding(),
        Ok(Some(TerminalProviderSession {
            id: RESUME_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Resume,
        }))
    );
}

#[test]
fn inherited_hook_receipt_from_a_forked_process_is_ignored() {
    const FOREIGN_ID: &str = "22222222-2222-4222-8222-222222222222";

    let fixture = Fixture::new();
    fixture.write(&record());
    let leader = fixture.write_transcript(SESSION_ID);
    let foreign = fixture.write_transcript(FOREIGN_ID);
    fixture.write_receipt(SESSION_ID, leader, "startup", 1_000_000_000);
    fixture.write_receipt_for_parent(FOREIGN_ID, foreign, "fork", 2_000_000_000, PID + 1);

    assert_eq!(
        fixture.binding(),
        Ok(Some(TerminalProviderSession {
            id: SESSION_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        }))
    );
}

#[test]
fn orphaned_temporary_does_not_hide_a_receipt_and_reaped_cleanup_removes_it() {
    let fixture = Fixture::new();
    fixture.write(&record());
    let transcript = fixture.write_transcript(SESSION_ID);
    fixture.write_receipt(SESSION_ID, transcript, "startup", 1_000_000_000);
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    let temporary = generation.join(format!(
        "{RECEIPT_TEMP_PREFIX}{}-{}{RECEIPT_TEMP_SUFFIX}",
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4()
    ));
    fs::write(&temporary, b"partial receipt").expect("partial temporary receipt");
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
        .expect("private temporary receipt mode");

    assert_eq!(
        fixture.binding(),
        Ok(Some(TerminalProviderSession {
            id: SESSION_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        }))
    );
    fixture
        .lookup
        .end_capture(TerminalLaunchProfileId::Claude, CAPTURE_TOKEN)
        .expect("retire receipt generation");
    assert!(!generation.exists());
}

#[test]
fn retired_generation_removes_only_its_private_receipts() {
    let fixture = Fixture::new();
    let transcript = fixture.write_transcript(SESSION_ID);
    fixture.write_receipt(SESSION_ID, transcript.clone(), "startup", 1_000_000_000);
    let generation = fixture.receipts.join(CAPTURE_TOKEN);
    assert!(generation.is_dir());

    fixture
        .lookup
        .end_capture(TerminalLaunchProfileId::Claude, CAPTURE_TOKEN)
        .expect("retire receipt generation");

    assert!(!generation.exists());
    assert!(
        transcript.is_file(),
        "provider transcript must be untouched"
    );
}

#[test]
fn a_missing_pid_record_is_not_an_error() {
    let fixture = Fixture::new();
    assert_eq!(fixture.read(&process()), Ok(None));
}

#[test]
fn refuses_a_final_component_symlink() {
    let fixture = Fixture::new();
    let target = fixture.sessions.join("target.json");
    fs::write(&target, serde_json::to_vec(&record()).expect("json")).expect("target");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).expect("private mode");
    symlink(&target, fixture.path()).expect("symlink");

    assert!(fixture.read(&process()).is_err());
}

#[test]
fn refuses_group_or_world_writable_records() {
    let fixture = Fixture::new();
    fixture.write(&record());
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o666)).expect("writable mode");

    assert!(fixture.read(&process()).is_err());
}

#[test]
fn refuses_multiply_linked_records() {
    let fixture = Fixture::new();
    fixture.write(&record());
    fs::hard_link(fixture.path(), fixture.sessions.join("second-link.json")).expect("hard link");

    assert!(fixture.read(&process()).is_err());
}

#[test]
fn refuses_pid_or_process_start_mismatches() {
    let fixture = Fixture::new();
    let mut wrong_pid = record();
    wrong_pid["pid"] = serde_json::json!(PID + 1);
    fixture.write(&wrong_pid);
    assert!(fixture.read(&process()).is_err());

    fixture.write(&record());
    let mismatched_process = TerminalProcessIdentity {
        process_id: PID,
        process_group_id: PID,
        #[cfg(target_os = "macos")]
        terminal_device: Some("ttys999".to_owned()),
        #[cfg(not(target_os = "macos"))]
        terminal_device: None,
        process_start: "Wed Sep 2 03:30:40 2026".to_owned(),
    };
    assert!(fixture.read(&mismatched_process).is_err());
}

#[test]
fn refuses_noncanonical_or_non_v4_session_ids() {
    let fixture = Fixture::new();
    for id in [
        "12345678-1234-3234-8234-123456789abc",
        "12345678-1234-4234-C234-123456789ABC",
        "12345678-1234-4234-7234-123456789abc",
    ] {
        let mut invalid = record();
        invalid["sessionId"] = serde_json::json!(id);
        fixture.write(&invalid);
        assert!(fixture.read(&process()).is_err(), "accepted {id}");
    }
}

#[test]
fn accepts_the_validated_registry_schema_version_family() {
    let fixture = Fixture::new();
    let mut unsupported = record();
    unsupported["version"] = serde_json::json!("2.1.257");
    fixture.write(&unsupported);
    assert!(fixture.read(&process()).is_err());

    for version in ["2.1.258", "2.1.259", "2.2.0"] {
        let mut supported = record();
        supported["version"] = serde_json::json!(version);
        fixture.write(&supported);
        assert_eq!(
            fixture.read(&process()),
            Ok(Some(SESSION_ID.to_owned())),
            "rejected {version}"
        );
    }

    for version in ["3.0.0", "2.1", "2.01.258", "2.1.258-beta"] {
        let mut invalid = record();
        invalid["version"] = serde_json::json!(version);
        fixture.write(&invalid);
        assert!(fixture.read(&process()).is_err(), "accepted {version}");
    }
}

#[test]
fn live_process_snapshot_must_match_the_full_spawn_identity() {
    let mut identity = process();
    #[cfg(target_os = "macos")]
    let matching = format!("{PID} {PID} ttys999 {PROCESS_START}\n");
    #[cfg(not(target_os = "macos"))]
    let matching = format!("{PID} {PID} {PROCESS_START}\n");
    assert_eq!(
        validate_live_process_output(matching.as_bytes(), &identity),
        Ok(())
    );

    identity.process_group_id = PID + 1;
    assert!(validate_live_process_output(matching.as_bytes(), &identity).is_err());
    identity.process_group_id = PID;

    identity.process_start = "Wed Sep 2 03:30:40 2026".to_owned();
    assert!(validate_live_process_output(matching.as_bytes(), &identity).is_err());
}

#[test]
fn pointer_rewrite_during_process_validation_is_rejected() {
    let fixture = Fixture::new();
    fixture.write(&record());
    let path = fixture.path();
    let result = read_current_session_with(&fixture.sessions, &process(), |_| {
        let mut changed = record();
        changed["sessionId"] = serde_json::json!("87654321-4321-4321-8321-cba987654321");
        fs::write(&path, serde_json::to_vec(&changed).expect("json"))
            .map_err(|_| invalid_registry())?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|_| invalid_registry())?;
        Ok(())
    });
    assert!(result.is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn live_process_snapshot_requires_the_same_real_terminal_device() {
    let identity = process();
    let mismatch = format!("{PID} {PID} ttys998 {PROCESS_START}\n");
    assert!(validate_live_process_output(mismatch.as_bytes(), &identity).is_err());

    let missing = format!("{PID} {PID} ?? {PROCESS_START}\n");
    assert!(validate_live_process_output(missing.as_bytes(), &identity).is_err());
}

#[test]
fn lookup_refuses_symbolic_registry_directories() {
    let root = tempfile::tempdir().expect("temp root");
    let actual_config = root.path().join("actual-claude");
    fs::create_dir_all(actual_config.join("sessions")).expect("actual sessions");
    let symbolic_config = root.path().join(".claude");
    symlink(&actual_config, &symbolic_config).expect("config symlink");

    let receipts = root.path().join("receipts");
    fs::create_dir(&receipts).expect("receipts root");
    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o700))
        .expect("private receipts root");
    let lookup = ClaudeTerminalSessionLookup::new(&symbolic_config.join("projects"), &receipts)
        .expect("path-only lookup construction");
    assert!(
        lookup
            .current_session_with(&process(), CAPTURE_TOKEN, |_| Ok(()))
            .is_err()
    );
}

#[test]
fn missing_provider_directories_are_a_retryable_idless_state() {
    let root = tempfile::tempdir().expect("temp root");
    let projects = root.path().join(".claude/projects");
    let receipts = root.path().join("receipts");
    fs::create_dir(&receipts).expect("receipts root");
    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o700))
        .expect("private receipts root");
    let lookup = ClaudeTerminalSessionLookup::new(&projects, &receipts).expect("lookup");
    assert_eq!(
        lookup.current_session_with(&process(), CAPTURE_TOKEN, |_| Ok(())),
        Ok(None)
    );
}

#[test]
fn lookup_refuses_replaced_or_writable_registry_directories() {
    let fixture = Fixture::new();
    fixture.write(&record());
    fs::set_permissions(&fixture.sessions, fs::Permissions::from_mode(0o777))
        .expect("writable sessions");
    assert!(
        fixture
            .lookup
            .current_session_with(&process(), CAPTURE_TOKEN, |_| Ok(()))
            .is_err()
    );
}

#[cfg(target_os = "macos")]
struct StoppedOrphanFixture(TerminalProcessIdentity);
#[cfg(target_os = "macos")]
impl Drop for StoppedOrphanFixture {
    fn drop(&mut self) {
        // Only this fixture's authenticated stopped orphan is eligible.
        let _ = settle_stopped_orphan_process_group(&self.0);
    }
}
#[cfg(target_os = "macos")]
fn stopped_orphan_fixture() -> StoppedOrphanFixture {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let script = r"
import json, os, pty, signal, subprocess, time
pid, master = pty.fork()
if pid == 0:
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    os.execl('/bin/sleep', 'sleep', '60')
try:
    time.sleep(0.1)
    os.kill(pid, signal.SIGSTOP)
    env = dict(os.environ, LC_ALL='C', TZ='UTC')
    fields = subprocess.check_output(['/bin/ps', '-p', str(pid), '-o', 'pgid=,tdev=,lstart='], env=env).decode().split()
    print(json.dumps(dict(process_id=pid, process_group_id=int(fields[0]), terminal_device=fields[1], process_start=' '.join(fields[2:]))), flush=True)
except BaseException:
    os.kill(pid, signal.SIGKILL)
    os.close(master)
    raise
os._exit(0)
";
    let mut parent = Command::new("/usr/bin/python3")
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn disposable PTY supervisor");
    let mut line = String::new();
    BufReader::new(parent.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&line).expect("fixture identity");
    let process = TerminalProcessIdentity {
        process_id: u32::try_from(value["process_id"].as_u64().unwrap()).unwrap(),
        process_group_id: u32::try_from(value["process_group_id"].as_u64().unwrap()).unwrap(),
        terminal_device: Some(value["terminal_device"].as_str().unwrap().to_owned()),
        process_start: value["process_start"].as_str().unwrap().to_owned(),
    };
    let orphan = StoppedOrphanFixture(process);
    assert!(parent.wait().expect("supervisor exit").success());
    orphan
}

#[cfg(target_os = "macos")]
#[test]
fn startup_recovers_real_stopped_orphans_and_preserves_the_session_across_two_cold_starts() {
    let fixture = Fixture::new();
    let saved = TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: Some(SESSION_ID.to_owned()),
        resume: true,
    };
    let transcript = fixture.write_transcript(SESSION_ID);
    for (index, token) in [CAPTURE_TOKEN, "22222222-2222-4222-8222-222222222222"]
        .iter()
        .enumerate()
    {
        let child = stopped_orphan_fixture();
        assert!(captured_process_group_is_live(&child.0).unwrap());
        fixture
            .lookup
            .begin_capture(
                TerminalLaunchProfileId::Claude,
                &TerminalProviderSessionCaptureOwner {
                    capture_token: (*token).to_owned(),
                    terminal_id: terminal_id(),
                    terminal_generation: TerminalGeneration(u64::try_from(index + 1).unwrap()),
                    root: RECEIPT_CWD.to_owned(),
                    cwd: RECEIPT_CWD.to_owned(),
                    prior_launch_session: Some(saved.clone()),
                    nested_shell: false,
                },
            )
            .expect("same saved pane admits its next capture");
        fixture
            .lookup
            .record_process(TerminalLaunchProfileId::Claude, token, &child.0)
            .unwrap();
        let receipt = ClaudeTerminalSessionReceipt {
            version: RECEIPT_VERSION,
            capture_token: (*token).to_owned(),
            parent_process_id: child.0.process_id,
            reported_session_id: SESSION_ID.to_owned(),
            transcript_path: transcript.clone(),
            cwd: RECEIPT_CWD.into(),
            source: "resume".into(),
            captured_at_unix_nanos: 1_000_000_000,
        };
        let path = fixture
            .receipts
            .join(token)
            .join("33333333-3333-4333-8333-333333333333.json");
        fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut observed = Vec::new();
        let sweep = fixture
            .lookup
            .reconcile_startup_receipts(|recovery| {
                observed.push(recovery.launch_session.clone());
                assert_eq!(recovery.terminal_id, terminal_id());
                StartupReceiptDisposition::Durable
            })
            .expect("recover orphan before pane reopen");
        assert_eq!(sweep.recovered_orphans, 1);
        assert_eq!(sweep.retired, 1);
        assert_eq!(sweep.active, 0);
        assert_eq!(sweep.retryable, 0);
        assert_eq!(observed, vec![saved.clone()]);
        assert!(!captured_process_group_is_live(&child.0).unwrap());
        assert!(!fixture.receipts.join(token).exists());
    }
}

#[test]
fn failed_or_declined_orphan_recovery_retains_capture_and_blocks_duplicate_launch() {
    for result in [Ok(false), Err(())] {
        let fixture = Fixture::new();
        fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, None);
        fixture.record_process();
        let sweep = fixture
            .lookup
            .reconcile_startup_receipts_with_recovery(
                |_| panic!("unsettled capture must not be applied"),
                |_| Ok(false),
                |_| result,
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(sweep.recovered_orphans, 0);
        assert_eq!(sweep.retired, 0);
        assert_eq!(sweep.active + sweep.retryable, 1);
        assert!(fixture.receipts.join(CAPTURE_TOKEN).is_dir());
        let mut next = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
            .unwrap()
            .unwrap();
        next.capture_token = "22222222-2222-4222-8222-222222222222".into();
        assert!(
            register_capture_owner(
                &fixture.receipts,
                &TerminalProviderSessionCaptureOwner {
                    capture_token: next.capture_token,
                    terminal_id: next.terminal_id,
                    terminal_generation: TerminalGeneration(2),
                    root: next.root,
                    cwd: next.cwd,
                    prior_launch_session: next.prior_launch_session,
                    nested_shell: false,
                }
            )
            .is_err()
        );
    }
}

#[test]
fn changed_private_capture_owner_declines_orphan_signalling() {
    let fixture = Fixture::new();
    fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, None);
    fixture.record_process();
    let owner_file = fixture
        .receipts
        .join(CAPTURE_TOKEN)
        .join(CAPTURE_OWNER_FILE);
    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with_recovery(
            |_| panic!("changed owner must not be applied"),
            |_| {
                let mut owner = capture_owner(&fixture.receipts, CAPTURE_TOKEN)
                    .unwrap()
                    .unwrap();
                owner.terminal_generation = TerminalGeneration(2);
                fs::write(&owner_file, serde_json::to_vec(&owner).unwrap()).unwrap();
                Ok(false)
            },
            |_| panic!("changed private owner must never authorize a signal"),
            Duration::ZERO,
        )
        .unwrap();
    assert_eq!(sweep.retryable, 1);
    assert_eq!(sweep.retired, 0);
    assert_eq!(sweep.recovered_orphans, 0);
    assert!(fixture.receipts.join(CAPTURE_TOKEN).is_dir());
}

#[test]
fn stopped_orphan_recovery_precedes_supervised_process_grace_checks() {
    let fixture = Fixture::new();
    fixture.begin_capture_at(RECEIPT_CWD, RECEIPT_CWD, None);
    fixture.record_process();
    fixture.write_receipt(
        SESSION_ID,
        fixture.write_transcript(SESSION_ID),
        "startup",
        1,
    );
    let mut probes = 0;
    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with_recovery(
            |_| StartupReceiptDisposition::Durable,
            |_| {
                probes += 1;
                assert_eq!(probes, 1, "an orphan cannot settle during grace checks");
                Ok(false)
            },
            |_| Ok(true),
            Duration::ZERO,
        )
        .unwrap();
    assert_eq!(sweep.recovered_orphans, 1);
    assert_eq!(sweep.retired, 1);
    assert_eq!(sweep.active + sweep.retryable, 0);
}

#[test]
fn owned_capture_adopts_and_reconciles_a_worktree_clear_receipt_and_allows_reopen() {
    const STARTUP_ID: &str = "11111111-1111-4111-8111-111111111111";
    const CLEAR_ID: &str = "22222222-2222-4222-8222-222222222222";
    const SECOND_TOKEN: &str = "33333333-3333-4333-8333-333333333333";
    const LAUNCH_CWD: &str = "/private/tmp/interface-claude-test";

    let fixture = Fixture::new();
    let prior = TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: Some(STARTUP_ID.to_owned()),
        resume: true,
    };
    fixture.begin_capture_at(LAUNCH_CWD, LAUNCH_CWD, Some(prior.clone()));
    fixture.record_process();

    let worktree_cwd = Path::new(RECEIPT_CWD);
    fixture.write_process_record(&process(), CLEAR_ID, worktree_cwd);
    let clear_transcript = fixture.write_transcript_at(worktree_cwd, CLEAR_ID);
    fixture.write_receipt_for_parent_at_cwd(
        CLEAR_ID,
        clear_transcript,
        "clear",
        2_000_000_000,
        PID,
        worktree_cwd,
    );

    assert_eq!(
        fixture.binding(),
        Ok(Some(TerminalProviderSession {
            id: CLEAR_ID.to_owned(),
            transition: TerminalProviderSessionTransition::Clear,
        }))
    );

    let mut recovered = None;
    let sweep = fixture
        .lookup
        .reconcile_startup_receipts_with(
            |candidate| {
                recovered = Some(candidate.clone());
                StartupReceiptDisposition::Durable
            },
            |_| Ok(true),
            Duration::ZERO,
        )
        .expect("reconcile worktree clear receipt");
    assert_eq!(
        sweep,
        StartupReceiptSweep {
            examined: 1,
            retired: 1,
            active: 0,
            retryable: 0,
            recovered_orphans: 0,
        }
    );
    let recovered = recovered.expect("recovered worktree clear session");
    assert_eq!(recovered.cwd, LAUNCH_CWD);
    assert_eq!(
        recovered.launch_session,
        TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: Some(CLEAR_ID.to_owned()),
            resume: true,
        }
    );
    fixture
        .lookup
        .begin_capture(
            TerminalLaunchProfileId::Claude,
            &TerminalProviderSessionCaptureOwner {
                terminal_id: terminal_id(),
                terminal_generation: TerminalGeneration(2),
                capture_token: SECOND_TOKEN.to_owned(),
                root: LAUNCH_CWD.to_owned(),
                cwd: LAUNCH_CWD.to_owned(),
                prior_launch_session: Some(recovered.launch_session),
                nested_shell: false,
            },
        )
        .expect("reopened pane can register a fresh capture generation");
}
