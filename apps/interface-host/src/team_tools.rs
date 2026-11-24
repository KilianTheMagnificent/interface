//! The `iface_team` MCP server (team tools, T3 section 2, protocol 57): one
//! owner-only socket per Threads session, at a short path
//! (`/private/tmp/iface-tt-<id>/s.sock`) so it binds under any data root.
//! Which thread calls is the socket it connected to, bound to the session's
//! exact key once the provider has named it; the tool input never says who
//! calls. Who may speak for that thread is the kernel's peer: every call is
//! answered only when the connecting process is the provider process the
//! adapter named for this session (`provider_process_started`) or descends
//! from it, by one process listing read after the request arrived, with
//! both processes' start times compared so a pid a later process reused
//! does not pass (to `lstart`'s one-second resolution). Anything else (a
//! command another session runs, any other process of the owner's) is
//! refused and logged without its input. What each tool does is
//! `crate::threads::team`'s.

use std::{
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::engine::{
    CollaborationBindingError, CollaborationMcpSessionBinding, CollaborationMcpSessionRequest,
    CollaborationMcpStdioDescriptor, SessionKey, TEAM_TOOLS_SERVER, TeamToolsFactory, session_key,
};
use interface_contracts::ipc::parse_key;
use interface_terminal::ProcessListing;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{
    ThreadHost,
    mcp_transport::{self, BoundSocket, McpServerSpec, Peer, ToolBackend, ToolOutput, tool},
};

/// The adapters' factory before the thread host exists: until it is
/// attached, no session is offered team tools.
#[derive(Default)]
pub struct DeferredTeamTools {
    inner: Mutex<Option<Arc<dyn TeamToolsFactory>>>,
}

impl fmt::Debug for DeferredTeamTools {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeferredTeamTools")
    }
}

impl DeferredTeamTools {
    pub fn attach(&self, factory: Arc<dyn TeamToolsFactory>) {
        *lock(&self.inner) = Some(factory);
    }
}

#[async_trait]
impl TeamToolsFactory for DeferredTeamTools {
    async fn create(
        &self,
        request: CollaborationMcpSessionRequest,
    ) -> Result<Option<Arc<dyn CollaborationMcpSessionBinding>>, CollaborationBindingError> {
        let factory = lock(&self.inner).clone();
        match factory {
            Some(factory) => factory.create(request).await,
            None => Ok(None),
        }
    }
}

/// The host's team tools factory: a socket per session, served by the
/// thread host.
pub struct TeamToolServers {
    host: ThreadHost,
    base: PathBuf,
}

impl fmt::Debug for TeamToolServers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TeamToolServers")
    }
}

impl TeamToolServers {
    #[must_use]
    pub fn new(host: ThreadHost) -> Arc<Self> {
        Self::with_base(
            host,
            // Release smoke roots have passed private-directory validation and
            // hold the host lock; keep their sockets inside that sandbox.
            std::env::var_os("INTERFACE_HOST_SMOKE_DATA_ROOT").map_or_else(
                || PathBuf::from("/private/tmp"),
                |root| PathBuf::from(root).join("team-tools"),
            ),
        )
    }

    /// Sockets go in their own directories under `base` (tests).
    #[must_use]
    pub fn with_base(host: ThreadHost, base: PathBuf) -> Arc<Self> {
        Arc::new(Self { host, base })
    }
}

#[async_trait]
impl TeamToolsFactory for TeamToolServers {
    async fn create(
        &self,
        request: CollaborationMcpSessionRequest,
    ) -> Result<Option<Arc<dyn CollaborationMcpSessionBinding>>, CollaborationBindingError> {
        // A resumed session's key is known: a thread team tools started never
        // gets them, whichever way it is resumed.
        let resumed = request
            .resumed
            .then(|| session_key(&request.engine, &request.provisional_session_id));
        if !self.host.team_offered(resumed.as_deref()).await {
            return Ok(None);
        }
        let id = &uuid::Uuid::new_v4().simple().to_string()[..12];
        let directory = self.base.join(format!("iface-tt-{id}"));
        let path = directory.join("s.sock");
        if path.as_os_str().len() > mcp_transport::MAX_UNIX_SOCKET_PATH_BYTES {
            tracing::warn!("the team tools socket path is too long; sessions start without them");
            return Err(CollaborationBindingError::unavailable());
        }
        let BoundSocket { listener, identity } =
            mcp_transport::bind(&directory, &path).map_err(|error| {
                tracing::warn!(%error, "a team tools socket could not be bound");
                CollaborationBindingError::unavailable()
            })?;
        let state = Arc::new(BindingState {
            engine: request.engine.as_str().to_owned(),
            expected: resumed,
            key: Mutex::new(None),
            provider: watch::Sender::new(Provider::Pending),
        });
        let stop = CancellationToken::new();
        mcp_transport::serve(
            listener,
            &path,
            Arc::new(spec()),
            Arc::new(TeamBackend {
                host: self.host.clone(),
                state: Arc::clone(&state),
            }),
            stop.clone(),
        );
        Ok(Some(Arc::new(TeamSocketBinding {
            host: self.host.clone(),
            directory,
            path,
            identity,
            stop,
            state,
        })))
    }
}

