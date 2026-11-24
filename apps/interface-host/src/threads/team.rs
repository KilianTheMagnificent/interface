//! Team tools (T3 section 2, protocol 57): what a Threads agent's
//! `iface_team` calls do. The caller is the session its socket was bound to
//! (`crate::team_tools`), never anything in the tool input. Every rule is
//! `interface_settings::thread_team`'s; this module reads the facts it needs
//! (the store, the run tap, the journal, the registry) and acts through the
//! guarded runtime, so the send hook, turn record, journal and permission
//! modes apply to every prompt it sends.
//!
//! A started thread is an ordinary thread: prepared like a New thread's
//! workspace, created on the guarded runtime at collaboration depth 1 (no
//! team tools, no collaboration MCP below it), bound with its `child`
//! lineage, then sent its task through the send hook (turn 0, its checkpoint
//! and its journal entries recorded the normal way). An ACP agent refuses an
//! empty launch, so there the task is the launch prompt and the thread is
//! bound the way a launch is.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use interface_common::sync::lock;
use interface_contracts::{
    engine::{
        EngineEvent, EngineEventKind, LaunchOptions, SessionKind, SessionStatus, SessionSummary,
        session_key,
    },
    ipc::{CHANNELS, EngineProbe, parse_key},
    threads::{LineageRelation, ThreadLineage, TitleSource, WorkspaceMode},
};
use interface_settings::{
    thread_journal::{self, EndedTurn},
    thread_team::{
        self, DeliveryHow, NoticeDecision, NoticeFacts, ReceiptDecision, StartedThread,
        StatusFacts, TeamRefusal, TeamStatus, ThreadTeamStore,
    },
    thread_visuals::{self, VisualRefusal},
    threads::prompt_digest,
};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::ThreadHost;
use crate::dispatcher::{DispatchError, HostDispatcher, SessionAccessMemory};

/// What [`ThreadHost::attach_team`] needs: the guarded runtime every start
/// and send goes through, the owner's chosen permission modes, and the
/// ledger store (or why it did not open).
pub struct ThreadTeamConfig {
    pub runtime: Arc<dyn HostDispatcher>,
    pub access: Option<Arc<dyn SessionAccessMemory>>,
    pub store: Result<ThreadTeamStore, String>,
}

pub(crate) struct TeamParts {
    runtime: Arc<dyn HostDispatcher>,
    access: Option<Arc<dyn SessionAccessMemory>>,
    store: Arc<Mutex<Result<ThreadTeamStore, String>>>,
    /// The started threads, kept beside the store so the dispatcher's resume
    /// can ask without waiting on a commit.
    children: Mutex<HashSet<String>>,
    /// Started threads whose notice is being worked on.
    notices: Mutex<HashSet<String>>,
    /// What views show of the ledger's outside agents part (T3 section 11),
    /// refreshed after every ledger call.
    pub(super) outside: Mutex<super::outside::OutsideSnapshot>,
    /// One outside start at a time, from its limits to its ledger entry, so
    /// concurrent calls never pass a limit together.
    pub(super) outside_starts: tokio::sync::Mutex<()>,
    /// One thread's start at a time, from its limits to its ledger entry, so
    /// calls that arrive together never pass a limit together.
    starts: tokio::sync::Mutex<()>,
    /// One notice decided and sent at a time: a notice starts its caller's
    /// turn, so the next one sees that turn and waits for it.
    notice_sends: tokio::sync::Mutex<()>,
}

/// One thread's status as the tools report it.
struct StatusView {
    status: TeamStatus,
    needs: Option<thread_team::Needs>,
    ended: Option<EndedTurn>,
}

/// The caller: the thread the socket belongs to.
struct Caller {
    key: String,
    engine: String,
    title: String,
    model: Option<String>,
    mode: Option<String>,
    project: Option<String>,
    workspace: Option<interface_contracts::threads::ThreadWorkspace>,
}

/// Which threads a caller reaches for reading: a thread caller its own
/// project's (`may_read`); an outside session any thread with a project.
#[derive(Clone, Copy)]
pub(super) enum Reach<'a> {
    Project(Option<&'a str>),
    AnyProject,
}

impl Reach<'_> {
    fn allows(self, target: Option<&str>) -> bool {
        match self {
            Self::Project(project) => thread_team::may_read(project, target),
            Self::AnyProject => target.is_some_and(|target| !target.is_empty()),
        }
    }

    const fn where_(self) -> &'static str {
        match self {
            Self::Project(_) => "in this project",
            Self::AnyProject => "in Interface's projects",
        }
    }
}

/// The caller as reading and control see it: its ledger key and its reach.
#[derive(Clone, Copy)]
pub(super) struct Who<'a> {
    pub(super) key: &'a str,
    pub(super) reach: Reach<'a>,
}

impl Caller {
    fn who(&self) -> Who<'_> {
        Who {
            key: &self.key,
            reach: Reach::Project(self.project.as_deref()),
        }
    }
}

/// Everything a start needs once its caller's rules chose it (a thread's
/// for team tools, an outside session's for `iface mcp`).
pub(super) struct StartPlan {
    /// The ledger's parent: the caller thread or the outside session.
    pub(super) parent: String,
    pub(super) started_by: Option<thread_team::StartedBy>,
    pub(super) project: String,
    /// `local`, `worktree` or `existing`, with a worktree's base or an
    /// existing worktree's path.
    pub(super) prepare: &'static str,
    pub(super) base: Option<String>,
    pub(super) existing: Option<String>,
    pub(super) lineage: Option<ThreadLineage>,
    pub(super) task: String,
    pub(super) provider: String,
    pub(super) model: Option<String>,
    pub(super) title: String,
    pub(super) title_source: TitleSource,
    pub(super) notify: bool,
    pub(super) mode: &'static str,
    pub(super) account: Option<String>,
}

/// A started thread's display label for its provider.
pub(super) fn provider_label(engine: &str, probes: Option<&[EngineProbe]>) -> String {
    let known = match engine {
        "claude" => Some("Claude"),
        "codex" => Some("Codex"),
        "acp:grok" => Some("Grok"),
        "acp:qwen" => Some("Qwen"),
        _ => None,
    };
    known.map_or_else(
        || {
            probes
                .and_then(|probes| probes.iter().find(|probe| probe.engine.as_str() == engine))
                .and_then(|probe| probe.info.as_ref()?.display_name.clone())
                .unwrap_or_else(|| engine.to_owned())
        },
        str::to_owned,
    )
}

pub(super) fn refusal(code: &'static str, message: impl Into<String>) -> TeamRefusal {
    TeamRefusal::new(code, message)
}

/// A runtime or thread refusal, as the model reads it: definitely unsent is
/// a plain refusal; anything else may have happened and is never retried.
fn dispatch_refusal(error: &DispatchError, what: &str) -> TeamRefusal {
    if error.replay_safe {
        refusal(
            thread_team::REFUSED,
            format!("{what} was refused: {}", error.message),
        )
    } else {
        refusal(
            thread_team::OUTCOME_UNKNOWN,
            format!(
                "{what} may or may not have happened ({}). Check with thread_status or list_threads; do not try again with a new requestId.",
                error.message
            ),
        )
    }
}

pub(super) fn unavailable(message: impl Into<String>) -> TeamRefusal {
    refusal(thread_team::UNAVAILABLE, message)
}

pub(super) fn string_field<'a>(input: &'a Value, name: &str) -> Option<&'a str> {
    input.get(name).and_then(Value::as_str)
}

