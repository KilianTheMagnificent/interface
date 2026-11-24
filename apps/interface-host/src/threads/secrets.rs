//! Private secret card (protocol 67, T3 section 12): team tools'
//! `request_secret`, the open request on a thread's view, and the
//! `iface:threadSecret` channel that answers it.
//!
//! An agent's `request_secret {name, reason}` opens one request on its
//! thread (its head, for a switched thread) and waits, at most
//! [`SECRET_WAIT`], for the owner. The view carries the request; the pane
//! docks a card above its composer. Give writes the value to its 0600 file
//! (`interface_settings::thread_secrets`) and the tool answers only the
//! file's path; Decline (or no answer in time, or the agent's call going
//! away) answers `{declined: true}`. The value crosses the wire once, in
//! Give, and is never logged, journalled, pushed, put in a draft or in the
//! tool's result: the journal sees only the tool call (name and reason) and
//! its result (the path, or declined). Secrets expire after a day and go
//! with their thread.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use interface_common::sync::lock;
use interface_contracts::{
    engine::EngineEventKind,
    thread_secrets::{
        THREAD_SECRETS_VIEW_VERSION, ThreadSecretCommand, ThreadSecretRequest, ThreadSecretsView,
        valid_secret_name, valid_secret_reason,
    },
    threads::ThreadsView,
};
use interface_settings::{thread_secrets, thread_team::TeamRefusal};
use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

use super::{ThreadHost, internal, worktree_missing};
use crate::dispatcher::{DispatchError, INVALID_PAYLOAD_CODE};

/// How long a request waits for the owner before it is answered declined.
pub const SECRET_WAIT: Duration = Duration::from_secs(600);
/// How often expired secrets are swept while Interface runs.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);
/// How long a request waits for its own `request_secret` tool call to show
/// in the asking session's events, which tells which run it belongs to. A
/// provider that never shows one leaves the request unbound: only its
/// caller's going, not a run's end, withdraws it then.
const SECRET_BIND_WAIT: Duration = Duration::from_millis(500);
/// The line the tool's result carries beside the path.
pub const SECRET_NOTE: &str = "Read the value from this file; do not print it.";
/// Secrets are not attached in this host. Definitely unsent.
pub const THREAD_SECRET_UNAVAILABLE_CODE: &str = "thread-secret-unavailable";
/// No such open request, or a rule refused it. Definitely unsent.
pub const THREAD_SECRET_REFUSED_CODE: &str = "thread-secret-refused";
/// The file could not be written; nothing was given. Definitely unsent.
pub const THREAD_SECRET_FAILED_CODE: &str = "thread-secret-failed";
/// `request_secret` while one is open on the thread.
pub const SECRET_PENDING: &str = "secret-pending";

enum SecretAnswer {
    /// The file's path, and the waiting call's word that it took it: false
    /// (or no word) when it had already stopped waiting, so Give removes
    /// the file and says nothing was given.
    Given(PathBuf, oneshot::Sender<bool>),
    Declined,
}

struct Pending {
    request: ThreadSecretRequest,
    answer: oneshot::Sender<SecretAnswer>,
    /// The session that asked (an earlier segment's, for a switched thread
    /// whose head is another).
    asker: String,
    /// The asker's run the request belongs to (see [`SecretCalls`]); `None`
    /// when its tool call never showed.
    run: Option<u64>,
}

/// One session's runs as `request_secret` needs them: a run is named by how
/// many runs of the session had ended before it, so the event stream alone
/// tells them apart, whoever opened them.
#[derive(Default)]
struct SecretCalls {
    /// Runs of this session that have ended.
    ends: u64,
    /// The run of each `request_secret` tool call seen and not yet claimed
    /// by its request, oldest first.
    unbound: VecDeque<u64>,
}

pub(crate) struct SecretParts {
    root: PathBuf,
    /// The open request of each thread, by its head key.
    pending: Mutex<HashMap<String, Pending>>,
    next: AtomicU64,
    /// Each session's runs and unclaimed `request_secret` calls.
    calls: Mutex<HashMap<String, SecretCalls>>,
    /// Rung when a `request_secret` tool call is seen.
    call_seen: Notify,
}

/// Takes a request out when the call waiting on it goes, however it goes.
struct PendingGuard {
    host: ThreadHost,
    head: String,
    id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if self.host.take_pending(&self.head, &self.id).is_some()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let host = self.host.clone();
            runtime.spawn(async move { host.push_current_view().await });
        }
    }
}