/// Which session a socket serves: unknown until confirmed.
struct BindingState {
    engine: String,
    /// A resumed session's key, which the confirmation must name.
    expected: Option<String>,
    key: Mutex<Option<String>>,
    /// The process serving the session, once its adapter names it.
    provider: watch::Sender<Provider>,
}

/// How long a call waits for the adapter to name the provider process. It
/// names it as soon as it spawns it, before any prompt (Claude, whose CLI
/// is given the server at launch, may connect first); a call that still
/// finds none is refused.
const PROVIDER_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Provider {
    /// Not named yet.
    Pending,
    Known(ProviderProcess),
    /// Named but not found in the process listing, or named twice
    /// differently: nothing is answered.
    Refused,
}

/// A process by pid and start time, as one [`ProcessListing`] saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderProcess {
    pid: u32,
    start: String,
}

/// Whether the connecting process (`peer`, which started at `peer_start`
/// when it connected) may speak for the session `provider` serves: it is
/// that very process or descends from it, and neither pid has been reused
/// since. A command another session runs descends from that session's
/// provider, never from this one's, so it is refused.
fn serves(
    listing: &ProcessListing,
    provider: &ProviderProcess,
    peer: u32,
    peer_start: &str,
) -> bool {
    listing.start(provider.pid) == Some(provider.start.as_str())
        && listing.start(peer) == Some(peer_start)
        && listing
            .owned_ancestor(peer, |pid| pid == provider.pid)
            .is_some()
}

struct TeamSocketBinding {
    host: ThreadHost,
    directory: PathBuf,
    path: PathBuf,
    identity: (u64, u64),
    stop: CancellationToken,
    state: Arc<BindingState>,
}

#[async_trait]
impl CollaborationMcpSessionBinding for TeamSocketBinding {
    fn descriptor(&self) -> CollaborationMcpStdioDescriptor {
        CollaborationMcpStdioDescriptor {
            name: TEAM_TOOLS_SERVER.into(),
            command: mcp_transport::NC.into(),
            args: vec!["-U".into(), self.path.to_string_lossy().into_owned()],
            env: Vec::new(),
        }
    }

    async fn confirm_source_session_key(
        &self,
        key: SessionKey,
    ) -> Result<(), CollaborationBindingError> {
        let engine_matches =
            parse_key(&key).is_ok_and(|(engine, _)| engine.as_str() == self.state.engine);
        if !engine_matches
            || self
                .state
                .expected
                .as_ref()
                .is_some_and(|expected| *expected != key)
            || self.host.is_team_child(&key)
            || self.stop.is_cancelled()
        {
            return Err(CollaborationBindingError::invalid());
        }
        let mut bound = lock(&self.state.key);
        if bound.as_ref().is_some_and(|bound| *bound != key) {
            return Err(CollaborationBindingError::invalid());
        }
        self.host.team_bound(&key, true);
        *bound = Some(key);
        Ok(())
    }

    /// Pins the provider by pid and start time. The adapter names it while
    /// it still holds the unreaped child, so the pid cannot have been
    /// reused yet; a second, different name refuses everything after it.
    async fn provider_process_started(&self, process_id: u32) {
        let named = tokio::task::spawn_blocking(move || {
            let start = ProcessListing::read()?.start(process_id)?.to_owned();
            Some(ProviderProcess {
                pid: process_id,
                start,
            })
        })
        .await
        .ok()
        .flatten();
        self.state.provider.send_modify(|provider| {
            let next = match (&*provider, named) {
                (Provider::Pending, Some(named)) => Provider::Known(named),
                (Provider::Known(known), Some(named)) if *known == named => return,
                _ => Provider::Refused,
            };
            if next == Provider::Refused {
                tracing::warn!(
                    pid = process_id,
                    "a team tools socket's provider could not be pinned; it answers nothing"
                );
            }
            *provider = next;
        });
    }