/// A start's title and where it came from: the given one is the owner's
/// kind (`user`), one made from the task may be replaced by a generated one.
pub(super) fn start_title(input: &Value, task: &str) -> (String, TitleSource) {
    match string_field(input, "title") {
        Some(title) => (title.trim().to_owned(), TitleSource::User),
        None => (thread_team::title_from_task(task), TitleSource::Prompt),
    }
}

/// The tools' closed input contracts: exact names and types, bounded text.
#[allow(clippy::too_many_lines)] // One arm per tool and per field.
fn validate_input(name: &str, input: &Value) -> Result<(), TeamRefusal> {
    let (required, optional): (&[&str], &[&str]) = match name {
        "team_info" | "list_threads" | "list_schedules" => (&[], &[]),
        "start_thread" => (
            &["task"],
            &[
                "title",
                "provider",
                "model",
                "workspace",
                "notify",
                "requestId",
            ],
        ),
        "wait_thread" => (&["thread"], &["timeoutSec"]),
        "thread_status" | "stop_thread" => (&["thread"], &[]),
        "read_thread" => (&["thread"], &["after", "limit"]),
        "send_to_thread" => (&["thread", "message"], &["requestId"]),
        "show_visual" => (&["title", "html"], &[]),
        // Protocol 62 (T3 section 7): schedules on the calling thread.
        "schedule_prompt" => (&["prompt", "when"], &["name"]),
        "cancel_schedule" => (&["id"], &[]),
        // Protocol 67 (T3 section 12): its rules are the secrets module's.
        "request_secret" => (&["name", "reason"], &[]),
        _ => return Err(refusal(thread_team::INVALID_INPUT, "Unknown team tool.")),
    };
    validate_fields(name, input, required, optional, |key, value| match key {
        "workspace" => matches!(value.as_str(), Some("same" | "worktree")),
        // Its size and kind are the visual rules' (typed refusals).
        "html" => value.is_string(),
        "title" if name == "show_visual" => value.as_str().is_some_and(|text| {
            !text.trim().is_empty()
                && text.trim().chars().count()
                    <= interface_contracts::thread_journal::MAX_VISUAL_TITLE_CHARS
                && !text.chars().any(char::is_control)
        }),
        // Protocol 62: the scheduling tools' fields (the calling thread's
        // only; outside agents have none of these tools).
        "prompt" => value.as_str().is_some_and(|text| {
            !text.trim().is_empty()
                && text.len() <= interface_settings::thread_schedules::MAX_PROMPT_BYTES
                && !text.contains('\0')
        }),
        "when" => value.is_object(),
        // Protocol 67: `request_secret`'s rules are the secrets module's
        // (typed refusals); a thread caller's only, never an outside one's.
        "name" | "reason" if name == "request_secret" => value.is_string(),
        "name" => value.as_str().is_some_and(|text| {
            !text.trim().is_empty()
                && text.chars().count() <= interface_settings::thread_schedules::MAX_NAME_CHARS
                && !text.chars().any(char::is_control)
        }),
        "id" => value
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 64),
        _ => field_ok(key, value),
    })
}

/// Checks `input` against a closed contract: an object holding every
/// `required` name, nothing beyond `required` and `optional`, and each
/// value as `valid` judges it.
pub(super) fn validate_fields(
    name: &str,
    input: &Value,
    required: &[&str],
    optional: &[&str],
    valid: impl Fn(&str, &Value) -> bool,
) -> Result<(), TeamRefusal> {
    let object = input
        .as_object()
        .ok_or_else(|| refusal(thread_team::INVALID_INPUT, "Tool input must be an object."))?;
    for key in object.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(refusal(
                thread_team::INVALID_INPUT,
                format!("{name} takes no `{key}`."),
            ));
        }
    }
    for key in required {
        if !object.contains_key(*key) {
            return Err(refusal(
                thread_team::INVALID_INPUT,
                format!("{name} needs `{key}`."),
            ));
        }
    }
    for (key, value) in object {
        if !valid(key, value) {
            return Err(refusal(
                thread_team::INVALID_INPUT,
                format!("`{key}` is not valid for {name}."),
            ));
        }
    }
    Ok(())
}

/// The fields team tools and outside agents share, checked the same way.
pub(super) fn field_ok(key: &str, value: &Value) -> bool {
    match key {
        "notify" => value.is_boolean(),
        "timeoutSec" => value
            .as_u64()
            .is_some_and(|secs| secs <= thread_team::WAIT_MAX_SECS),
        "after" => value.is_u64(),
        "limit" => value
            .as_u64()
            .is_some_and(|limit| (1..=2_000).contains(&limit)),
        "task" | "message" => value.as_str().is_some_and(|text| {
            !text.trim().is_empty()
                && text.len() <= thread_team::MAX_TASK_BYTES
                && !text.contains('\0')
        }),
        "title" => value.as_str().is_some_and(|text| {
            !text.trim().is_empty()
                && text.chars().count() <= thread_team::MAX_TITLE_CHARS
                && !text.chars().any(char::is_control)
        }),
        "requestId" => value.as_str().is_some_and(|id| {
            !id.is_empty()
                && id.len() <= thread_team::MAX_REQUEST_ID_BYTES
                && !id.chars().any(char::is_control)
        }),
        "provider" => value.as_str().is_some_and(|engine| {
            engine
                .parse::<interface_contracts::engine::EngineId>()
                .is_ok()
        }),
        "model" => value.as_str().is_some_and(|model| {
            !model.is_empty() && model.len() <= 128 && !model.chars().any(char::is_control)
        }),
        "thread" => value
            .as_str()
            .is_some_and(|key| key.len() <= 512 && parse_key(key).is_ok()),
        _ => false,
    }
}

impl ThreadHost {
    /// Attaches team tools once the guarded runtime exists. Attaching twice
    /// keeps the first.
    pub fn attach_team(&self, config: ThreadTeamConfig) {
        let children = config
            .store
            .as_ref()
            .map(|store| {
                // Every started thread, for the dispatcher's resume check.
                store_children(store)
            })
            .unwrap_or_default();
        let outside = config
            .store
            .as_ref()
            .map(super::outside::OutsideSnapshot::of)
            .unwrap_or_default();
        let _ = self.inner.team.set(TeamParts {
            runtime: config.runtime,
            access: config.access,
            store: Arc::new(Mutex::new(config.store)),
            children: Mutex::new(children),
            notices: Mutex::new(HashSet::new()),
            outside: Mutex::new(outside),
            outside_starts: tokio::sync::Mutex::new(()),
            starts: tokio::sync::Mutex::new(()),
            notice_sends: tokio::sync::Mutex::new(()),
        });
    }

    /// Opens `thread-team.v1.json` in the private data root. A file that does
    /// not decode is quarantined and the ledger starts empty.
    ///
    /// # Errors
    /// Why the store could not open (team tools then refuse every start).
    pub fn open_team_store(data_root: &std::path::Path) -> Result<ThreadTeamStore, String> {
        ThreadTeamStore::open(&data_root.join(thread_team::THREAD_TEAM_FILE_NAME)).map_err(
            |error| {
                tracing::error!(%error, "the team tools ledger did not open; no thread can be started");
                error.to_string()
            },
        )
    }

    pub(super) fn team(&self) -> Result<&TeamParts, TeamRefusal> {
        self.inner
            .team
            .get()
            .ok_or_else(|| unavailable("Team tools are not available in this Interface."))
    }

