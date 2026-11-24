//! Desktop-only administration of the remote service; HTTP never dispatches IPC.
use crate::{DispatchError, HostDispatcher, TerminalHostDispatcher};
use async_trait::async_trait;
use interface_contracts::{
    agents::AgentControlResponse,
    engine::{SessionSummary, session_key},
    harness::AttentionTarget,
    ipc::{CHANNELS, HistoryAvailability},
    remote::RemoteAccessRequest,
    remote_connection::{ConnectionAdminRequest, PairingRequestV6},
    remote_threads::{
        REMOTE_THREADS_VERSION, RemoteInboxItemV2, RemoteInboxV2, RemoteThreadApprovalV1,
        RemoteThreadDetailV1, RemoteThreadStatus, RemoteThreadV1, RemoteThreadsV1,
    },
    remote_trust::TrustAdminRequest,
    terminal::{TerminalSnapshot, TerminalSummary},
    threads::ThreadsView,
};
use interface_remote::{
    InboxFuture, RemoteBackend, RemoteError, RemoteServer, ThreadAnswerFuture, ThreadDetailFuture,
    ThreadsFuture, replay::StreamKey, threads as phone,
};
use interface_runtime::SessionRuntime;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

pub const REMOTE_OPERATIONS: [&str; 1] = [CHANNELS.remote_access];

#[derive(Default)]
pub struct RemoteHostDispatcher {
    server: Option<RemoteServer>,
}

/// What the phone's Threads (review D6) reach on the Mac: the Threads view,
/// the guarded runtime owner the desktop's own approvals go through, and who
/// else drives a provider session.
pub struct PhoneThreadPorts {
    pub threads: crate::threads::ThreadHost,
    pub runtime: Arc<dyn HostDispatcher>,
    pub claims: Arc<interface_runtime::SessionClaims>,
}

impl RemoteHostDispatcher {
    /// The phone's Control assistant route stays served as wire protocol and
    /// answers `voice-control-unavailable` (the `RemoteBackend` default): the
    /// Control assistant was retired on 2026-10-14.
    #[must_use]
    pub fn new(
        terminal: Arc<TerminalHostDispatcher>,
        runtime: Arc<SessionRuntime>,
        assets: PathBuf,
        threads: Option<PhoneThreadPorts>,
    ) -> Self {
        let hub = terminal.remote_hub.clone();
        let store = match terminal.desktop.open_remote_trust() {
            Ok(store) => store,
            Err(error) => {
                tracing::warn!(%error, "remote access unavailable: trust store did not open");
                return Self::default();
            }
        };
        if let Err(error) = hub.configure_trust(Arc::new(TrustStore(store))) {
            tracing::warn!(
                code = error.code,
                message = error.message,
                "remote access unavailable: trust could not be configured"
            );
            return Self::default();
        }
        let connection_store = match terminal.desktop.open_phone_connection() {
            Ok(store) => store,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "remote access unavailable: phone connection store did not open"
                );
                return Self::default();
            }
        };
        let profile = if cfg!(debug_assertions) {
            interface_contracts::remote_connection::MacProfile::Development
        } else {
            interface_contracts::remote_connection::MacProfile::Production
        };
        let backend: Arc<dyn RemoteBackend> = Arc::new(CompanionBackend {
            terminal,
            runtime,
            threads,
        });
        let server = RemoteServer::new(hub, backend, assets)
            .with_connection_store(Arc::new(ConnectionStore(connection_store)), profile)
            .inspect_err(|error| {
                tracing::warn!(
                    code = error.code,
                    message = error.message,
                    "remote access unavailable: the remote server could not be built"
                );
            })
            .ok();
        Self { server }
    }

    pub async fn restore(&self) {
        if let Some(server) = &self.server {
            if let Err(error) = server.restore().await {
                tracing::warn!(
                    code = error.code,
                    message = error.message,
                    "Remembered remote access could not start. Review iPhone remote access in Settings."
                );
            }
        }
    }

    pub async fn close(&self) {
        if let Some(server) = &self.server {
            server.stop().await;
        }
    }
}