    async fn dispose(&self) -> Result<(), CollaborationBindingError> {
        self.stop.cancel();
        if let Some(key) = lock(&self.state.key).take() {
            self.host.team_bound(&key, false);
        }
        mcp_transport::remove_bound_socket(&self.path, self.identity);
        let _ = std::fs::remove_dir(&self.directory);
        Ok(())
    }
}

impl Drop for TeamSocketBinding {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(key) = lock(&self.state.key).take() {
            self.host.team_bound(&key, false);
        }
        mcp_transport::remove_bound_socket(&self.path, self.identity);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

struct TeamBackend {
    host: ThreadHost,
    state: Arc<BindingState>,
}

impl TeamBackend {
    /// The provider process, once its adapter has named it (waiting up to
    /// [`PROVIDER_WAIT`]); `None` when it was not, or could not be pinned.
    async fn provider(&self) -> Option<ProviderProcess> {
        let mut named = self.state.provider.subscribe();
        let provider = tokio::time::timeout(
            PROVIDER_WAIT,
            named.wait_for(|provider| *provider != Provider::Pending),
        )
        .await
        .ok()?
        .ok()?
        .clone();
        match provider {
            Provider::Known(provider) => Some(provider),
            Provider::Pending | Provider::Refused => None,
        }
    }

    /// Whether this call's connecting process may speak for the session,
    /// by a process listing read now, after the request arrived.
    async fn speaks_for_this_session(&self, peer: &Peer) -> bool {
        let (Some(pid), Some(start)) = (peer.pid, peer.start.clone()) else {
            return false;
        };
        let Some(provider) = self.provider().await else {
            return false;
        };
        tokio::task::spawn_blocking(move || {
            ProcessListing::read().is_some_and(|listing| serves(&listing, &provider, pid, &start))
        })
        .await
        .unwrap_or(false)
    }
}

#[async_trait]
impl ToolBackend for TeamBackend {
    /// Reads when the connecting process started, so a later call can tell
    /// it from a process that reused its pid; one that is already gone, or
    /// cannot be read, is closed unanswered.
    async fn admit(&self, peer: Peer) -> Option<Peer> {
        let pid = peer.pid?;
        let start = tokio::task::spawn_blocking(move || {
            ProcessListing::read()?.start(pid).map(Arc::<str>::from)
        })
        .await
        .ok()
        .flatten();
        if start.is_none() {
            tracing::warn!(
                pid,
                "closed a team tools connection whose process could not be read"
            );
        }
        Some(Peer {
            pid: Some(pid),
            start: Some(start?),
        })
    }

    async fn call(&self, name: &str, input: Value, peer: Peer) -> Result<ToolOutput, String> {
        self.call_until_gone(name, input, peer, CancellationToken::new())
            .await
    }