/// The run of `asker`'s `request_secret` tool call this request is, once its
/// event shows (it may arrive after the call, from another pipe); `None`
/// when none shows within [`SECRET_BIND_WAIT`].
async fn bind_secret_call(parts: &SecretParts, asker: &str) -> Option<u64> {
    let deadline = tokio::time::Instant::now() + SECRET_BIND_WAIT;
    loop {
        let seen = parts.call_seen.notified();
        tokio::pin!(seen);
        seen.as_mut().enable();
        if let Some(run) = lock(&parts.calls)
            .get_mut(asker)
            .and_then(|session| session.unbound.pop_front())
        {
            return Some(run);
        }
        if tokio::time::timeout_at(deadline, seen).await.is_err() {
            return None;
        }
    }
}

fn refused(message: impl Into<String>) -> DispatchError {
    DispatchError::new(THREAD_SECRET_REFUSED_CODE, message, false, true)
}

impl ThreadHost {
    /// Keeps secrets under `data_root`, sweeps expired ones now and then
    /// every hour while the host runs. Attaching twice keeps the first.
    pub fn attach_secrets(&self, data_root: &Path) {
        if self
            .inner
            .secrets
            .set(SecretParts {
                root: data_root.to_path_buf(),
                pending: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
                calls: Mutex::new(HashMap::new()),
                call_seen: Notify::new(),
            })
            .is_err()
        {
            return;
        }
        let swept = thread_secrets::sweep(data_root, self.now());
        if swept > 0 {
            tracing::info!(swept, "expired thread secrets were removed");
        }
        let host = self.clone();
        self.spawn_tracked(async move {
            let mut stop = host.inner.stop.subscribe();
            loop {
                tokio::select! {
                    _ = stop.changed() => return,
                    () = tokio::time::sleep(SWEEP_EVERY) => {}
                }
                let (root, now) = match host.inner.secrets.get() {
                    Some(parts) => (parts.root.clone(), host.now()),
                    None => return,
                };
                let _ =
                    tokio::task::spawn_blocking(move || thread_secrets::sweep(&root, now)).await;
            }
        });
    }

    fn take_pending(&self, head: &str, id: &str) -> Option<Pending> {
        let parts = self.inner.secrets.get()?;
        let mut pending = lock(&parts.pending);
        if pending.get(head).is_some_and(|open| open.request.id == id) {
            return pending.remove(head);
        }
        None
    }

    /// Puts each thread's open request on `view`.
    pub(super) fn with_secrets(&self, view: &mut ThreadsView) {
        let Some(parts) = self.inner.secrets.get() else {
            return;
        };
        let pending = lock(&parts.pending);
        for thread in &mut view.threads {
            thread.secret_request = pending.get(&thread.key).map(|open| open.request.clone());
        }
    }

    /// Pushes the current view, holding the store's lock so pushes leave in
    /// order.
    async fn push_current_view(&self) {
        let guard = self.inner.store.lock().await;
        let Ok(store) = guard.as_ref() else {
            return;
        };
        let now = self.now();
        let document = store.document().clone();
        let Ok(mut view) =
            tokio::task::spawn_blocking(move || document.view(now, worktree_missing)).await
        else {
            return;
        };
        self.with_away(&mut view);
        self.with_hidden(&mut view);
        self.with_accounts(&mut view);
        self.with_schedules(&mut view);
        self.with_outside(&mut view);
        self.with_limits(&mut view);
        self.with_secrets(&mut view);
        self.with_sides(&mut view);
        self.with_folder_trust(&mut view);
        self.push_view(&view).await;
        drop(guard);
    }