struct CompanionBackend {
    terminal: Arc<TerminalHostDispatcher>,
    runtime: Arc<SessionRuntime>,
    threads: Option<PhoneThreadPorts>,
}

fn thread_error(code: &'static str, message: &'static str) -> RemoteError {
    RemoteError::new(code, message, true)
}

impl CompanionBackend {
    fn terminal(&self, key: StreamKey) -> Result<TerminalSummary, RemoteError> {
        terminal_at(self.terminal.snapshot()?, key)
    }

    fn thread_ports(&self) -> Result<&PhoneThreadPorts, RemoteError> {
        self.threads.as_ref().ok_or_else(|| {
            thread_error(
                "threads-unavailable",
                "Threads are unavailable on this Mac.",
            )
        })
    }

    async fn threads_view(&self) -> Result<ThreadsView, RemoteError> {
        self.thread_ports()?.threads.view().await.map_err(|_| {
            thread_error(
                "threads-unavailable",
                "Your Mac's Threads could not be read. Try again.",
            )
        })
    }

    /// The registry rows of the view's threads, read under the registry lock.
    fn thread_sessions(
        &self,
        view: &ThreadsView,
    ) -> Result<HashMap<String, SessionSummary>, RemoteError> {
        let keys: HashSet<&str> = view.threads.iter().map(|t| t.key.as_str()).collect();
        crate::registry_access::read(&self.runtime, |registry| {
            registry
                .summaries()
                .filter_map(|session| {
                    let key = session_key(&session.engine, &session.session_id);
                    keys.contains(key.as_str()).then(|| (key, session.clone()))
                })
                .collect()
        })
        .map_err(|_| {
            thread_error(
                "sessions-unavailable",
                "Your Mac's session list could not be read.",
            )
        })
    }

    /// An authorized Thread or roster-bound Agent session. Display pagination
    /// never revokes an open page; hidden workers and archived sessions stay out.
    async fn listed_thread(&self, key: &str) -> Result<RemoteThreadV1, RemoteError> {
        self.inbox_view()
            .await?
            .items
            .into_iter()
            .filter_map(|item| item.thread)
            .find(|thread| thread.key == key)
            .ok_or_else(|| {
                thread_error(
                    "thread-not-listed",
                    "That thread is no longer on your phone's list. Go back and refresh.",
                )
            })
    }