    /// `request_secret` stops waiting for the owner once its caller is gone.
    async fn call_until_gone(
        &self,
        name: &str,
        input: Value,
        peer: Peer,
        gone: CancellationToken,
    ) -> Result<ToolOutput, String> {
        if !self.speaks_for_this_session(&peer).await {
            // The tool name is one of the catalogue's (the transport
            // refuses any other); the input is never logged.
            tracing::warn!(
                pid = peer.pid,
                tool = name,
                "refused a team tools call from a process outside the session it belongs to"
            );
            return Err(json!({
                "code": "refused",
                "message": "These team tools answer only the agent session they belong to.",
            })
            .to_string());
        }
        let key = lock(&self.state.key).clone().ok_or_else(|| {
            json!({
                "code": "unavailable",
                "message": "This session's team tools are not bound to it yet; try again in a moment.",
            })
            .to_string()
        })?;
        self.host
            .team_call_until_gone(&key, name, input, gone)
            .await
            .map(ToolOutput::Text)
    }
}

/// The longest `iface_team` request line: room for a 512 KiB visual
/// however its client escapes it.
pub(crate) const TEAM_REQUEST_LINE_BYTES: usize = 4 << 20;

pub(crate) fn spec() -> McpServerSpec {
    McpServerSpec {
        wire_name: "iface-team",
        title: "Interface Team Tools",
        instructions: "Interface team tools v1. Start helper threads (ordinary Interface threads the owner can open, read and take over) on any ready provider and model, wait for their results, read and message them, and show the owner a rendered page (show_visual). A thread you start never gets more permission than yours, never gets these tools itself, and works in this thread's project. Every result is JSON; a refusal is {code, message}. Give start_thread and send_to_thread a stable requestId; never retry an unknown outcome with a new one. schedule_prompt sends this thread a prompt later. request_secret asks the owner for a secret, which you get only as a private file's path.",
        tools: catalogue(),
        compatibility_tools: &[],
        // `show_visual` carries a page of up to 512 KiB; JSON escaping can
        // grow it several times over (T3 section 8).
        max_request_line_bytes: Some(TEAM_REQUEST_LINE_BYTES),
    }
}

/// The `tools/list` catalogue, in [`interface_contracts::engine::TEAM_TOOL_NAMES`] order.
pub(crate) fn catalogue() -> Vec<Value> {
    let string = json!({"type": "string"});
    let thread =
        json!({"type": "string", "description": "A thread key from start_thread or list_threads."});
    let request = json!({"type": "string", "description": "A stable id for this call; a retry with the same id answers the first call's result."});
    let mut tools = vec![
        tool(
            "team_info",
            "Who you are (this thread, its provider, model, permission mode and project), which providers and models can start a thread, and the limits.",
            &json!({}),
            &[],
            true,
        ),
        tool(
            "start_thread",
            "Start a new Interface thread in this project and send it `task` as its first prompt. Provider and model default to yours; it runs in your permission mode or a narrower one. `workspace` is `same` (your folder, the default) or `worktree` (a new worktree from your branch). With `notify` on (the default) you get one message when it finishes, unless you already read its result.",
            &json!({
                "task": string,
                "title": string,
                "provider": {"type": "string", "description": "claude, codex, or acp:<id> from team_info."},
                "model": string,
                "workspace": {"type": "string", "enum": ["same", "worktree"]},
                "notify": {"type": "boolean"},
                "requestId": request,
            }),
            &["task"],
            false,
        ),
        tool(
            "wait_thread",
            "Wait (up to timeoutSec, default 120, at most 600) for a thread to finish or need the owner. Answers its status, its last answer when it ended, and whether the wait timed out; a wait that times out stops nothing.",
            &json!({"thread": thread, "timeoutSec": {"type": "integer", "minimum": 0, "maximum": 600}}),
            &["thread"],
            true,
        ),
        tool(
            "thread_status",
            "A thread's status now (running, needsYou, done, failed or stopped), with its last answer when it ended.",
            &json!({"thread": thread}),
            &["thread"],
            true,
        ),
        tool(
            "stop_thread",
            "Stop the running turn of a thread you started.",
            &json!({"thread": thread}),
            &["thread"],
            false,
        ),
        tool(
            "list_threads",
            "Up to 50 threads in this project, newest activity first, with their status and whether you started them.",
            &json!({}),
            &[],
            true,
        ),
        tool(
            "read_thread",
            "A thread's record as compact items (prompts, answers, tool calls and results, diffs), about 30,000 characters a call; pass `next` back as `after` for more.",
            &json!({"thread": thread, "after": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 1, "maximum": 2000}}),
            &["thread"],
            true,
        ),
        tool(
            "send_to_thread",
            "Send a follow-up to a thread you started, only while it is idle (a running thread refuses; nothing is queued).",
            &json!({"thread": thread, "message": string, "requestId": request}),
            &["thread", "message"],
            false,
        ),
        tool(
            "show_visual",
            "Show the owner a rendered page (a chart, a mockup, a diagram) in a card above your written reply. `html` is ONE self-contained HTML document of at most 512 KiB: inline <style> and <script> are fine; images, fonts and media only as data: URLs; nothing loads from the network or from files (every request is blocked), and nothing is kept between views (no cookies or storage). Links do nothing when clicked. Read `prefers-color-scheme` so the page fits the owner's light or dark theme, and lay it out for a card about 720 px wide and 420 px tall (it scrolls inside; the owner can open it larger). Keep writing your answer as usual: the page is shown beside it, not instead of it. Answers {id, bytes}; a refusal says why (visual-too-large, visual-not-html).",
            &json!({
                "title": {"type": "string", "description": "A short title for the card, at most 120 characters."},
                "html": {"type": "string", "description": "The whole page: <!doctype html> and everything it needs inline."},
            }),
            &["title", "html"],
            false,
        ),
    ];
    tools.extend(schedule_tools(&string));
    tools.push(tool(
        "request_secret",
        "Ask the owner for a secret you need (an API key, a password, a token). The owner sees `name` and `reason` in a card above their composer and may give it or decline. Given, the value is written to a private file and you get only its `path`: read the value from that file when a command needs it, never print, echo, log or commit it, and never ask for it in chat instead. Declined (or not answered within 10 minutes), you get {declined: true}. `name` is 1 to 64 letters, digits, `_`, `.` or `-`; `reason` is one line. One request per thread at a time.",
        &json!({
            "name": {"type": "string", "description": "The secret's name, e.g. STRIPE_TEST_KEY."},
            "reason": {"type": "string", "description": "One line saying what it is for."},
        }),
        &["name", "reason"],
        false,
    ));
    for tool in &mut tools {
        tool["_meta"] = json!({"anthropic/alwaysLoad": true});
    }
    tools
}