    /// `request_secret` for the thread `caller` (an earlier segment's key
    /// reaches its head): opens the request, shows it, and waits for the
    /// owner. Answers the file's path, or declined.
    pub(super) async fn team_request_secret(
        &self,
        caller: &str,
        agent: String,
        name: &str,
        reason: &str,
        gone: CancellationToken,
    ) -> Result<Value, TeamRefusal> {
        use interface_settings::thread_team;
        if !valid_secret_name(name) {
            return Err(TeamRefusal::new(
                thread_team::INVALID_INPUT,
                "`name` is 1 to 64 letters, digits, `_`, `.` or `-`, not starting with `.`.",
            ));
        }
        if !valid_secret_reason(reason) {
            return Err(TeamRefusal::new(
                thread_team::INVALID_INPUT,
                "`reason` is one line of at most 300 characters.",
            ));
        }
        let Some(parts) = self.inner.secrets.get() else {
            return Err(TeamRefusal::new(
                thread_team::UNAVAILABLE,
                "Interface cannot keep secrets here.",
            ));
        };
        let run = bind_secret_call(parts, caller).await;
        let head = self.head_key(caller).await;
        let id = format!("secret-{}", parts.next.fetch_add(1, Ordering::Relaxed));
        let (answer, answered) = oneshot::channel();
        {
            let mut pending = lock(&parts.pending);
            if pending.contains_key(&head) {
                return Err(TeamRefusal::new(
                    SECRET_PENDING,
                    "This thread already asks the owner for a secret; wait for that answer first.",
                ));
            }
            pending.insert(
                head.clone(),
                Pending {
                    request: ThreadSecretRequest {
                        id: id.clone(),
                        name: name.to_owned(),
                        reason: reason.trim().to_owned(),
                        agent,
                    },
                    answer,
                    asker: caller.to_owned(),
                    run,
                },
            );
        }
        let guard = PendingGuard {
            host: self.clone(),
            head,
            id,
        };
        self.push_current_view().await;
        let mut stop = self.inner.stop.subscribe();
        let mut answered = answered;
        // The owner's answer, or None: the wait ran out, the host stops, or
        // the caller went (its connection closed or it cancelled the call).
        let waited = tokio::select! {
            answer = &mut answered => Some(answer),
            () = tokio::time::sleep(SECRET_WAIT) => None,
            _ = stop.changed() => None,
            () = gone.cancelled() => None,
        };
        let path = match waited {
            Some(Ok(SecretAnswer::Given(path, took))) => {
                let _ = took.send(true);
                Some(path)
            }
            Some(Ok(SecretAnswer::Declined) | Err(_)) => None,
            None => {
                if self.take_pending(&guard.head, &guard.id).is_some() {
                    // Withdrawn: the card goes now.
                    self.push_current_view().await;
                } else if let Ok(SecretAnswer::Given(_, took)) = answered.await {
                    // A Give took the request as the wait ended: it must
                    // not leave a file this caller was never told about.
                    let _ = took.send(false);
                }
                None
            }
        };
        drop(guard);
        Ok(match path {
            Some(path) => json!({"path": path.to_string_lossy(), "note": SECRET_NOTE}),
            None => json!({"declined": true}),
        })
    }