    /// Full authorized membership. Only the public list response is truncated.
    async fn inbox_view(&self) -> Result<RemoteInboxV2, RemoteError> {
        let view = self.threads_view().await?;
        let sessions = crate::registry_access::read(&self.runtime, |registry| {
            registry
                .summaries()
                .map(|session| {
                    (
                        session_key(&session.engine, &session.session_id),
                        session.clone(),
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .map_err(|_| {
            thread_error(
                "sessions-unavailable",
                "Your Mac's session list could not be read.",
            )
        })?;
        let agents = self
            .terminal
            .desktop
            .agent_control_response()
            .ok()
            .and_then(|value| serde_json::from_value::<AgentControlResponse>(value).ok());
        Ok(phone::authorized_inbox(&view, &sessions, agents.as_ref()))
    }

    /// The approval a live session is parked on, from its retained events.
    async fn pending_approval(&self, key: &str) -> Option<RemoteThreadApprovalV1> {
        let session = self.runtime.history_session(key).await.ok().flatten()?;
        phone::pending_approval(session.events.iter().map(|event| &event.event))
    }
}

/// The exact terminal generation a phone request names; anything else has changed.
fn terminal_at(snapshot: TerminalSnapshot, key: StreamKey) -> Result<TerminalSummary, RemoteError> {
    snapshot
        .terminals
        .into_iter()
        .find(|terminal| terminal.id.as_uuid() == key.0 && terminal.generation.0 == key.1)
        .ok_or_else(|| RemoteError::new("stale-terminal", "This terminal has changed.", true))
}

/// Whether saved task evidence belongs to one terminal generation, directly or
/// through the provider session that terminal is bound to.
fn task_for_terminal(
    target: &AttentionTarget,
    terminal_id: &str,
    generation: u64,
    binding: Option<&String>,
) -> bool {
    match target {
        AttentionTarget::Terminal {
            terminal_id: target,
            generation: at,
        } => target == terminal_id && *at == generation,
        AttentionTarget::Session { key } => binding == Some(key),
    }
}

impl RemoteBackend for CompanionBackend {
    fn snapshot(
        &self,
    ) -> Result<interface_contracts::terminal::TerminalSnapshot, interface_remote::RemoteError>
    {
        self.terminal.snapshot()
    }
    fn workspaces(
        &self,
    ) -> Result<
        Vec<interface_contracts::terminal::TerminalWorkspaceRecord>,
        interface_remote::RemoteError,
    > {
        self.terminal.workspaces()
    }
    fn session_projects(
        &self,
        keys: &std::collections::BTreeSet<&str>,
    ) -> Result<interface_remote::workspaces::SessionProjects, interface_remote::RemoteError> {
        // Indexed under the registry lock: no row is copied or sorted.
        crate::registry_access::read(&self.runtime, |registry| {
            interface_remote::workspaces::SessionProjects::index(
                registry.summaries().filter(|session| !session.archived),
                keys,
            )
        })
        .map_err(|_| {
            interface_remote::RemoteError::new(
                "sessions-unavailable",
                "Your Mac's session list could not be read.",
                true,
            )
        })
    }
    fn enqueue(
        &self,
        key: interface_remote::replay::StreamKey,
        bytes: Vec<u8>,
    ) -> Result<(), interface_remote::RemoteError> {
        self.terminal.enqueue(key, bytes)
    }
    fn draft(
        &self,
        key: interface_remote::replay::StreamKey,
        edit: Option<(u64, String)>,
    ) -> Result<interface_contracts::harness::SharedDraft, interface_remote::RemoteError> {
        use interface_contracts::harness::SharedDraft;
        let terminal = self.terminal(key)?;
        let target = AttentionTarget::Terminal {
            terminal_id: terminal.id.to_string(),
            generation: key.1,
        };
        // A poll reads only this target's draft; a save still commits the
        // whole harness store, which is what makes it durable.
        let draft = if let Some((revision, text)) = edit {
            self.terminal
                .desktop
                .harness_save_draft(target.clone(), revision, text)
                .map(|state| state.drafts.into_iter().find(|draft| draft.target == target))
        } else {
            self.terminal.desktop.harness_draft(&target)
        }
        .map_err(|_| interface_remote::RemoteError::new("draft-conflict", "Draft changed elsewhere or could not be saved. Reload the saved draft before sharing edits.", true))?;
        Ok(draft.unwrap_or(SharedDraft {
            target,
            revision: 0,
            text: String::new(),
        }))
    }
    fn results(
        &self,
        key: interface_remote::replay::StreamKey,
    ) -> Result<Vec<interface_contracts::harness::TaskEvidence>, interface_remote::RemoteError>
    {
        let terminal = self.terminal(key)?;
        let binding = interface_remote::workspaces::history_key(&terminal);
        let terminal_id = terminal.id.to_string();
        self.terminal
            .desktop
            .harness_recent_tasks(
                |task| task_for_terminal(&task.target, &terminal_id, key.1, binding.as_ref()),
                20,
            )
            .map_err(|_| {
                interface_remote::RemoteError::new(
                    "results-unavailable",
                    "Task results could not be read.",
                    true,
                )
            })
    }
    fn threads(&self) -> ThreadsFuture<'_> {
        Box::pin(async move {
            let view = self.threads_view().await?;
            let sessions = self.thread_sessions(&view)?;
            Ok(RemoteThreadsV1 {
                version: REMOTE_THREADS_VERSION,
                threads: phone::phone_threads(&view, &sessions),
            })
        })
    }
    fn inbox(&self) -> InboxFuture<'_> {
        Box::pin(async move { self.inbox_view().await.map(phone::inbox_page) })
    }
    fn thread_detail(&self, key: String) -> ThreadDetailFuture<'_> {
        Box::pin(async move {
            let thread = self.listed_thread(&key).await?;
            let (turns, older_turns, unavailable) =
                match self.runtime.read_provider_history(&key, 500).await {
                    Ok(page) if page.availability != Some(HistoryAvailability::Unavailable) => {
                        let (turns, older) = phone::thread_turns(&page);
                        (turns, older, None)
                    }
                    Ok(page) => (
                        Vec::new(),
                        0,
                        Some(page.reason.unwrap_or_else(|| {
                            "This thread's turns are only on your Mac.".to_owned()
                        })),
                    ),
                    Err(_) => (
                        Vec::new(),
                        0,
                        Some(
                            "Recent turns could not be read. Read this thread on your Mac."
                                .to_owned(),
                        ),
                    ),
                };
            let approval = if thread.status == RemoteThreadStatus::NeedsYou {
                self.pending_approval(&key).await
            } else {
                None
            };
            Ok(RemoteThreadDetailV1 {
                version: REMOTE_THREADS_VERSION,
                thread,
                turns,
                older_turns,
                approval,
                unavailable,
            })
        })
    }
    fn answer_thread(
        &self,
        key: String,
        approval_id: String,
        allow: bool,
    ) -> ThreadAnswerFuture<'_> {
        Box::pin(async move {
            let ports = self.thread_ports()?;
            let thread = self.listed_thread(&key).await?;
            let current = self.pending_approval(&key).await;
            answer_gate(
                &thread,
                ports.claims.claim(&key).is_some(),
                current.as_ref(),
                &approval_id,
            )?;
            let request = serde_json::json!({
                "sessionKey": key,
                "approvalId": approval_id,
                "verdict": if allow { "allow" } else { "deny" },
            });
            match ports.runtime.dispatch(CHANNELS.respond, request).await {
                Ok(_) => Ok(()),
                Err(error) => Err(answer_refusal(&error)),
            }
        })
    }
    fn send_to_thread(&self, key: String, text: String) -> ThreadAnswerFuture<'_> {
        Box::pin(async move {
            let ports = self.thread_ports()?;
            let item = self
                .inbox_view()
                .await?
                .items
                .into_iter()
                .find(|item| item.thread.as_ref().is_some_and(|thread| thread.key == key))
                .ok_or_else(|| {
                    thread_error(
                        "thread-not-listed",
                        "This session is no longer in the inbox.",
                    )
                })?;
            send_item_gate(&item, ports.claims.claim(&key).is_some())?;
            let request = serde_json::json!({ "sessionKey": key, "text": text });
            match ports.runtime.dispatch(CHANNELS.send, request).await {
                Ok(_) => Ok(()),
                Err(error) => Err(send_refusal(&error)),
            }
        })
    }
    fn history(&self, key: String, limit: u64) -> interface_remote::HistoryFuture<'_> {
        Box::pin(async move {
            self.runtime
                .read_provider_history(&key, limit)
                .await
                .map_err(|_| {
                    interface_remote::RemoteError::new(
                        "history-unavailable",
                        "Conversation history could not be read. Try again or use Terminal view.",
                        true,
                    )
                })
        })
    }
}