/// Protocol 62 (T3 section 7): the calling thread's own scheduled prompts.
fn schedule_tools(string: &Value) -> [Value; 3] {
    [
        tool(
            "schedule_prompt",
            "Schedule a prompt to this thread (for example \"check again in an hour\"). `when` is {kind: \"once\", inMinutes} or {kind: \"once\", at: ISO 8601 time}, {kind: \"interval\", minutes} (at least 5) or {kind: \"calendar\", calendar: {timezone, weekdays: [1-7, Monday 1; empty = every day], hour, minute}}. It is sent when this thread is idle, runs only while Interface is open, and the owner sees it (set by the agent) and can cancel it.",
            &json!({
                "prompt": string,
                "when": {"type": "object"},
                "name": string,
            }),
            &["prompt", "when"],
            false,
        ),
        tool(
            "list_schedules",
            "This thread's scheduled prompts: id, name, when, next run and last result.",
            &json!({}),
            &[],
            true,
        ),
        tool(
            "cancel_schedule",
            "Delete one of this thread's scheduled prompts by its id.",
            &json!({"id": string}),
            &["id"],
            false,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::{ProcessListing, ProviderProcess, serves};

    /// Who may speak for a session, over a made-up process listing: the
    /// host (1) runs this session's provider (100, whose MCP server is 101
    /// and whose own tool shell ran 102) and another session's provider
    /// (200, whose tool shell ran 201 and 202). Only 100 and the processes
    /// under it are answered, and only while neither pid has been reused.
    #[test]
    fn a_team_socket_answers_only_its_own_provider_and_the_processes_under_it() {
        let listing: ProcessListing = [
            "1 0 Mon Nov 23 09:00:00 2026",
            "100 1 Mon Nov 23 10:00:00 2026",
            "101 100 Mon Nov 23 10:00:01 2026",
            "102 101 Mon Nov 23 10:00:02 2026",
            "200 1 Mon Nov 23 11:00:00 2026",
            "201 200 Mon Nov 23 11:00:01 2026",
            "202 201 Mon Nov 23 11:00:02 2026",
            "300 1 Mon Nov 23 12:00:00 2026",
        ]
        .into_iter()
        .collect();
        let ours = ProviderProcess {
            pid: 100,
            start: "Mon Nov 23 10:00:00 2026".into(),
        };
        let start = |pid| listing.start(pid).unwrap().to_owned();
        // The provider itself, its MCP child, and a process under that.
        assert!(serves(&listing, &ours, 100, &start(100)));
        assert!(serves(&listing, &ours, 101, &start(101)));
        assert!(serves(&listing, &ours, 102, &start(102)));
        // A command another session runs, that session's provider, and an
        // unrelated process of the owner's.
        assert!(!serves(&listing, &ours, 202, &start(202)));
        assert!(!serves(&listing, &ours, 200, &start(200)));
        assert!(!serves(&listing, &ours, 300, &start(300)));
        assert!(!serves(&listing, &ours, 1, &start(1)));
        // A caller whose pid now names a later process (its start moved),
        // and one that is gone.
        assert!(!serves(&listing, &ours, 101, "Mon Nov 23 09:59:59 2026"));
        assert!(!serves(&listing, &ours, 999, "Mon Nov 23 10:00:01 2026"));
        // A provider whose pid now names a later process, or that is gone.
        let reused = ProviderProcess {
            pid: 100,
            start: "Mon Nov 23 08:00:00 2026".into(),
        };
        assert!(!serves(&listing, &reused, 101, &start(101)));
        let gone = ProviderProcess {
            pid: 150,
            start: "Mon Nov 23 10:00:00 2026".into(),
        };
        assert!(!serves(&listing, &gone, 101, &start(101)));
        // The walk is bounded: a caller 40 processes down is not followed
        // all the way up.
        let deep: ProcessListing = std::iter::once("100 1 Mon Nov 23 10:00:00 2026".to_owned())
            .chain((101..=140).map(|pid| format!("{pid} {} Mon Nov 23 10:00:01 2026", pid - 1)))
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str)
            .collect();
        assert!(serves(&deep, &ours, 120, "Mon Nov 23 10:00:01 2026"));
        assert!(!serves(&deep, &ours, 140, "Mon Nov 23 10:00:01 2026"));
    }
}
