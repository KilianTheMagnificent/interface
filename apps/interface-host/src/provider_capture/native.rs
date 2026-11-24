//! Provider-native hooks, scoped to one host-owned PTY generation.
use super::{
    Deserialize, Duration, JsonLimits, Path, PrivateDirectory, ProviderCaptureLookup, Serialize,
    StoredProcessIdentity, TerminalBackendError, TerminalLaunchProfileId, TerminalProcessIdentity,
    TerminalProviderSession, WriterLock, acquire_writer_lock, atomic_write_json, canonical_uuid_v4,
    fs, invalid_registry, read_capture_owner, read_json, snapshot_target, thread, transition_for,
    verified_owner,
};
use crate::capture_generation::generation_is_sealed;
use crate::capture_hook::{hook_process_lineage, parent_process_id};
use interface_terminal::native_checkpoint as native;
use std::io::{self, Read, Write};

pub(crate) const FILE: &str = ".interface-native-session.json";
const MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    version: u32,
    token: String,
    profile: TerminalLaunchProfileId,
    process: StoredProcessIdentity,
    binding: String,
    source: String,
    activity: native::Activity,
}

fn limits() -> io::Result<JsonLimits> {
    JsonLimits::new(MAX_BYTES, 8, 2048).map_err(io::Error::other)
}

pub(crate) fn read(
    directory: &PrivateDirectory,
    token: &str,
    process: &TerminalProcessIdentity,
) -> Result<Option<(TerminalProviderSession, TerminalLaunchProfileId)>, TerminalBackendError> {
    let receipt = read_json::<Receipt>(directory, FILE, limits().map_err(|_| invalid_registry())?)
        .map_err(|_| invalid_registry())?
        .map(|loaded| loaded.into_parts().0);
    receipt
        .map(|receipt| {
            if receipt.version != 1
                || receipt.token != token
                || receipt.process != StoredProcessIdentity::from(process)
                || native::session_id(receipt.profile, &receipt.binding).is_none()
                || !receipt.activity.valid()
            {
                return Err(invalid_registry());
            }
            Ok((
                TerminalProviderSession {
                    id: receipt.binding,
                    transition: transition_for(&receipt.source).ok_or_else(invalid_registry)?,
                },
                receipt.profile,
            ))
        })
        .transpose()
}

/// The hook's own ancestry, nearest first: the provider process that
/// spawned the hook and up to two of its parents, as (pid, process start).
/// A pane's capture owner matches when it names one of these exactly.
fn hook_ancestry() -> io::Result<Vec<(u32, String)>> {
    let mut ancestry = Vec::new();
    let mut next = parent_process_id().ok();
    for _ in 0..3 {
        let Some(pid) = next else {
            break;
        };
        let Some((identity, parent)) = hook_process_lineage(pid)? else {
            break;
        };
        ancestry.push((identity.process_id, identity.process_start));
        next = parent;
    }
    Ok(ancestry)
}

fn lineage_matches(process: &StoredProcessIdentity, ancestry: &[(u32, String)]) -> bool {
    ancestry
        .iter()
        .any(|(pid, start)| *pid == process.process_id && start == &process.process_start)
}

/// The capture generation under `root` whose filed process is in this
/// hook's ancestry. Plugin hooks carry no per-pane token (Muse filters
/// their environment), so ancestry is the only binding; generations whose
/// owner cannot be read are skipped, and `record` re-validates the chosen
/// owner strictly before anything is written. Hooks go through
/// [`wait_for_lineage_token`]; this single scan stays for the tests.
#[cfg(test)]
fn lineage_token_in(
    root: &PrivateDirectory,
    ancestry: &[(u32, String)],
) -> io::Result<Option<String>> {
    Ok(scan_lineage(root, ancestry)?.0)
}

/// [`lineage_token_in`], plus whether some unsealed generation's owner has
/// no process filed yet: the only case in which a later scan can find more.
fn scan_lineage(
    root: &PrivateDirectory,
    ancestry: &[(u32, String)],
) -> io::Result<(Option<String>, bool)> {
    let generations = super::bounded_generation_names(root).map_err(io::Error::other)?;
    let mut pending = false;
    for token in generations {
        let Ok(directory) = root.open_child(&token) else {
            continue;
        };
        let Ok(Some(owner)) = read_capture_owner(&directory, &token) else {
            continue;
        };
        match owner.process.as_ref() {
            Some(process) if lineage_matches(process, ancestry) => {
                return Ok((Some(token), pending));
            }
            None if !owner.nested_shell => {
                pending = pending || !generation_is_sealed(&directory).unwrap_or(true);
            }
            None | Some(_) => {}
        }
    }
    Ok((None, pending))
}