#[async_trait]
impl HostDispatcher for RemoteHostDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if operation != CHANNELS.remote_access {
            return Err(DispatchError::new(
                "operation-not-migrated",
                "Unknown remote access operation.",
                false,
                true,
            ));
        }
        let server = self.server.as_ref().ok_or_else(|| {
            DispatchError::new(
                "remote-unavailable",
                "Remote access is unavailable in this host.",
                false,
                true,
            )
        })?;
        let status = match decode_command(payload)? {
            RemoteCommand::PairDirect(request) => {
                serde_json::to_value(server.pair_direct(request).await.map_err(remote_error)?)
            }
            RemoteCommand::Connection(request) => serde_json::to_value(
                server
                    .manage_connection(request)
                    .await
                    .map_err(remote_error)?,
            ),
            RemoteCommand::Trusted(request) => serde_json::to_value(
                server
                    .manage_trust(request.version, request.command)
                    .map_err(remote_error)?,
            ),
            RemoteCommand::Access(request) => {
                serde_json::to_value(server.manage(request).await.map_err(remote_error)?)
            }
        };
        status.map_err(|_| {
            DispatchError::new(
                "remote-encode",
                "Remote access status could not be encoded.",
                false,
                false,
            )
        })
    }
}

/// One remote administration request. Its `action` picks the shape; every
/// other action is the original remote-access request.
enum RemoteCommand {
    PairDirect(PairingRequestV6),
    Connection(ConnectionAdminRequest),
    Trusted(TrustAdminRequest),
    Access(RemoteAccessRequest),
}