    /// Something a wait or a notice watches changed.
    pub(crate) fn team_changed(&self) {
        self.inner
            .changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    fn team_changes(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    /// Marks `text` as a prompt Interface sends `key` itself, for the send hook.
    pub(super) fn permit_host_prompt(&self, key: &str, text: &str) {
        lock(&self.inner.host_prompts).insert(key.to_owned(), prompt_digest(text));
    }

    /// Whether `text` is the prompt Interface marked for `key` (taken once).
    pub(super) fn take_host_prompt(&self, key: &str, text: &str) -> bool {
        let mut prompts = lock(&self.inner.host_prompts);
        if prompts
            .get(key)
            .is_some_and(|digest| *digest == prompt_digest(text))
        {
            prompts.remove(key);
            return true;
        }
        false
    }

    /// The guarded runtime every start and send goes through, once attached.
    pub(super) fn guarded_runtime(&self) -> Option<Arc<dyn HostDispatcher>> {
        self.inner.team.get().map(|team| Arc::clone(&team.runtime))
    }

    /// The permission mode the owner chose for `key`, as the session access
    /// memory holds it.
    pub(super) fn access_mode(&self, key: &str) -> Option<String> {
        self.inner
            .team
            .get()?
            .access
            .as_ref()
            .and_then(|access| access.mode(key))
    }

    /// The owner's chosen permission modes, to give a side thread its own
    /// (protocol 67).
    pub(super) fn side_access(&self) -> Option<Arc<dyn SessionAccessMemory>> {
        self.inner.team.get()?.access.clone()
    }

    /// A switch (T3 section 5) moved the thread from `from` to `to`: the
    /// ledger follows its head, so a started thread stays started (and
    /// resumes one hop down) and a caller keeps its started threads. A
    /// failed write is logged: the switch stands either way.
    pub(super) async fn team_follow(&self, from: &str, to: &str) {
        let Some(team) = self.inner.team.get() else {
            return;
        };
        let root = self.journal_root();
        let target = to.to_owned();
        let next_turn = tokio::task::spawn_blocking(move || {
            root.and_then(|root| thread_journal::last_ended_turn(&root, &target))
                .map_or(0, |ended| ended.turn.saturating_add(1))
        })
        .await
        .unwrap_or(0);
        {
            let mut children = lock(&team.children);
            if children.remove(from) {
                children.insert(to.to_owned());
            }
        }
        let (from, to) = (from.to_owned(), to.to_owned());
        let followed = self
            .team_store(move |store| store.follow(&from, &to, next_turn))
            .await;
        if !matches!(followed, Ok(Ok(()))) {
            tracing::warn!("the team tools ledger could not follow a switched thread");
        }
    }

    /// Marks `key` as a started thread in memory, as attaching over a
    /// ledger that lists it would.
    #[cfg(test)]
    pub(crate) fn mark_team_child(&self, key: &str) {
        if let Some(team) = self.inner.team.get() {
            lock(&team.children).insert(key.to_owned());
        }
    }

    /// Whether team tools started `key` (it resumes one hop down).
    #[must_use]
    pub fn is_team_child(&self, key: &str) -> bool {
        self.inner
            .team
            .get()
            .is_some_and(|team| lock(&team.children).contains(key))
    }

    /// Whether a new or resumed session may be offered team tools: the
    /// owner's setting is on, and it is not a thread team tools started.
    pub(crate) async fn team_offered(&self, key: Option<&str>) -> bool {
        if self.inner.team.get().is_none()
            || key.is_some_and(|key| self.is_team_child(key) || self.is_side_thread(key))
        {
            return false;
        }
        self.peek(|document, _| document.settings.team_tools)
            .await
            .unwrap_or(false)
    }

    pub(crate) async fn team_store<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut ThreadTeamStore) -> T + Send + 'static,
    ) -> Result<T, TeamRefusal> {
        let store = Arc::clone(&self.team()?.store);
        let (value, snapshot) = tokio::task::spawn_blocking(move || {
            let mut store = lock(&store);
            match store.as_mut() {
                Ok(store) => {
                    let value = work(store);
                    Ok((value, super::outside::OutsideSnapshot::of(store)))
                }
                Err(reason) => Err(unavailable(format!(
                    "The team tools ledger could not be opened ({reason}). Restart Interface to try again."
                ))),
            }
        })
        .await
        .map_err(|_| unavailable("The team tools ledger stopped unexpectedly."))??;
        *lock(&self.team()?.outside) = snapshot;
        Ok(value)
    }

    /// Runs one tool call for `caller` that never goes (tests). Answers its
    /// JSON text, or the refusal's.
    #[cfg(test)]
    pub(crate) async fn team_call(
        &self,
        caller: &str,
        name: &str,
        input: Value,
    ) -> Result<String, String> {
        self.team_call_until_gone(caller, name, input, CancellationToken::new())
            .await
    }

    /// Runs one tool call for `caller`, which may go before it is answered:
    /// `gone` is cancelled then, and a pending `request_secret` is
    /// withdrawn. Answers its JSON text, or the refusal's.
    pub(crate) async fn team_call_until_gone(
        &self,
        caller: &str,
        name: &str,
        input: Value,
        gone: CancellationToken,
    ) -> Result<String, String> {
        let result = self.team_call_inner(caller, name, &input, gone).await;
        result
            .map(|value| value.to_string())
            .map_err(|refusal| refusal.to_json().to_string())
    }

    async fn team_call_inner(
        &self,
        caller: &str,
        name: &str,
        input: &Value,
        gone: CancellationToken,
    ) -> Result<Value, TeamRefusal> {
        validate_input(name, input)?;
        let caller = self.team_caller(caller).await?;
        // A switched thread is one thread (T3 section 5): an earlier
        // segment's key reaches its head.
        let thread = match string_field(input, "thread") {
            Some(thread) => self.head_key(thread).await,
            None => String::new(),
        };
        let thread = thread.as_str();
        match name {
            "team_info" => self.team_info(&caller).await,
            "start_thread" => {
                let request = string_field(input, "requestId").map(str::to_owned);
                self.with_receipt(&caller.key, request.as_deref(), name, input, || {
                    self.start_thread(&caller, input)
                })
                .await
            }
            "wait_thread" => {
                let secs = input
                    .get("timeoutSec")
                    .and_then(Value::as_u64)
                    .unwrap_or(thread_team::WAIT_DEFAULT_SECS);
                self.wait_thread(caller.who(), thread, secs).await
            }
            "thread_status" => self.wait_thread(caller.who(), thread, 0).await,
            "stop_thread" => self.stop_thread(caller.who(), thread).await,
            "list_threads" => self.list_threads(caller.who(), None).await,
            "read_thread" => {
                self.read_thread(
                    caller.who(),
                    thread,
                    input.get("after").and_then(Value::as_u64),
                    input
                        .get("limit")
                        .and_then(Value::as_u64)
                        .and_then(|limit| u32::try_from(limit).ok())
                        .unwrap_or(200),
                )
                .await
            }
            "send_to_thread" => {
                let request = string_field(input, "requestId").map(str::to_owned);
                self.with_receipt(&caller.key, request.as_deref(), name, input, || {
                    self.send_to_thread(
                        caller.who(),
                        thread,
                        string_field(input, "message").unwrap_or_default(),
                    )
                })
                .await
            }
            "show_visual" => {
                self.show_visual(
                    &caller,
                    string_field(input, "title").unwrap_or_default(),
                    string_field(input, "html").unwrap_or_default(),
                )
                .await
            }
            // Only ever the calling thread's own schedules.
            "schedule_prompt" => self
                .team_schedule_prompt(&caller.key, input)
                .await
                .map_err(|message| refusal(thread_team::REFUSED, message)),
            "list_schedules" => Ok(self.team_list_schedules(&caller.key)),
            "cancel_schedule" => self
                .team_cancel_schedule(&caller.key, string_field(input, "id").unwrap_or_default())
                .await
                .map_err(|message| refusal(thread_team::NOT_FOUND, message)),
            "request_secret" => {
                self.team_request_secret(
                    &caller.key,
                    provider_label(&caller.engine, None),
                    string_field(input, "name").unwrap_or_default(),
                    string_field(input, "reason").unwrap_or_default(),
                    gone,
                )
                .await
            }
            _ => Err(refusal(thread_team::INVALID_INPUT, "Unknown team tool.")),
        }
    }