/// The PTY may reach its first hook before spawn has filed the child PID,
/// so a lineage hook re-scans while some generation in this channel still
/// lacks its process, up to 50 times. Otherwise one scan settles it: a hook
/// from another channel's pane, or from outside any pane, never waits
/// (each hook used to pause about half a second per channel).
fn wait_for_lineage_token(
    root: &PrivateDirectory,
    ancestry: &[(u32, String)],
    mut pause: impl FnMut(),
) -> io::Result<Option<String>> {
    for attempt in 0..50 {
        let (found, pending) = scan_lineage(root, ancestry)?;
        if found.is_some() || !pending || attempt == 49 {
            return Ok(found);
        }
        pause();
    }
    Ok(None)
}

/// No hook output enters the model's context or changes its permissions.
/// The activity sequence is written to the hook's controlling PTY only after
/// the pane's exact process identity and private generation have been checked.
///
/// # Errors
/// Returns an error when a private receipt or the controlling PTY is unavailable.
pub fn run(
    profile: TerminalLaunchProfileId,
    name: &str,
    token: &str,
    root: &Path,
    input: impl Read,
) -> io::Result<()> {
    // Muse plugin hooks pass the lineage sentinel: they reload for every
    // session (unlike run-scoped `--config` hooks, which a `/clear` drops),
    // but their filtered environment carries no capture token.
    let lineage = profile == TerminalLaunchProfileId::Muse && token == native::LINEAGE_TOKEN;
    if !native::supports(profile) || (!lineage && !canonical_uuid_v4(token)) || !root.is_absolute()
    {
        return Ok(());
    }
    let mut bytes = Vec::new();
    input.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        return Ok(());
    }
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(());
    };
    // Muse explicitly distinguishes its root chat from a child thread. A
    // child's own Stop must never report that the containing pane is idle.
    if profile == TerminalLaunchProfileId::Muse
        && let (Ok(root_id), Ok(thread_id)) = (
            std::env::var("MUSE_SESSION_ID"),
            std::env::var("MUSE_THREAD_ID"),
        )
        && root_id != thread_id
    {
        if name != "SessionEnd" {
            return Ok(());
        }
        value["session_id"] = serde_json::Value::String(root_id);
        value["child_session_id"] = serde_json::Value::String(thread_id);
    }
    let Some(event) = native::event(profile, name, &value) else {
        return Ok(());
    };
    let root = PrivateDirectory::open(root).map_err(io::Error::other)?;
    let ancestry = hook_ancestry()?;
    // The PTY may reach its first hook before spawn has filed the child PID.
    // Wait without the writer lock, which record_process itself needs. A
    // lineage hook waits by re-scanning for its generation to file one.
    let resolved;
    let token = if lineage {
        let found = wait_for_lineage_token(&root, &ancestry, || {
            thread::sleep(Duration::from_millis(10));
        })?;
        let Some(found) = found else {
            return Ok(());
        };
        resolved = found;
        resolved.as_str()
    } else {
        token
    };
    let directory = root.open_child(token).map_err(io::Error::other)?;
    if !lineage {
        for _ in 0..50 {
            if read_capture_owner(&directory, token)
                .map_err(io::Error::other)?
                .is_some_and(|owner| owner.process.is_some())
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    let writer = acquire_writer_lock(&directory)?;
    let result = record(
        &directory, &writer, profile, name, token, event, &value, &ancestry,
    );
    let released = writer.release(&directory).map_err(io::Error::other);
    let activity = result?;
    released?;
    if let Some(activity) = activity {
        // stdout belongs to the hook protocol; /dev/tty is this pane alone.
        if let Ok(mut tty) = fs::OpenOptions::new().write(true).open("/dev/tty") {
            write!(tty, "\x1b]2;{}{activity}\x07", native::ACTIVITY_PREFIX)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record(
    directory: &PrivateDirectory,
    writer: &WriterLock,
    profile: TerminalLaunchProfileId,
    name: &str,
    token: &str,
    event: native::HookEvent,
    value: &serde_json::Value,
    ancestry: &[(u32, String)],
) -> io::Result<Option<&'static str>> {
    if generation_is_sealed(directory)? {
        return Ok(None);
    }
    let Some(owner) = read_capture_owner(directory, token).map_err(io::Error::other)? else {
        return Ok(None);
    };
    let Some(process) = owner.process else {
        return Ok(None);
    };
    // A child may finish in its own isolated worktree. Its root-session ID
    // and the pane process ancestry still bind that completion to this pane.
    let child_end = name == "SessionEnd" && value.get("child_session_id").is_some();
    // Kimi reports no cwd at all; its token and ancestry bind the pane.
    if !child_end
        && profile != TerminalLaunchProfileId::Kimi
        && !event.cwd.iter().any(|cwd| {
            Path::new(cwd).starts_with(&owner.cwd) || Path::new(&owner.cwd).starts_with(cwd)
        })
    {
        return Ok(None);
    }
    if !lineage_matches(&process, ancestry) {
        return Ok(None);
    }
    let mut receipt = read_json::<Receipt>(directory, FILE, limits()?)
        .map_err(io::Error::other)?
        .map_or_else(
            || Receipt {
                version: 1,
                token: token.into(),
                profile,
                process: process.clone(),
                binding: event.binding.clone(),
                source: event.source.clone(),
                activity: native::Activity::default(),
            },
            |loaded| loaded.into_parts().0,
        );
    if receipt.version != 1
        || receipt.token != token
        || receipt.profile != profile
        || receipt.process != process
    {
        return Ok(None);
    }
    if receipt.binding != event.binding {
        // A child startup cannot move the root binding. Explicit Muse
        // clear/resume/fork transitions may; unknown transitions stay busy.
        // Kimi's `/new` (`startup`) and session switch (`resume`) open
        // another session in the same TUI.
        let rebinds = match profile {
            TerminalLaunchProfileId::Gemini => true,
            TerminalLaunchProfileId::Kimi => {
                matches!(event.source.as_str(), "startup" | "resume")
            }
            _ => matches!(event.source.as_str(), "clear" | "resume" | "fork"),
        };
        if name != "SessionStart" || !rebinds {
            return Ok(Some("working"));
        }
        receipt.binding = event.binding;
        receipt.source = event.source;
        receipt.activity = native::Activity::default();
    }
    let activity = receipt.activity.observe(
        profile,
        name,
        event.activity,
        value
            .get("child_session_id")
            .and_then(serde_json::Value::as_str),
    );
    let expected = snapshot_target(directory, FILE, MAX_BYTES).map_err(io::Error::other)?;
    atomic_write_json(directory, writer, FILE, &receipt, &expected, limits()?)
        .map_err(io::Error::other)?;
    Ok(Some(activity))
}

impl ProviderCaptureLookup {
    pub(super) fn native_session_with<F>(
        &self,
        process: &TerminalProcessIdentity,
        token: &str,
        validate: F,
    ) -> Result<Option<(TerminalProviderSession, TerminalLaunchProfileId)>, TerminalBackendError>
    where
        F: Fn(&TerminalProcessIdentity) -> Result<(), TerminalBackendError>,
    {
        verified_owner(&self.receipts, token, process, true)?.ok_or_else(invalid_registry)?;
        validate(process)?;
        let root = PrivateDirectory::open(&self.receipts).map_err(|_| invalid_registry())?;
        let directory = root.open_child(token).map_err(|_| invalid_registry())?;
        let result = read(&directory, token, process)?;
        validate(process)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_receipts_cannot_cross_panes_generations_or_provider_kinds() {
        let temp = tempfile::tempdir().unwrap();
        let directory = PrivateDirectory::open_or_create(temp.path().join("receipts")).unwrap();
        let writer = acquire_writer_lock(&directory).unwrap();
        let process = TerminalProcessIdentity {
            process_id: 123,
            process_group_id: 123,
            process_start: "Tue Oct 20 10:00:00 2026".into(),
            terminal_device: Some("ttys001".into()),
        };
        let mut receipt = Receipt {
            version: 1,
            token: "d2d9dfef-c119-4df6-8827-2bad4e79eda6".into(),
            profile: TerminalLaunchProfileId::Muse,
            process: StoredProcessIdentity::from(&process),
            binding: "muse-01a15add-7a55-74d1-bf33-5d8d5502c449".into(),
            source: "resume".into(),
            activity: native::Activity::default(),
        };
        let write = |receipt: &Receipt| {
            let snapshot = snapshot_target(&directory, FILE, MAX_BYTES).unwrap();
            atomic_write_json(
                &directory,
                &writer,
                FILE,
                receipt,
                &snapshot,
                limits().unwrap(),
            )
            .unwrap();
        };
        write(&receipt);
        assert_eq!(
            read(&directory, &receipt.token, &process)
                .unwrap()
                .unwrap()
                .0
                .id,
            receipt.binding
        );
        assert!(read(&directory, "another-generation", &process).is_err());
        let mut reused = process.clone();
        reused.process_start = "later process".into();
        assert!(read(&directory, &receipt.token, &reused).is_err());
        reused = process.clone();
        reused.process_id = 124;
        assert!(read(&directory, &receipt.token, &reused).is_err());
        receipt.profile = TerminalLaunchProfileId::Gemini;
        write(&receipt);
        assert!(read(&directory, &receipt.token, &process).is_err());
        writer.release(&directory).unwrap();
    }

    // Muse plugin hooks run in a filtered environment that carries no
    // per-pane token, so the hook hands the host the lineage sentinel and
    // the host must find its own generation by process ancestry alone.
    #[test]
    fn lineage_scan_selects_the_generation_whose_process_matches() {
        use super::super::{
            CAPTURE_OWNER_VERSION, StoredCaptureOwner, TerminalGeneration, TerminalId,
            write_capture_owner,
        };
        let temp = tempfile::tempdir().unwrap();
        let root = PrivateDirectory::open_or_create(temp.path().join("receipts")).unwrap();
        let process = |pid: u32| {
            StoredProcessIdentity::from(&TerminalProcessIdentity {
                process_id: pid,
                process_group_id: pid,
                process_start: "Tue Oct 20 10:00:00 2026".into(),
                terminal_device: Some("ttys001".into()),
            })
        };
        let own = |token: &str, process: Option<StoredProcessIdentity>| {
            let generation = root.open_or_create_child(token).unwrap();
            let writer = acquire_writer_lock(&generation).unwrap();
            let owner = StoredCaptureOwner {
                version: CAPTURE_OWNER_VERSION,
                capture_token: token.into(),
                terminal_id: TerminalId::new(
                    uuid::Uuid::parse_str("b45742b6-1234-4abc-8def-0123456789ab").unwrap(),
                ),
                terminal_generation: TerminalGeneration::initial(),
                root: "/tmp/project".into(),
                cwd: "/tmp/project".into(),
                prior_launch_session: None,
                nested_shell: false,
                nested_replacement: false,
                process,
            };
            write_capture_owner(&generation, &writer, &owner).unwrap();
            writer.release(&generation).unwrap();
        };
        // An ownerless generation (registered, process not filed yet is the
        // second case), the pane this hook descends from, and a foreign pane.
        root.open_or_create_child("0fbdf625-e72a-4691-85a8-049e03e57053")
            .unwrap();
        own("7ee25969-e32c-444e-97a8-f4832047679b", Some(process(4242)));
        own("a4945528-9b44-4cc5-aa1a-6d9091759793", None);
        own("e28c0704-3be6-444a-9ec2-f7398d552864", Some(process(7777)));
        let ancestry = [(4242, "Tue Oct 20 10:00:00 2026".to_owned())];
        assert_eq!(
            lineage_token_in(&root, &ancestry).unwrap().as_deref(),
            Some("7ee25969-e32c-444e-97a8-f4832047679b")
        );
        // The same pid with a different start is a reused pid, not this pane.
        let reused = [(4242, "Wed Oct 21 10:00:00 2026".to_owned())];
        assert_eq!(lineage_token_in(&root, &reused).unwrap(), None);
        assert_eq!(lineage_token_in(&root, &[]).unwrap(), None);
    }

    // Every Muse hook runs each channel's host in lineage mode. A host whose
    // channel does not own the pane (or any host, outside Interface) used to
    // re-scan 50 times, 10 ms apart, before giving up; Muse waited on every
    // hook. Only a generation whose process is not filed yet may still match.
    #[test]
    fn lineage_hook_waits_only_while_a_process_is_not_yet_filed() {
        use super::super::{
            CAPTURE_OWNER_VERSION, StoredCaptureOwner, TerminalGeneration, TerminalId,
            write_capture_owner,
        };
        use crate::capture_generation::{RECEIPT_SEALED, create_new_private_file};
        let temp = tempfile::tempdir().unwrap();
        let root = PrivateDirectory::open_or_create(temp.path().join("receipts")).unwrap();
        let process = |pid: u32| {
            StoredProcessIdentity::from(&TerminalProcessIdentity {
                process_id: pid,
                process_group_id: pid,
                process_start: "Tue Oct 20 10:00:00 2026".into(),
                terminal_device: Some("ttys001".into()),
            })
        };
        let own = |token: &str, process: Option<StoredProcessIdentity>| {
            let generation = root.open_or_create_child(token).unwrap();
            let writer = acquire_writer_lock(&generation).unwrap();
            let owner = StoredCaptureOwner {
                version: CAPTURE_OWNER_VERSION,
                capture_token: token.into(),
                terminal_id: TerminalId::new(
                    uuid::Uuid::parse_str("b45742b6-1234-4abc-8def-0123456789ab").unwrap(),
                ),
                terminal_generation: TerminalGeneration::initial(),
                root: "/tmp/project".into(),
                cwd: "/tmp/project".into(),
                prior_launch_session: None,
                nested_shell: false,
                nested_replacement: false,
                process,
            };
            write_capture_owner(&generation, &writer, &owner).unwrap();
            writer.release(&generation).unwrap();
        };
        let ancestry = [(4242, "Tue Oct 20 10:00:00 2026".to_owned())];

        // No pane at all, then only another pane whose process is filed, and
        // a finished generation that never filed one: one scan, no pause.
        let mut pauses = 0;
        assert_eq!(
            wait_for_lineage_token(&root, &ancestry, || pauses += 1).unwrap(),
            None
        );
        own("e28c0704-3be6-444a-9ec2-f7398d552864", Some(process(7777)));
        let sealed = "0fbdf625-e72a-4691-85a8-049e03e57053";
        own(sealed, None);
        create_new_private_file(&root.path().join(sealed).join(RECEIPT_SEALED)).unwrap();
        assert_eq!(
            wait_for_lineage_token(&root, &ancestry, || pauses += 1).unwrap(),
            None
        );
        assert_eq!(pauses, 0);

        // This hook's own pane, whose process is filed a moment after the
        // hook starts: the wait the scan exists for still finds it.
        let mine = "7ee25969-e32c-444e-97a8-f4832047679b";
        own(mine, None);
        let found = wait_for_lineage_token(&root, &ancestry, || {
            pauses += 1;
            if pauses == 3 {
                own(mine, Some(process(4242)));
            }
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some(mine));
        assert_eq!(pauses, 3);

        // A process that is never filed is waited for, but not forever.
        own(mine, None);
        pauses = 0;
        assert_eq!(
            wait_for_lineage_token(&root, &ancestry, || pauses += 1).unwrap(),
            None
        );
        assert_eq!(pauses, 49);
    }

    #[test]
    fn lineage_hook_does_not_wait_for_open_manual_shell_panes() {
        use super::super::{
            CAPTURE_OWNER_VERSION, StoredCaptureOwner, TerminalGeneration, TerminalId,
            write_capture_owner,
        };
        let temp = tempfile::tempdir().unwrap();
        let root = PrivateDirectory::open_or_create(temp.path().join("receipts")).unwrap();
        let shell_token = "9b2a14c7-5e31-4f82-9a10-6c4d8e2f1b05";
        let generation = root.open_or_create_child(shell_token).unwrap();
        let writer = acquire_writer_lock(&generation).unwrap();
        write_capture_owner(
            &generation,
            &writer,
            &StoredCaptureOwner {
                version: CAPTURE_OWNER_VERSION,
                capture_token: shell_token.into(),
                terminal_id: TerminalId::new(
                    uuid::Uuid::parse_str("b45742b6-1234-4abc-8def-0123456789ab").unwrap(),
                ),
                terminal_generation: TerminalGeneration::initial(),
                root: "/tmp/project".into(),
                cwd: "/tmp/project".into(),
                prior_launch_session: None,
                nested_shell: true,
                nested_replacement: false,
                process: None,
            },
        )
        .unwrap();
        writer.release(&generation).unwrap();

        let ancestry = [(4242, "Tue Oct 20 10:00:00 2026".to_owned())];
        let mut pauses = 0;
        assert_eq!(
            wait_for_lineage_token(&root, &ancestry, || pauses += 1).unwrap(),
            None
        );
        assert_eq!(pauses, 0);
    }

    // Kimi Code's hooks carry no cwd and report every agent under the root
    // session id; its `/new` (`startup`) moves the pane to the new session.
    #[test]
    fn kimi_receipts_bind_by_token_and_lineage_and_follow_a_new_session() {
        use super::super::{
            CAPTURE_OWNER_VERSION, StoredCaptureOwner, TerminalGeneration, TerminalId,
            write_capture_owner,
        };
        let temp = tempfile::tempdir().unwrap();
        let root = PrivateDirectory::open_or_create(temp.path().join("receipts")).unwrap();
        let token = "7ee25969-e32c-444e-97a8-f4832047679b";
        let generation = root.open_or_create_child(token).unwrap();
        let identity = TerminalProcessIdentity {
            process_id: 4242,
            process_group_id: 4242,
            process_start: "Tue Oct 20 10:00:00 2026".into(),
            terminal_device: Some("ttys001".into()),
        };
        let writer = acquire_writer_lock(&generation).unwrap();
        write_capture_owner(
            &generation,
            &writer,
            &StoredCaptureOwner {
                version: CAPTURE_OWNER_VERSION,
                capture_token: token.into(),
                terminal_id: TerminalId::new(
                    uuid::Uuid::parse_str("b45742b6-1234-4abc-8def-0123456789ab").unwrap(),
                ),
                terminal_generation: TerminalGeneration::initial(),
                root: "/tmp/project".into(),
                cwd: "/tmp/project".into(),
                prior_launch_session: None,
                nested_shell: false,
                nested_replacement: false,
                process: Some(StoredProcessIdentity::from(&identity)),
            },
        )
        .unwrap();
        let pane = [(4242, "Tue Oct 20 10:00:00 2026".to_owned())];
        let observe = |name: &str, id: &str, source: Option<&str>, ancestry: &[(u32, String)]| {
            let mut value =
                serde_json::json!({"hook_event_name": name, "session_id": format!("session_{id}")});
            if let Some(source) = source {
                value["source"] = source.into();
            }
            let event = native::event(TerminalLaunchProfileId::Kimi, name, &value).unwrap();
            record(
                &generation,
                &writer,
                TerminalLaunchProfileId::Kimi,
                name,
                token,
                event,
                &value,
                ancestry,
            )
            .unwrap()
        };
        let bound = || read(&generation, token, &identity).unwrap().unwrap();
        let first = "3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73";
        let foreign = [(7777, "Tue Oct 20 10:00:00 2026".to_owned())];
        assert_eq!(
            observe("SessionStart", first, Some("startup"), &foreign),
            None
        );
        assert!(read(&generation, token, &identity).unwrap().is_none());
        assert_eq!(
            observe("SessionStart", first, Some("startup"), &pane),
            Some("idle")
        );
        assert_eq!(
            observe("UserPromptSubmit", first, None, &pane),
            Some("working")
        );
        assert_eq!(observe("Stop", first, None, &pane), Some("complete"));
        assert_eq!(bound().0.id, format!("kimi-{first}"));
        assert_eq!(bound().1, TerminalLaunchProfileId::Kimi);
        let second = "0a8d4c55-1f3e-4a8b-9c2d-6e7f8a9b0c1d";
        assert_eq!(
            observe("Stop", second, None, &pane),
            Some("working"),
            "only a session start moves the binding"
        );
        assert_eq!(bound().0.id, format!("kimi-{first}"));
        assert_eq!(
            observe("SessionStart", second, Some("startup"), &pane),
            Some("idle")
        );
        assert_eq!(bound().0.id, format!("kimi-{second}"));
        writer.release(&generation).unwrap();
    }
}