fn decode_command(payload: Value) -> Result<RemoteCommand, DispatchError> {
    match payload.get("action").and_then(Value::as_str) {
        Some("pairDirect") => {
            decode(payload, "Invalid phone pairing request.").map(RemoteCommand::PairDirect)
        }
        Some("connection") => {
            decode(payload, "Invalid phone connection request.").map(RemoteCommand::Connection)
        }
        Some("trusted") => decode(payload, "Invalid device request.").map(RemoteCommand::Trusted),
        _ => decode(payload, "Invalid remote access request.").map(RemoteCommand::Access),
    }
}

fn decode<T: DeserializeOwned>(payload: Value, message: &'static str) -> Result<T, DispatchError> {
    serde_json::from_value(payload)
        .map_err(|_| DispatchError::new("invalid-payload", message, false, true))
}

/// Whether the phone may answer `approval_id` now: only while no other
/// writer (a Code pane) holds the session, and only the request the Mac shows
/// now, one the phone can answer. Every refusal is replay-safe: nothing ran.
fn answer_gate(
    thread: &RemoteThreadV1,
    held_elsewhere: bool,
    current: Option<&RemoteThreadApprovalV1>,
    approval_id: &str,
) -> Result<(), RemoteError> {
    if held_elsewhere {
        return Err(thread_error(
            "thread-in-terminal",
            "This session is open in a Code pane on your Mac. Answer it there.",
        ));
    }
    if thread.status != RemoteThreadStatus::NeedsYou
        || !current.is_some_and(|approval| approval.id == approval_id && approval.answerable)
    {
        return Err(thread_error(
            "approval-changed",
            "This request has changed or was answered. Review it again.",
        ));
    }
    Ok(())
}

/// Check the authoritative item, not a display row or a second snapshot whose
/// membership could differ from the one that supplied `can_send`.
fn send_item_gate(item: &RemoteInboxItemV2, held_elsewhere: bool) -> Result<(), RemoteError> {
    if !item.can_send {
        return Err(thread_error(
            "task-read-only",
            "Read this task's result here. Start or continue work from its agent on your Mac.",
        ));
    }
    let thread = item
        .thread
        .as_ref()
        .ok_or_else(|| thread_error("thread-not-listed", "This task has no provider session."))?;
    send_gate(thread, held_elsewhere)
}

/// Whether the phone may send a follow-up now: only to a thread at rest
/// (not working, not parked on a request) that no Code pane holds.
fn send_gate(thread: &RemoteThreadV1, held_elsewhere: bool) -> Result<(), RemoteError> {
    if held_elsewhere {
        return Err(thread_error(
            "thread-in-terminal",
            "This session is open in a Code pane on your Mac. Continue there.",
        ));
    }
    if !matches!(
        thread.status,
        RemoteThreadStatus::Idle | RemoteThreadStatus::Failed
    ) {
        return Err(thread_error(
            "thread-busy",
            "This thread is working or waiting on a request. Send when it is done.",
        ));
    }
    Ok(())
}

/// How a refused approval answer reads on the phone, keeping its certainty:
/// replay-safe only when nothing reached the agent.
fn answer_refusal(error: &DispatchError) -> RemoteError {
    effect_refusal(
        error,
        (
            "approval-refused",
            "Your Mac did not deliver this answer. Review the request again.",
        ),
        "The agent did not take this answer. Check the thread on your Mac.",
        "Your answer may have reached the agent. Check on your Mac before answering again.",
    )
}

/// How a refused follow-up reads on the phone, keeping its certainty.
fn send_refusal(error: &DispatchError) -> RemoteError {
    effect_refusal(
        error,
        (
            "send-refused",
            "Your Mac did not send this message. Nothing reached the agent.",
        ),
        "The agent did not take this message. Check the thread on your Mac.",
        "Your message may have reached the agent. Check the thread before sending it again.",
    )
}