    /// `show_visual` (T3 section 8): checks the page, keeps it in the
    /// thread's visual store, journals it and shows it in the thread's pane
    /// as one `visual.shown` event. A refusal keeps and shows nothing.
    async fn show_visual(
        &self,
        caller: &Caller,
        title: &str,
        html: &str,
    ) -> Result<Value, TeamRefusal> {
        if let Err(refused) = thread_visuals::check_visual(title, html) {
            let code = match refused {
                VisualRefusal::TooLarge { .. } => thread_team::VISUAL_TOO_LARGE,
                VisualRefusal::NotHtml => thread_team::VISUAL_NOT_HTML,
                VisualRefusal::InvalidTitle => thread_team::INVALID_INPUT,
            };
            return Err(refusal(code, refused.message()));
        }
        // A visual belongs to the thread: an earlier segment's key reaches
        // its head (T3 section 5).
        let key = self.head_key(&caller.key).await;
        let at = self.now();
        let id = thread_visuals::visual_id(
            at,
            format!("{key}\n{at}\n{}", prompt_digest(html)).as_bytes(),
        );
        let title = title.trim().to_owned();
        let saved = self
            .journal_visual(&key, id.clone(), title.clone(), html.to_owned())
            .await
            .map_err(unavailable)?;
        let event = EngineEvent {
            at: Some(interface_common::time::unix_millis_iso(
                i64::try_from(at).unwrap_or(i64::MAX),
                interface_common::time::IsoFraction::Millis,
            )),
            event: EngineEventKind::VisualShown {
                id: id.clone(),
                title,
                bytes: saved.bytes,
            },
        };
        // Display only: the page is kept and journalled, so a pane that
        // missed the event shows it from history.
        if !self.push_session_event(&key, &event).await {
            tracing::warn!("a visual was kept but not shown in its pane");
        }
        Ok(json!({"id": id, "bytes": saved.bytes}))
    }