    /// One event of session `key`, in its stream's order, with whether it
    /// ended a run. A `request_secret` tool call is noted for its request to
    /// claim; a run's end withdraws (answers declined) the request that run
    /// made, since it can take no answer, and never one a later run made.
    /// Inline on every event: nothing is locked unless it matters.
    pub(crate) fn secret_observe(&self, key: &str, event: &EngineEventKind, ended: bool) {
        let asks = matches!(event, EngineEventKind::ToolCall { tool, .. }
            if tool.ends_with("request_secret"));
        let closed = matches!(event, EngineEventKind::SessionClosed);
        if !(asks || ended || closed) {
            return;
        }
        let Some(parts) = self.inner.secrets.get() else {
            return;
        };
        let withdrawn = {
            let mut calls = lock(&parts.calls);
            let mut withdrawn = None;
            if ended {
                let session = calls.entry(key.to_owned()).or_default();
                let run = session.ends;
                session.ends += 1;
                // A call of that run its request never claimed (an approval
                // refused it) can no longer be claimed.
                session.unbound.retain(|unclaimed| *unclaimed > run);
                let mut pending = lock(&parts.pending);
                let head = pending
                    .iter()
                    .find(|(_, open)| open.asker == key && open.run == Some(run))
                    .map(|(head, _)| head.clone());
                withdrawn = head.and_then(|head| pending.remove(&head));
            }
            if closed {
                calls.remove(key);
            } else if asks {
                let session = calls.entry(key.to_owned()).or_default();
                let run = session.ends;
                session.unbound.push_back(run);
            }
            withdrawn
        };
        if asks {
            parts.call_seen.notify_waiters();
        }
        let Some(open) = withdrawn else {
            return;
        };
        let _ = open.answer.send(SecretAnswer::Declined);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let host = self.clone();
            runtime.spawn(async move { host.push_current_view().await });
        }
    }

    /// `iface:threadSecret`. The payload may carry a value: a payload that
    /// does not decode is refused without saying why, so nothing of it is
    /// logged.
    pub(super) async fn secret_command(&self, payload: Value) -> Result<Value, DispatchError> {
        let command = crate::payload::decode_object::<ThreadSecretCommand, _>(payload, |_| {
            DispatchError::new(
                INVALID_PAYLOAD_CODE,
                "The secret command does not match its v1 schema.",
                false,
                true,
            )
        })?;
        let Some(parts) = self.inner.secrets.get() else {
            return Err(DispatchError::new(
                THREAD_SECRET_UNAVAILABLE_CODE,
                "Interface cannot keep secrets here.",
                false,
                true,
            ));
        };
        let root = parts.root.clone();
        match command {
            ThreadSecretCommand::List {} => {}
            ThreadSecretCommand::Give { key, id, value } => {
                let head = self.head_key(&key).await;
                let name = lock(&parts.pending)
                    .get(&head)
                    .filter(|open| open.request.id == id)
                    .map(|open| open.request.name.clone())
                    .ok_or_else(|| {
                        refused("The agent no longer asks for that secret; nothing was given.")
                    })?;
                let (folder, thread, secret) = (root.clone(), head.clone(), name.clone());
                let path = tokio::task::spawn_blocking(move || {
                    thread_secrets::write_secret(&folder, &thread, &secret, &value)
                })
                .await
                .map_err(|_| internal("The secret task stopped unexpectedly."))?
                .map_err(|error| {
                    DispatchError::new(
                        THREAD_SECRET_FAILED_CODE,
                        format!("The secret could not be saved: {error}. Nothing was given."),
                        false,
                        true,
                    )
                })?;
                let Some(open) = self.take_pending(&head, &id) else {
                    // The request went while the file was written: the
                    // value must not stay where nobody asked for it.
                    let _ = thread_secrets::forget(&root, &head, &name);
                    return Err(refused(
                        "The agent stopped waiting for that secret; nothing was given.",
                    ));
                };
                // The waiting call says whether it took the path; one that
                // went meanwhile did not, and the file goes with it.
                let (took, taken) = oneshot::channel();
                let delivered = open.answer.send(SecretAnswer::Given(path, took)).is_ok()
                    && taken.await == Ok(true);
                self.push_current_view().await;
                if !delivered {
                    let _ = thread_secrets::forget(&root, &head, &name);
                    return Err(refused(
                        "The agent stopped waiting for that secret; nothing was given.",
                    ));
                }
            }
            ThreadSecretCommand::Decline { key, id } => {
                let head = self.head_key(&key).await;
                let open = self
                    .take_pending(&head, &id)
                    .ok_or_else(|| refused("The agent no longer asks for that secret."))?;
                let _ = open.answer.send(SecretAnswer::Declined);
                self.push_current_view().await;
            }
            ThreadSecretCommand::Forget { key, name } => {
                let (folder, thread) = (root.clone(), self.head_key(&key).await);
                tokio::task::spawn_blocking(move || {
                    thread_secrets::forget(&folder, &thread, &name)
                })
                .await
                .map_err(|_| internal("The secret task stopped unexpectedly."))?
                .map_err(|error| {
                    DispatchError::new(THREAD_SECRET_FAILED_CODE, error.0, false, true)
                })?;
            }
        }
        let view = tokio::task::spawn_blocking(move || ThreadSecretsView {
            version: THREAD_SECRETS_VIEW_VERSION,
            secrets: thread_secrets::list(&root),
        })
        .await
        .map_err(|_| internal("The secret task stopped unexpectedly."))?;
        serde_json::to_value(view).map_err(|_| internal("The secrets could not be listed."))
    }

    /// A deleted or forgotten thread's secrets go, and its open request is
    /// declined.
    pub(crate) fn secrets_removed(&self, key: &str) {
        let Some(parts) = self.inner.secrets.get() else {
            return;
        };
        if let Some(open) = lock(&parts.pending).remove(key) {
            let _ = open.answer.send(SecretAnswer::Declined);
        }
        if let Err(error) = thread_secrets::forget_thread(&parts.root, key) {
            tracing::warn!(%error, "a removed thread's secrets stayed");
        }
    }
}