fn effect_refusal(
    error: &DispatchError,
    unsent: (&'static str, &'static str),
    rejected: &'static str,
    unknown: &'static str,
) -> RemoteError {
    if error.replay_safe {
        RemoteError::new(unsent.0, unsent.1, true)
    } else if error.code == crate::dispatcher::DEFINITELY_REJECTED_CODE {
        RemoteError::new("effect-rejected", rejected, false)
    } else {
        RemoteError::new("outcome-unknown", unknown, false)
    }
}

/// The remote service's own code, message and replay safety; never retryable.
#[allow(clippy::needless_pass_by_value)] // The shape `map_err` hands over.
fn remote_error(error: RemoteError) -> DispatchError {
    DispatchError::new(error.code, error.message, false, error.replay_safe)
}

struct TrustStore(interface_desktop_services::RemoteTrustStore);

struct ConnectionStore(interface_desktop_services::PhoneConnectionStore);
impl interface_remote::ConnectionPersistence for ConnectionStore {
    fn load_listener(
        &self,
    ) -> Result<
        Option<interface_contracts::remote_connection::ConnectionListenerV1>,
        interface_remote::RemoteError,
    > {
        self.0.load_listener().map_err(|_| {
            interface_remote::RemoteError::new(
                "connection-storage",
                "The phone connection port could not be loaded.",
                true,
            )
        })
    }
    fn save_listener(
        &self,
        value: &interface_contracts::remote_connection::ConnectionListenerV1,
    ) -> Result<(), interface_remote::RemoteError> {
        self.0.save_listener(value).map_err(|_| {
            interface_remote::RemoteError::new(
                "connection-storage",
                "The phone connection port could not be saved.",
                false,
            )
        })
    }
    fn load(
        &self,
    ) -> Result<
        Option<interface_contracts::remote_connection::ConnectionConfigurationV1>,
        interface_remote::RemoteError,
    > {
        self.0.load().map_err(|_| {
            interface_remote::RemoteError::new(
                "connection-storage",
                "The secure phone configuration could not be loaded.",
                true,
            )
        })
    }
    fn save(
        &self,
        value: &interface_contracts::remote_connection::ConnectionConfigurationV1,
    ) -> Result<(), interface_remote::RemoteError> {
        self.0.save(value).map_err(|_| {
            interface_remote::RemoteError::new(
                "connection-storage",
                "The secure phone configuration could not be saved.",
                false,
            )
        })
    }
}
impl interface_remote::TrustPersistence for TrustStore {
    fn load(
        &self,
    ) -> Result<interface_contracts::remote_trust::RemoteTrustData, interface_remote::RemoteError>
    {
        self.0.load().map_err(|_| {
            interface_remote::RemoteError::new(
                "trust-unavailable",
                "Trusted devices could not be loaded.",
                true,
            )
        })
    }
    fn save(
        &self,
        value: &interface_contracts::remote_trust::RemoteTrustData,
    ) -> Result<(), interface_remote::RemoteError> {
        self.0.save(value).map_err(|_| {
            interface_remote::RemoteError::new(
                "trust-write-failed",
                "Trusted devices could not be saved.",
                false,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn terminal() -> TerminalSummary {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-companion-v2.json"
        ))
        .unwrap();
        serde_json::from_value(fixture["snapshot"]["workspaces"][0]["terminals"][0].clone())
            .unwrap()
    }

    fn refusal(payload: Value) -> DispatchError {
        match decode_command(payload) {
            Ok(_) => panic!("an invalid payload was admitted"),
            Err(error) => error,
        }
    }

    #[test]
    fn each_action_decodes_its_own_request_and_names_its_own_refusal() {
        assert!(matches!(
            decode_command(json!({"action": "pairDirect", "version": 6})),
            Ok(RemoteCommand::PairDirect(_))
        ));
        assert!(matches!(
            decode_command(json!({"action": "connection", "version": 5, "command": "status"})),
            Ok(RemoteCommand::Connection(_))
        ));
        assert!(matches!(
            decode_command(json!({"action": "trusted", "version": 3, "command": "status"})),
            Ok(RemoteCommand::Trusted(_))
        ));
        assert!(matches!(
            decode_command(json!({"action": "status"})),
            Ok(RemoteCommand::Access(RemoteAccessRequest::Status {}))
        ));
        for (payload, message) in [
            (
                json!({"action": "pairDirect", "version": 6, "extra": true}),
                "Invalid phone pairing request.",
            ),
            (
                json!({"action": "connection", "version": 5}),
                "Invalid phone connection request.",
            ),
            (json!({"action": "trusted"}), "Invalid device request."),
            (
                json!({"action": "unknown"}),
                "Invalid remote access request.",
            ),
            (json!({}), "Invalid remote access request."),
            (json!({"action": 6}), "Invalid remote access request."),
        ] {
            let error = refusal(payload);
            assert_eq!(
                (error.code.as_str(), error.message.as_str()),
                ("invalid-payload", message)
            );
            assert!(!error.retryable && error.replay_safe);
        }
    }

    #[test]
    fn remote_refusals_keep_their_code_message_and_replay_safety() {
        for replay_safe in [true, false] {
            let error = remote_error(RemoteError::new("control-busy", "Busy.", replay_safe));
            assert_eq!(
                (error.code.as_str(), error.message.as_str()),
                ("control-busy", "Busy.")
            );
            assert!(!error.retryable);
            assert_eq!(error.replay_safe, replay_safe);
        }
    }

    #[test]
    fn a_phone_request_reaches_only_the_exact_terminal_generation() {
        let current = terminal();
        let key = (current.id.as_uuid(), current.generation.0);
        let snapshot = || TerminalSnapshot {
            revision: 1,
            terminals: vec![current.clone()],
        };
        assert_eq!(terminal_at(snapshot(), key).unwrap().id, current.id);
        for stale in [(key.0, key.1 + 1), (uuid::Uuid::new_v4(), key.1)] {
            let error = terminal_at(snapshot(), stale).unwrap_err();
            assert_eq!(
                (error.code, error.message, error.replay_safe),
                ("stale-terminal", "This terminal has changed.", true)
            );
        }
    }

    #[test]
    fn the_phone_answers_only_the_current_request_no_pane_holds() {
        let thread = RemoteThreadV1 {
            key: "claude#t".into(),
            engine: "claude".into(),
            session_id: "t".into(),
            title: "T".into(),
            project: None,
            status: RemoteThreadStatus::NeedsYou,
            section: interface_contracts::threads::ThreadSection::Active,
            unread: false,
            last_activity: String::new(),
        };
        let approval = |id: &str, answerable| RemoteThreadApprovalV1 {
            id: id.into(),
            tool: "Bash".into(),
            summary: "ls".into(),
            answerable,
        };
        let current = approval("a1", true);
        assert!(answer_gate(&thread, false, Some(&current), "a1").is_ok());
        let refusal = |thread: &RemoteThreadV1, held, current: Option<&RemoteThreadApprovalV1>| {
            let error = answer_gate(thread, held, current, "a1").unwrap_err();
            assert!(error.replay_safe);
            error.code
        };
        assert_eq!(refusal(&thread, true, Some(&current)), "thread-in-terminal");
        assert_eq!(refusal(&thread, false, None), "approval-changed");
        assert_eq!(
            refusal(&thread, false, Some(&approval("a2", true))),
            "approval-changed"
        );
        assert_eq!(
            refusal(&thread, false, Some(&approval("a1", false))),
            "approval-changed",
            "a question is answered on the Mac"
        );
        let working = RemoteThreadV1 {
            status: RemoteThreadStatus::Working,
            ..thread.clone()
        };
        assert_eq!(refusal(&working, false, Some(&current)), "approval-changed");
    }

    #[test]
    fn an_answers_certainty_survives_the_trip_to_the_phone() {
        let refused = |code: &str, replay_safe| {
            answer_refusal(&DispatchError::new(code, "detail", false, replay_safe))
        };
        let unsent = refused(crate::dispatcher::DEFINITELY_UNSENT_CODE, true);
        assert_eq!(
            (unsent.code, unsent.replay_safe),
            ("approval-refused", true)
        );
        let rejected = refused(crate::dispatcher::DEFINITELY_REJECTED_CODE, false);
        assert_eq!(
            (rejected.code, rejected.replay_safe),
            ("effect-rejected", false)
        );
        let unknown = refused(crate::dispatcher::OUTCOME_UNKNOWN_CODE, false);
        assert_eq!(
            (unknown.code, unknown.replay_safe),
            ("outcome-unknown", false)
        );
        // Anything else that may have run is unknown, never safe to repeat.
        let other = refused("registry-unreachable", false);
        assert_eq!((other.code, other.replay_safe), ("outcome-unknown", false));
        let sent = send_refusal(&DispatchError::new(
            crate::dispatcher::DEFINITELY_UNSENT_CODE,
            "detail",
            false,
            true,
        ));
        assert_eq!((sent.code, sent.replay_safe), ("send-refused", true));
        let lost = send_refusal(&DispatchError::new(
            crate::dispatcher::OUTCOME_UNKNOWN_CODE,
            "detail",
            false,
            false,
        ));
        assert_eq!((lost.code, lost.replay_safe), ("outcome-unknown", false));
    }

    #[test]
    fn phone_send_guard_refuses_task_results_even_when_the_provider_is_idle() {
        let inbox: RemoteInboxV2 = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-inbox-v2.json"
        ))
        .unwrap();
        let result = inbox
            .items
            .iter()
            .find(|item| !item.can_send && item.thread.is_some())
            .unwrap();
        let error = send_item_gate(result, false).unwrap_err();
        assert_eq!(error.code, "task-read-only");
        assert!(error.replay_safe);
        let mut ordinary = result.clone();
        ordinary.can_send = true;
        assert!(send_item_gate(&ordinary, false).is_ok());
        assert_eq!(
            send_item_gate(&ordinary, true).unwrap_err().code,
            "thread-in-terminal"
        );
        ordinary.thread = None;
        assert_eq!(
            send_item_gate(&ordinary, false).unwrap_err().code,
            "thread-not-listed"
        );
    }