    /// A call with a `requestId`: a retry answers the first call's result,
    /// one whose outcome is unknown answers so and does nothing, and the
    /// receipt is reserved before the effect.
    pub(super) async fn with_receipt<F, Fut>(
        &self,
        caller: &str,
        request_id: Option<&str>,
        tool: &str,
        input: &Value,
        effect: F,
    ) -> Result<Value, TeamRefusal>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Value, TeamRefusal>>,
    {
        let Some(request_id) = request_id else {
            return effect().await;
        };
        let digest = thread_team::call_digest(tool, input);
        let (owner, id, tool_name, held) = (
            caller.to_owned(),
            request_id.to_owned(),
            tool.to_owned(),
            digest.clone(),
        );
        let decision = self
            .team_store(move |store| {
                let decision = thread_team::receipt_decision(store.receipt(&owner, &id), &held);
                if decision == ReceiptDecision::Fresh {
                    store
                        .reserve(&owner, &id, &tool_name, &held, crate::threads::now_ms())
                        .map(|()| decision)
                        .map_err(|error| error.to_string())
                } else {
                    Ok(decision)
                }
            })
            .await?
            .map_err(|error| {
                unavailable(format!(
                    "The call's receipt could not be saved ({error}); nothing was done."
                ))
            })?;
        match decision {
            ReceiptDecision::Replay(result) => {
                let value: Value = serde_json::from_str(&result).unwrap_or(Value::Null);
                if let (Some(code), Some(message)) = (
                    value.get("code").and_then(Value::as_str),
                    value.get("message").and_then(Value::as_str),
                ) {
                    let code = KNOWN_CODES
                        .iter()
                        .find(|known| **known == code)
                        .copied()
                        .unwrap_or(thread_team::REFUSED);
                    return Err(refusal(code, message));
                }
                return Ok(value);
            }
            ReceiptDecision::Unknown => {
                return Err(refusal(
                    thread_team::OUTCOME_UNKNOWN,
                    "The first call with this requestId has no known outcome; it is never tried again. Check with list_threads or thread_status.",
                ));
            }
            ReceiptDecision::Reused => {
                return Err(refusal(
                    thread_team::REQUEST_REUSED,
                    "That requestId was used for a different call; use a new one.",
                ));
            }
            ReceiptDecision::Fresh => {}
        }
        let result = effect().await;
        let settled = match &result {
            Ok(value) => Some(value.to_string()),
            Err(refusal) if refusal.code != thread_team::OUTCOME_UNKNOWN => {
                Some(refusal.to_json().to_string())
            }
            // An unknown outcome keeps its receipt reserved: a retry says so.
            Err(_) => None,
        };
        if let Some(settled) = settled {
            let (owner, id) = (caller.to_owned(), request_id.to_owned());
            match self
                .team_store(move |store| store.settle(&owner, &id, &settled))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, "a team tools receipt could not be settled");
                }
                Err(error) => tracing::warn!(
                    code = error.code,
                    "a team tools receipt could not be settled"
                ),
            }
        }
        result
    }

    async fn team_caller(&self, key: &str) -> Result<Caller, TeamRefusal> {
        self.team()?;
        if self.is_team_child(key) {
            return Err(refusal(
                thread_team::NO_NESTING,
                "A thread started by team tools cannot use them.",
            ));
        }
        let record = self.caller_record(key).await?.ok_or_else(|| {
            refusal(
                thread_team::NOT_A_THREAD,
                "Team tools work only in a Threads thread.",
            )
        })?;
        let summary = self.summary(key);
        let (engine, _) = parse_key(key)
            .map_err(|_| refusal(thread_team::NOT_A_THREAD, "This session has no thread key."))?;
        let workspace = record.workspace.clone();
        let project = workspace
            .as_ref()
            .filter(|workspace| workspace.mode != WorkspaceMode::None)
            .map(|workspace| workspace.project_root.clone())
            .filter(|root| !root.is_empty());
        let mode = self
            .team()?
            .access
            .as_ref()
            .and_then(|access| access.mode(key));
        Ok(Caller {
            key: key.to_owned(),
            engine: engine.as_str().to_owned(),
            title: summary
                .as_ref()
                .map_or_else(String::new, |summary| summary.title.clone()),
            model: summary.and_then(|summary| summary.model),
            mode,
            project,
            workspace,
        })
    }

    /// The caller's thread record. A new thread's first prompt can make its
    /// agent call a tool before the launch's bind files the thread (the
    /// session runs first, then is bound): while the store says that bind
    /// is awaited (`ThreadsDocument::awaits_bind`, a prepared workspace for
    /// the session's folder not yet bound), this waits for it, waking on
    /// each store change, for at most [`BIND_WAIT`]; then answers what the
    /// record says, so an unbound session is refused as before.
    async fn caller_record(
        &self,
        key: &str,
    ) -> Result<Option<interface_settings::threads::ThreadRecord>, TeamRefusal> {
        let cwd = self
            .summary(key)
            .map(|summary| summary.cwd)
            .unwrap_or_default();
        let deadline = tokio::time::Instant::now() + BIND_WAIT;
        let mut changes = self.team_changes();
        let mut stop = self.inner.stop.subscribe();
        let mut told = false;
        loop {
            changes.borrow_and_update();
            let (owned, folder) = (key.to_owned(), cwd.clone());
            let (record, awaited) = self
                .peek(move |document, _| {
                    (
                        document.record(&owned).cloned(),
                        document.awaits_bind(&owned, &folder),
                    )
                })
                .await
                .map_err(|error| unavailable(error.message))?;
            if !awaited {
                return Ok(record);
            }
            if !told {
                told = true;
                #[cfg(test)]
                self.inner.bind_waits.notify_one();
            }
            tokio::select! {
                moved = changes.changed() => if moved.is_err() { return Ok(record); },
                () = tokio::time::sleep_until(deadline) => return Ok(record),
                _ = stop.changed() => return Ok(record),
            }
        }
    }

    pub(super) fn summary(&self, key: &str) -> Option<SessionSummary> {
        self.inner.ports.sessions.session(key).ok().flatten()
    }

    pub(crate) fn journal_root(&self) -> Option<PathBuf> {
        self.inner
            .journal
            .as_ref()
            .map(super::journal::JournalHandle::journal_root)
    }

    pub(super) async fn probes(&self) -> Vec<EngineProbe> {
        let Ok(team) = self.team() else {
            return Vec::new();
        };
        match team
            .runtime
            .dispatch(CHANNELS.probe_engines, Value::Null)
            .await
        {
            Ok(value) => serde_json::from_value(value).unwrap_or_default(),
            Err(error) => {
                tracing::warn!(code = %error.code, "team tools could not read the providers");
                Vec::new()
            }
        }
    }

    async fn team_info(&self, caller: &Caller) -> Result<Value, TeamRefusal> {
        let probes = self.probes().await;
        let providers = probes
            .iter()
            .map(|probe| {
                let engine = probe.engine.as_str();
                let signed_out = probe.info.as_ref().and_then(|info| info.auth)
                    == Some(interface_contracts::engine::EngineAuth::SignedOut);
                let ready = probe.ok && probe.info.is_some() && !signed_out;
                let mut models: Vec<String> = Vec::new();
                if let Some(info) = &probe.info {
                    if let Some(catalog) = &info.model_catalog {
                        let mut sorted = catalog.iter().collect::<Vec<_>>();
                        sorted.sort_by_key(|model| model.is_default != Some(true));
                        models.extend(sorted.into_iter().map(|model| model.value.clone()));
                    } else if let interface_contracts::engine::EngineModels::Models(list) =
                        &info.capabilities.models
                    {
                        models.extend(list.iter().cloned());
                    }
                }
                models.truncate(8);
                let mut provider = json!({
                    "provider": engine,
                    "label": provider_label(engine, Some(&probes)),
                    "ready": ready,
                    "models": models,
                });
                if !ready {
                    provider["reason"] = Value::String(if signed_out {
                        "signed out".into()
                    } else {
                        probe
                            .error
                            .clone()
                            .unwrap_or_else(|| "not available".into())
                    });
                }
                provider
            })
            .collect::<Vec<_>>();
        let workspace = match caller.workspace.as_ref().map(|workspace| workspace.mode) {
            Some(WorkspaceMode::Local) => "local",
            Some(WorkspaceMode::Worktree) => "worktree",
            _ => "none",
        };
        Ok(json!({
            "you": {
                "thread": caller.key,
                "title": caller.title,
                "provider": caller.engine,
                "model": caller.model,
                "mode": caller.mode.clone().unwrap_or_else(|| "default".into()),
                "project": caller.project,
                "workspace": workspace,
            },
            "providers": providers,
            "limits": {
                "runningThreads": thread_team::MAX_RUNNING_PER_CALLER,
                "totalThreads": thread_team::MAX_STARTED_PER_CALLER,
            },
        }))
    }

    /// `key`'s status from the run tap, its record and its journal.
    async fn status_of(&self, key: &str) -> Result<StatusView, TeamRefusal> {
        let owned = key.to_owned();
        let record = self
            .peek(move |document, _| document.record(&owned).cloned())
            .await
            .map_err(|error| unavailable(error.message))?;
        let tap_open = self.run_tracked(key);
        let waiting = self.run_waiting(key).map(|question| {
            if question {
                thread_team::Needs::Question
            } else {
                thread_team::Needs::Approval
            }
        });
        let record_open = record
            .as_ref()
            .is_some_and(interface_settings::threads::ThreadRecord::running);
        let outcome = record
            .as_ref()
            .and_then(|record| record.last_run.as_ref())
            .and_then(|run| run.outcome);
        let root = self.journal_root();
        let journal_key = key.to_owned();
        let (journal_open, ended) = tokio::task::spawn_blocking(move || {
            root.map_or((false, None), |root| {
                (
                    thread_journal::watch(&root, &journal_key).turn_open,
                    thread_journal::last_ended_turn(&root, &journal_key),
                )
            })
        })
        .await
        .unwrap_or((false, None));
        let (status, needs) = thread_team::status(StatusFacts {
            run_open: tap_open || record_open,
            journal_open,
            waiting,
            outcome,
        });
        Ok(StatusView {
            status,
            needs,
            ended,
        })
    }

    /// The target thread, readable by `who`: in the registry, a thread,
    /// not hidden, not an agent's, with a project its reach allows.
    pub(super) async fn readable(
        &self,
        who: Who<'_>,
        thread: &str,
    ) -> Result<SessionSummary, TeamRefusal> {
        let not_found = || {
            refusal(
                thread_team::NOT_FOUND,
                format!("There is no thread {thread} {}.", who.reach.where_()),
            )
        };
        let summary = self.summary(thread).ok_or_else(not_found)?;
        if summary.kind == Some(SessionKind::Agent)
            || self.inner.ports.hidden.hidden_keys().await.contains(thread)
        {
            return Err(not_found());
        }
        let owned = thread.to_owned();
        let project = self
            .peek(move |document, _| {
                document.record(&owned).map(|record| {
                    record
                        .workspace
                        .as_ref()
                        .filter(|workspace| workspace.mode != WorkspaceMode::None)
                        .map(|workspace| workspace.project_root.clone())
                })
            })
            .await
            .map_err(|error| unavailable(error.message))?
            .ok_or_else(not_found)?;
        // An outside session reaches only threads with a project; any other
        // is not there for it.
        if matches!(who.reach, Reach::AnyProject) && !who.reach.allows(project.as_deref()) {
            return Err(not_found());
        }
        if !who.reach.allows(project.as_deref()) {
            return Err(refusal(
                thread_team::OTHER_PROJECT,
                "That thread belongs to another project; team tools reach only this thread's project.",
            ));
        }
        Ok(summary)
    }

    /// The target thread, controllable by `who`: one it started.
    async fn controllable(
        &self,
        who: Who<'_>,
        thread: &str,
    ) -> Result<SessionSummary, TeamRefusal> {
        let summary = self.readable(who, thread).await?;
        let owned = thread.to_owned();
        let parent = self
            .team_store(move |store| store.started(&owned).map(|entry| entry.parent.clone()))
            .await?;
        if !thread_team::may_control(parent.as_deref(), who.key) {
            return Err(refusal(
                thread_team::NOT_STARTED_BY_YOU,
                if thread_team::is_outside_caller(who.key) {
                    "Only a thread this session started can be sent to or stopped."
                } else {
                    "Only a thread this thread started can be sent to or stopped."
                },
            ));
        }
        Ok(summary)
    }

    fn status_json(thread: &str, title: &str, view: &StatusView, timed_out: bool) -> Value {
        let mut result = json!({
            "thread": thread,
            "title": title,
            "status": view.status.as_str(),
            "waitTimedOut": timed_out,
        });
        if let Some(needs) = view.needs {
            result["needs"] = needs.as_str().into();
        }
        if matches!(
            view.status,
            TeamStatus::Done | TeamStatus::Failed | TeamStatus::Stopped
        ) && let Some(ended) = &view.ended
        {
            result["turn"] = ended.turn.into();
            if let Some(answer) = &ended.answer {
                let (answer, cut) = thread_team::cut_bytes(answer, thread_team::ANSWER_LIMIT_BYTES);
                result["answer"] = answer.into();
                if cut {
                    result["answerCut"] = true.into();
                }
            }
            if view.status == TeamStatus::Failed
                && let Some(error) = &ended.error
            {
                result["error"] = error.as_str().into();
            }
        }
        result
    }

    pub(super) async fn wait_thread(
        &self,
        who: Who<'_>,
        thread: &str,
        secs: u64,
    ) -> Result<Value, TeamRefusal> {
        let summary = self.readable(who, thread).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        let mut revisions = self.team_changes();
        let mut stop = self.inner.stop.subscribe();
        let (view, timed_out) = loop {
            revisions.borrow_and_update();
            let view = self.status_of(thread).await?;
            if view.status.settles_wait() || secs == 0 {
                break (view, false);
            }
            tokio::select! {
                moved = revisions.changed() => {
                    if moved.is_err() {
                        break (view, false);
                    }
                }
                () = tokio::time::sleep_until(deadline) => break (self.status_of(thread).await?, true),
                _ = stop.changed() => break (view, false),
            }
        };
        let timed_out = timed_out && !view.status.settles_wait();
        // The caller has this result now: no notice repeats it.
        if matches!(
            view.status,
            TeamStatus::Done | TeamStatus::Failed | TeamStatus::Stopped
        ) && let Some(turn) = view.ended.as_ref().map(|ended| ended.turn)
        {
            let (child, parent) = (thread.to_owned(), who.key.to_owned());
            let marked = self
                .team_store(move |store| {
                    if store
                        .started(&child)
                        .is_some_and(|entry| entry.parent == parent)
                    {
                        store
                            .mark_delivered(&child, turn, DeliveryHow::Read)
                            .map_err(|error| error.to_string())
                    } else {
                        Ok(())
                    }
                })
                .await;
            if !matches!(marked, Ok(Ok(()))) {
                tracing::warn!("a started thread's read result could not be recorded");
            }
        }
        Ok(Self::status_json(thread, &summary.title, &view, timed_out))
    }

    pub(super) async fn stop_thread(
        &self,
        who: Who<'_>,
        thread: &str,
    ) -> Result<Value, TeamRefusal> {
        let summary = self.controllable(who, thread).await?;
        let view = self.status_of(thread).await?;
        if view.status.settles_wait() && view.status != TeamStatus::NeedsYou {
            return Err(refusal(
                thread_team::THREAD_IDLE,
                format!("{thread} is not running ({}).", view.status.as_str()),
            ));
        }
        self.team()?
            .runtime
            .dispatch(CHANNELS.interrupt, json!({"sessionKey": thread}))
            .await
            .map_err(|error| dispatch_refusal(&error, "Stopping the thread"))?;
        Ok(json!({"thread": thread, "title": summary.title, "stopped": true}))
    }

    /// Up to [`thread_team::LIST_LIMIT`] threads `who` reaches, newest
    /// activity first; `project` (an outside session's filter) keeps one
    /// project's and adds each thread's project to its row.
    pub(super) async fn list_threads(
        &self,
        who: Who<'_>,
        project: Option<&str>,
    ) -> Result<Value, TeamRefusal> {
        let outside = matches!(who.reach, Reach::AnyProject);
        let ((), view) = self
            .read(|_, _| ())
            .await
            .map_err(|error| unavailable(error.message))?;
        let summaries = self
            .inner
            .ports
            .sessions
            .sessions()
            .map_err(unavailable)?
            .into_iter()
            .map(|summary| (session_key(&summary.engine, &summary.session_id), summary))
            .collect::<HashMap<_, _>>();
        let hidden = self.inner.ports.hidden.hidden_keys().await;
        let started_by_caller = {
            let parent = who.key.to_owned();
            self.team_store(move |store| {
                store
                    .children(&parent)
                    .into_iter()
                    .map(|entry| entry.child.clone())
                    .collect::<HashSet<_>>()
            })
            .await
            .unwrap_or_default()
        };
        let mut threads = view
            .threads
            .iter()
            .filter(|thread| thread.key != who.key && !hidden.contains(&thread.key))
            .filter_map(|thread| {
                let summary = summaries.get(&thread.key)?;
                if summary.kind == Some(SessionKind::Agent) {
                    return None;
                }
                let root = thread
                    .workspace
                    .as_ref()
                    .filter(|workspace| workspace.mode != WorkspaceMode::None)
                    .map(|workspace| workspace.project_root.as_str());
                if !who.reach.allows(root) || project.is_some_and(|wanted| root != Some(wanted)) {
                    return None;
                }
                let run = thread.last_run.as_ref();
                let running =
                    self.run_tracked(&thread.key) || run.is_some_and(|run| run.ended_at.is_none());
                let (status, _) = thread_team::status(StatusFacts {
                    run_open: running,
                    journal_open: false,
                    waiting: self.run_waiting(&thread.key).map(|question| {
                        if question {
                            thread_team::Needs::Question
                        } else {
                            thread_team::Needs::Approval
                        }
                    }),
                    outcome: run.and_then(|run| run.outcome),
                });
                let last_activity = run
                    .map_or(thread.created_at, |run| {
                        run.ended_at.unwrap_or(run.started_at)
                    })
                    .max(thread.created_at);
                let mut row = json!({
                    "thread": thread.key,
                    "title": summary.title,
                    "provider": summary.engine.as_str(),
                    "status": status.as_str(),
                    "section": serde_json::to_value(thread.section).unwrap_or(Value::Null),
                    "startedByYou": started_by_caller.contains(&thread.key),
                    "lastActivityAt": last_activity,
                });
                if outside {
                    row["project"] = root.unwrap_or_default().into();
                }
                Some((last_activity, row))
            })
            .collect::<Vec<_>>();
        threads.sort_by(|left, right| right.0.cmp(&left.0));
        threads.truncate(thread_team::LIST_LIMIT);
        Ok(json!({"threads": threads.into_iter().map(|(_, thread)| thread).collect::<Vec<_>>()}))
    }

    pub(super) async fn read_thread(
        &self,
        who: Who<'_>,
        thread: &str,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Value, TeamRefusal> {
        let summary = self.readable(who, thread).await?;
        let root = self
            .journal_root()
            .ok_or_else(|| unavailable("Threads keep no journal in this Interface."))?;
        let key = thread.to_owned();
        let page =
            tokio::task::spawn_blocking(move || thread_journal::read(&root, &key, after, limit))
                .await
                .map_err(|_| unavailable("The thread's journal could not be read."))?;
        let (items, next) = thread_team::compact_page(&page.entries, page.has_more);
        let mut result = json!({"thread": thread, "title": summary.title, "items": items});
        if let Some(next) = next {
            result["next"] = next.into();
        }
        Ok(result)
    }

    pub(super) async fn send_to_thread(
        &self,
        who: Who<'_>,
        thread: &str,
        message: &str,
    ) -> Result<Value, TeamRefusal> {
        let summary = self.controllable(who, thread).await?;
        let view = self.status_of(thread).await?;
        if !view.status.settles_wait() || view.status == TeamStatus::NeedsYou {
            return Err(refusal(
                thread_team::THREAD_BUSY,
                format!(
                    "{thread} is {}; a message goes only to an idle thread (wait_thread first). Nothing was sent.",
                    view.status.as_str()
                ),
            ));
        }
        self.permit_host_prompt(thread, message);
        let sent = self
            .team()?
            .runtime
            .dispatch(
                CHANNELS.send,
                json!({"sessionKey": thread, "text": message}),
            )
            .await;
        if let Err(error) = sent {
            self.take_host_prompt(thread, message);
            return Err(dispatch_refusal(&error, "The message"));
        }
        Ok(json!({"thread": thread, "title": summary.title, "status": "running"}))
    }

    /// Starts a thread for `caller` (see the module docs).
    async fn start_thread(&self, caller: &Caller, input: &Value) -> Result<Value, TeamRefusal> {
        let project = caller.project.clone().ok_or_else(|| {
            refusal(
                thread_team::NO_PROJECT,
                "This thread has no project folder, so it cannot start threads.",
            )
        })?;
        let workspace = caller
            .workspace
            .clone()
            .ok_or_else(|| refusal(thread_team::NO_PROJECT, "This thread has no workspace."))?;
        let task = string_field(input, "task").unwrap_or_default().to_owned();
        let provider = string_field(input, "provider")
            .unwrap_or(&caller.engine)
            .to_owned();
        let model = string_field(input, "model").map(str::to_owned).or_else(|| {
            (provider == caller.engine)
                .then(|| caller.model.clone())
                .flatten()
        });
        let (title, title_source) = start_title(input, &task);
        let notify = input.get("notify").and_then(Value::as_bool).unwrap_or(true);
        let new_worktree = string_field(input, "workspace") == Some("worktree");
        let (_, mode) =
            thread_team::started_mode(&caller.engine, caller.mode.as_deref(), &provider)?;
        // Limits: running (or waiting on the owner) and in all, checked and
        // used up one start at a time.
        let team = self.team()?;
        let _one_at_a_time = team.starts.lock().await;
        let parent = caller.key.clone();
        let (children, ledger) = self
            .team_store(move |store| {
                (
                    store
                        .children(&parent)
                        .into_iter()
                        .map(|entry| entry.child.clone())
                        .collect::<Vec<_>>(),
                    store.len(),
                )
            })
            .await?;
        let running = self.running_among(&children).await?;
        thread_team::check_start_limits(running, children.len(), ledger)?;
        // The workspace: the caller's own folder, or a new worktree of it.
        let (prepare, base, existing) = if new_worktree {
            (
                "worktree",
                (workspace.mode == WorkspaceMode::Worktree)
                    .then(|| workspace.branch.clone())
                    .flatten(),
                None,
            )
        } else if workspace.mode == WorkspaceMode::Worktree {
            ("existing", None, workspace.worktree_path.clone())
        } else {
            ("local", None, None)
        };
        let account = self
            .accounts()
            .and_then(|accounts| accounts.started_thread_account(&caller.key, &provider));
        self.start_child(StartPlan {
            parent: caller.key.clone(),
            started_by: None,
            project,
            prepare,
            base,
            existing,
            lineage: Some(ThreadLineage {
                parent: caller.key.clone(),
                relation: LineageRelation::Child,
                kept_turns: 0,
                at: 0,
            }),
            task,
            provider,
            model,
            title,
            title_source,
            notify,
            mode,
            // Protocol 60: the caller's account on the caller's provider,
            // else the target provider's default account.
            account,
        })
        .await
    }

    /// How many of `children` run or wait on the owner now.
    pub(super) async fn running_among(&self, children: &[String]) -> Result<usize, TeamRefusal> {
        let mut running = 0;
        for child in children {
            if !self.status_of(child).await?.status.settles_wait()
                || self.run_waiting(child).is_some()
            {
                running += 1;
            }
        }
        Ok(running)
    }

    /// Starts the thread `plan` describes: its workspace, its session at
    /// collaboration depth 1, the ledger entry, its bind, then its task
    /// through the send hook (see the module docs). Every rule that chose
    /// the plan is the caller's to have checked.
    #[allow(clippy::too_many_lines)] // One ordered effect: workspace, session, ledger, bind, task.
    pub(super) async fn start_child(&self, plan: StartPlan) -> Result<Value, TeamRefusal> {
        let StartPlan {
            parent,
            started_by,
            project,
            prepare,
            base,
            existing,
            lineage,
            task,
            provider,
            model,
            title,
            title_source,
            notify,
            mode,
            account,
        } = plan;
        let hint = format!("team {title}");
        let prepared = self
            .prepare_workspace_vetted(project, prepare, base, existing, hint, lineage, true)
            .await
            .map_err(|error| dispatch_refusal(&error, "Preparing the thread's workspace"))?
            .prepared
            .ok_or_else(|| {
                refusal(
                    thread_team::REFUSED,
                    "The workspace's setup did not finish; nothing was started.",
                )
            })?;
        // An ACP agent refuses an empty launch: its task rides the launch.
        let launch_task = provider.starts_with("acp:");
        let options = LaunchOptions {
            cwd: prepared.cwd.clone(),
            prompt: if launch_task {
                task.clone()
            } else {
                String::new()
            },
            model: model.clone(),
            permission_mode: Some(mode.to_owned()),
            name: Some(title.clone()),
            kind: Some(SessionKind::Code),
            collaboration_depth: Some(1),
            account,
            ..LaunchOptions::default()
        };
        let runtime = Arc::clone(&self.team()?.runtime);
        let created = runtime
            .dispatch(
                CHANNELS.create_session,
                json!({"engine": provider, "options": options}),
            )
            .await;
        let child = match created {
            Ok(value) => value
                .get("key")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    refusal(
                        thread_team::OUTCOME_UNKNOWN,
                        "The provider started without naming its session; check list_threads before starting again.",
                    )
                })?,
            Err(error) => {
                if error.replay_safe {
                    self.away_discard(&prepared.token).await;
                }
                return Err(dispatch_refusal(&error, "Starting the thread"));
            }
        };
        // The ledger first: the thread is this caller's from now on.
        let entry = StartedThread {
            parent,
            child: child.clone(),
            started_at: crate::threads::now_ms(),
            notify,
            delivered: None,
            started_by,
        };
        let recorded = self
            .team_store(move |store| store.record_start(entry).map_err(|error| error.to_string()))
            .await?;
        if let Err(error) = recorded {
            return Err(refusal(
                thread_team::OUTCOME_UNKNOWN,
                format!(
                    "{child} started, but it could not be recorded as this caller's ({error}); its task was not sent."
                ),
            ));
        }
        lock(&self.team()?.children).insert(child.clone());
        let bound = if launch_task {
            self.bind(
                child.clone(),
                Some(prepared.token.clone()),
                task.clone(),
                Some(title_source),
            )
            .await
            .map(|_| ())
        } else {
            self.bind_empty_thread(&child, &prepared.token, title_source)
                .await
        };
        if let Err(error) = bound {
            return Err(refusal(
                thread_team::OUTCOME_UNKNOWN,
                format!(
                    "{child} started but could not be filed as a thread ({}); its task was not sent.",
                    error.message
                ),
            ));
        }
        if !launch_task {
            self.permit_host_prompt(&child, &task);
            if let Err(error) = runtime
                .dispatch(CHANNELS.send, json!({"sessionKey": child, "text": task}))
                .await
            {
                self.take_host_prompt(&child, &task);
                let mut refused = dispatch_refusal(&error, "Sending the task");
                refused.message = format!("{child} started, but {}", refused.message);
                return Err(refused);
            }
        }
        Ok(json!({
            "thread": child,
            "title": title,
            "provider": provider,
            "model": model,
            "status": "running",
        }))
    }

    /// A started thread's run ended: its caller gets one notice, once it is
    /// idle, unless it has the result already (the worker calls this after
    /// the run's end is recorded).
    pub(crate) fn team_run_ended(&self, key: &str) {
        if !self.is_team_child(key) {
            return;
        }
        let Ok(team) = self.team() else {
            return;
        };
        if !lock(&team.notices).insert(key.to_owned()) {
            return;
        }
        let host = self.clone();
        let child = key.to_owned();
        self.spawn_tracked(async move {
            host.notice(&child).await;
            if let Ok(team) = host.team() {
                lock(&team.notices).remove(&child);
            }
        });
    }

    /// A deleted or forgotten session leaves the ledger.
    pub(crate) fn team_removed(&self, key: &str) {
        let Ok(team) = self.team() else {
            return;
        };
        lock(&team.children).remove(key);
        let host = self.clone();
        let owned = key.to_owned();
        self.spawn_tracked(async move {
            if !matches!(
                host.team_store(move |store| store.forget(&owned)).await,
                Ok(Ok(()))
            ) {
                tracing::warn!("a removed session's team tools entries could not be forgotten");
            }
        });
    }

    async fn notice(&self, child: &str) {
        let mut revisions = self.team_changes();
        let mut stop = self.inner.stop.subscribe();
        loop {
            revisions.borrow_and_update();
            if self.notice_step(child).await == NoticeStep::Done {
                return;
            }
            tokio::select! {
                moved = revisions.changed() => if moved.is_err() { return; },
                _ = stop.changed() => return,
            }
        }
    }

    /// One look at a finished thread's notice: sent, dropped or not needed
    /// (`Done`), or waiting for the child's journal or the caller's turn to
    /// end (`Wait`).
    pub(crate) async fn notice_step(&self, child: &str) -> NoticeStep {
        let owned = child.to_owned();
        let Ok(Some(entry)) = self
            .team_store(move |store| store.started(&owned).cloned())
            .await
        else {
            return NoticeStep::Done;
        };
        let Ok(view) = self.status_of(child).await else {
            return NoticeStep::Done;
        };
        let ended = match view.status {
            // The owner is asked first; the inbox already says so.
            TeamStatus::NeedsYou => return NoticeStep::Done,
            TeamStatus::Running => return NoticeStep::Wait,
            _ => match view.ended {
                Some(ended) => ended,
                None => return NoticeStep::Done,
            },
        };
        let Ok(team) = self.team() else {
            return NoticeStep::Done;
        };
        let _one_at_a_time = team.notice_sends.lock().await;
        let facts = self.caller_facts(&entry, &ended).await;
        match thread_team::notice_decision(facts) {
            NoticeDecision::None => NoticeStep::Done,
            NoticeDecision::Drop => {
                self.mark_notice(child, ended.turn, DeliveryHow::Dropped)
                    .await;
                NoticeStep::Done
            }
            NoticeDecision::Send => {
                self.send_notice(&entry, ended).await;
                NoticeStep::Done
            }
            NoticeDecision::Wait => NoticeStep::Wait,
        }
    }

    async fn caller_facts(&self, entry: &StartedThread, ended: &EndedTurn) -> NoticeFacts {
        let parent = entry.parent.clone();
        let record = self
            .peek(move |document, _| document.record(&parent).cloned())
            .await
            .ok()
            .flatten();
        let summary = self.summary(&entry.parent);
        NoticeFacts {
            notify: entry.notify,
            outcome: Some(ended.outcome),
            handled: entry
                .delivered
                .is_some_and(|delivered| delivered.turn >= ended.turn),
            caller_settled: record
                .as_ref()
                .is_none_or(|record| record.settled_at.is_some()),
            caller_archived: summary.as_ref().is_some_and(|summary| summary.archived),
            caller_away: lock(&self.inner.away_holds).contains_key(&entry.parent),
            caller_pane: self.inner.ports.claims.claim(&entry.parent).is_some(),
            caller_running: self.run_tracked(&entry.parent)
                || record
                    .as_ref()
                    .is_some_and(interface_settings::threads::ThreadRecord::running)
                || summary.as_ref().is_some_and(|summary| {
                    matches!(
                        summary.status,
                        SessionStatus::Working | SessionStatus::Waiting
                    )
                }),
        }
    }

    async fn mark_notice(&self, child: &str, turn: u32, how: DeliveryHow) -> bool {
        let owned = child.to_owned();
        let marked = self
            .team_store(move |store| store.mark_delivered(&owned, turn, how))
            .await;
        if !matches!(marked, Ok(Ok(()))) {
            tracing::warn!("a started thread's notice could not be recorded");
            return false;
        }
        true
    }

    /// One attempt: recorded as unknown before it goes out, so a restart
    /// never sends it again, then as what became of it.
    async fn send_notice(&self, entry: &StartedThread, ended: EndedTurn) {
        if !self
            .mark_notice(&entry.child, ended.turn, DeliveryHow::Unknown)
            .await
        {
            return;
        }
        let Ok(team) = self.team() else {
            return;
        };
        let title = self
            .summary(&entry.child)
            .map_or_else(|| entry.child.clone(), |summary| summary.title);
        let engine = parse_key(&entry.child)
            .map(|(engine, _)| engine.as_str().to_owned())
            .unwrap_or_default();
        let text = thread_team::notice_text(
            &title,
            &provider_label(&engine, None),
            ended.outcome,
            ended.answer.as_deref(),
            ended.error.as_deref(),
        );
        self.permit_host_prompt(&entry.parent, &text);
        let sent = team
            .runtime
            .dispatch(
                CHANNELS.send,
                json!({"sessionKey": entry.parent, "text": text}),
            )
            .await;
        let how = match &sent {
            Ok(_) => DeliveryHow::Sent,
            Err(error) if error.replay_safe => {
                self.take_host_prompt(&entry.parent, &text);
                DeliveryHow::Unsent
            }
            Err(_) => DeliveryHow::Unknown,
        };
        self.mark_notice(&entry.child, ended.turn, how).await;
    }
}

/// Where a finished thread's notice stands after one look.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NoticeStep {
    Done,
    Wait,
}

/// How long a team tools call waits for its new thread's bind.
pub(crate) const BIND_WAIT: Duration = Duration::from_secs(10);

/// The refusal codes a settled receipt may carry.
const KNOWN_CODES: [&str; 19] = [
    thread_team::NOT_A_THREAD,
    thread_team::NO_NESTING,
    thread_team::NO_PROJECT,
    thread_team::LIMIT_RUNNING,
    thread_team::LIMIT_TOTAL,
    thread_team::TEAM_FULL,
    thread_team::NOT_FOUND,
    thread_team::OTHER_PROJECT,
    thread_team::NOT_STARTED_BY_YOU,
    thread_team::THREAD_BUSY,
    thread_team::THREAD_IDLE,
    thread_team::MODE_UNAVAILABLE,
    thread_team::OUTCOME_UNKNOWN,
    thread_team::REQUEST_REUSED,
    thread_team::INVALID_INPUT,
    thread_team::UNAVAILABLE,
    thread_team::REFUSED,
    thread_team::VISUAL_TOO_LARGE,
    thread_team::VISUAL_NOT_HTML,
];

fn store_children(store: &ThreadTeamStore) -> HashSet<String> {
    store.started_children().map(str::to_owned).collect()
}

impl crate::dispatcher::TeamChildren for ThreadHost {
    /// A thread team tools started, or (protocol 67) a side question's side
    /// thread: either resumes one hop down, with no team tools.
    fn is_team_child(&self, session_key: &str) -> bool {
        Self::is_team_child(self, session_key) || self.is_side_thread(session_key)
    }
}