    #[test]
    fn the_phone_sends_a_follow_up_only_to_a_thread_at_rest_no_pane_holds() {
        let thread = |status| RemoteThreadV1 {
            key: "claude#t".into(),
            engine: "claude".into(),
            session_id: "t".into(),
            title: "T".into(),
            project: None,
            status,
            section: interface_contracts::threads::ThreadSection::Active,
            unread: false,
            last_activity: String::new(),
        };
        assert!(send_gate(&thread(RemoteThreadStatus::Idle), false).is_ok());
        assert!(send_gate(&thread(RemoteThreadStatus::Failed), false).is_ok());
        for (status, held, code) in [
            (RemoteThreadStatus::Idle, true, "thread-in-terminal"),
            (RemoteThreadStatus::Working, false, "thread-busy"),
            (RemoteThreadStatus::NeedsYou, false, "thread-busy"),
        ] {
            let error = send_gate(&thread(status), held).unwrap_err();
            assert_eq!(error.code, code);
            assert!(error.replay_safe);
        }
    }

    #[test]
    fn task_results_follow_the_terminal_generation_or_its_bound_session() {
        let terminal = |id: &str, generation| AttentionTarget::Terminal {
            terminal_id: id.into(),
            generation,
        };
        let session = |key: &str| AttentionTarget::Session { key: key.into() };
        let binding = "claude:session".to_owned();
        assert!(task_for_terminal(&terminal("t", 3), "t", 3, None));
        assert!(!task_for_terminal(&terminal("t", 2), "t", 3, None));
        assert!(!task_for_terminal(&terminal("u", 3), "t", 3, None));
        assert!(task_for_terminal(
            &session("claude:session"),
            "t",
            3,
            Some(&binding)
        ));
        assert!(!task_for_terminal(
            &session("codex:other"),
            "t",
            3,
            Some(&binding)
        ));
        assert!(!task_for_terminal(&session("claude:session"), "t", 3, None));
    }
}
