//! Phone Threads v1 (review D6): pure projections from what the Mac already
//! holds (the Threads view, the session registry, a provider history page and
//! a live session's events) into what the phone lists, reads and answers.
use std::{
    collections::{HashMap, HashSet},
    hash::BuildHasher,
};

use interface_contracts::{
    agents::{AgentControlResponse, AgentRun, AgentRunStatus},
    engine::{EngineEvent, EngineEventKind, SessionKind, SessionStatus, SessionSummary},
    ipc::HistoryPage,
    questions::parse_agent_questions,
    remote_threads::{
        RemoteInboxItemV2, RemoteInboxSource, RemoteInboxV2, RemoteThreadApprovalV1,
        RemoteThreadStatus, RemoteThreadTurnV1, RemoteThreadV1,
    },
    threads::{ThreadSection, ThreadView, ThreadsView, WorkspaceMode},
};

/// Shared owner inbox: only roster-bound Agent threads/runs, never arbitrary
/// provider sessions or hidden workers. History and approvals keep their guards.
#[must_use]
pub fn inbox<S: BuildHasher>(
    view: &ThreadsView,
    sessions: &HashMap<String, SessionSummary, S>,
    agents: Option<&AgentControlResponse>,
) -> RemoteInboxV2 {
    inbox_page(authorized_inbox(view, sessions, agents))
}

/// Bound a display page without using its truncation as session authority.
#[must_use]
pub fn inbox_page(mut inbox: RemoteInboxV2) -> RemoteInboxV2 {
    inbox.items.truncate(MAX_PHONE_THREADS);
    inbox
}

/// Full membership for detail/effects, including items displaced by a bounded
/// inbox page. Only the Threads view and persisted Agent bindings admit a key.
#[must_use]
pub fn authorized_inbox<S: BuildHasher>(
    view: &ThreadsView,
    sessions: &HashMap<String, SessionSummary, S>,
    agents: Option<&AgentControlResponse>,
) -> RemoteInboxV2 {
    // Provider history can rehydrate a session without its Agent kind. A known
    // Agent binding still owns that session; it cannot become a writable Thread.
    let reserved: HashSet<&str> = agents
        .into_iter()
        .flat_map(|agents| {
            agents
                .workspace
                .threads
                .values()
                .flat_map(|state| {
                    state
                        .current
                        .iter()
                        .map(|thread| thread.session_key.as_str())
                        .chain(state.past.iter().map(|thread| thread.session_key.as_str()))
                })
                .chain(
                    agents
                        .workspace
                        .runs
                        .iter()
                        .filter_map(|run| run.session_key.as_deref()),
                )
        })
        .collect();
    let mut seen = HashSet::new();
    let mut items: Vec<_> = view
        .threads
        .iter()
        .filter(|thread| !reserved.contains(thread.key.as_str()) && seen.insert(&thread.key))
        .filter_map(|thread| phone_thread(thread, sessions.get(&thread.key)?))
        .map(|thread| RemoteInboxItemV2 {
            id: thread.key.clone(),
            title: thread.title.clone(),
            status: thread.status,
            detail: String::new(),
            source: RemoteInboxSource::Thread,
            can_send: true,
            agent_name: None,
            thread: Some(thread),
        })
        .collect();
    if let Some(agents) = agents {
        append_agent_items(&mut items, sessions, agents);
    }
    items.sort_by_key(|item| match item.status {
        RemoteThreadStatus::NeedsYou => 0,
        RemoteThreadStatus::Working => 1,
        RemoteThreadStatus::Failed => 2,
        RemoteThreadStatus::Idle => 3,
    });
    RemoteInboxV2 {
        version: 2,
        items,
        unavailable: agents
            .is_none()
            .then(|| "Agent tasks could not be read from this Mac.".into()),
    }
}

fn append_agent_items<S: BuildHasher>(
    items: &mut Vec<RemoteInboxItemV2>,
    sessions: &HashMap<String, SessionSummary, S>,
    agents: &AgentControlResponse,
) {
    let task_keys: HashSet<&str> = agents
        .workspace
        .runs
        .iter()
        .filter_map(|run| run.session_key.as_deref())
        .collect();
    for agent in &agents.workspace.agents {
        if let Some(current) = agents
            .workspace
            .threads
            .get(&agent.id)
            .and_then(|state| state.current.as_ref())
            && !task_keys.contains(current.session_key.as_str())
            && let Some(session) = sessions
                .get(&current.session_key)
                .filter(|session| !session.archived)
        {
            if let Some(existing) = items.iter_mut().find(|item| {
                item.thread
                    .as_ref()
                    .is_some_and(|thread| thread.key == current.session_key)
            }) {
                // A malformed duplicate binding cannot widen send authority.
                existing.can_send = false;
                continue;
            }
            let mut thread = agent_session(session, &current.session_key);
            if agents
                .pending_approvals
                .iter()
                .any(|approval| approval.session_key == current.session_key)
            {
                thread.status = RemoteThreadStatus::NeedsYou;
            }
            items.push(RemoteInboxItemV2 {
                id: current.session_key.clone(),
                source: RemoteInboxSource::AgentThread,
                title: thread.title.clone(),
                status: thread.status,
                detail: if current.interrupted {
                    "Interrupted. Continue after reviewing the partial result.".into()
                } else {
                    String::new()
                },
                can_send: !current.interrupted,
                agent_name: Some(agent.name.clone()),
                thread: Some(thread),
            });
        }
    }
    for run in agents.workspace.runs.iter().rev() {
        if run.session_key.as_ref().is_some_and(|key| {
            items.iter().any(|item| {
                item.thread
                    .as_ref()
                    .is_some_and(|thread| &thread.key == key)
            })
        }) {
            continue;
        }
        items.push(agent_run(run, sessions));
    }
}

fn agent_run<S: BuildHasher>(
    run: &AgentRun,
    sessions: &HashMap<String, SessionSummary, S>,
) -> RemoteInboxItemV2 {
    let status = match run.status {
        AgentRunStatus::Waiting => RemoteThreadStatus::NeedsYou,
        AgentRunStatus::Queued | AgentRunStatus::Starting | AgentRunStatus::Running => {
            RemoteThreadStatus::Working
        }
        AgentRunStatus::Succeeded => RemoteThreadStatus::Idle,
        AgentRunStatus::Failed | AgentRunStatus::Unknown => RemoteThreadStatus::Failed,
    };
    let thread = run.session_key.as_ref().and_then(|key| {
        sessions
            .get(key)
            .filter(|session| !session.archived)
            .map(|session| {
                let mut thread = agent_session(session, key);
                thread.status = status;
                thread
            })
    });
    RemoteInboxItemV2 {
        id: format!("run:{}", run.id),
        source: RemoteInboxSource::AgentTask,
        title: run.name.clone(),
        status,
        detail: format!(
            "{}{}{}",
            match run.status {
                AgentRunStatus::Queued => "Queued",
                AgentRunStatus::Starting => "Starting",
                AgentRunStatus::Running => "Running",
                AgentRunStatus::Waiting => "Waiting for you",
                AgentRunStatus::Succeeded => "Completed",
                AgentRunStatus::Failed => "Failed",
                AgentRunStatus::Unknown => "Outcome unknown — inspect on Mac",
            },
            if run.detail.is_empty() { "" } else { ": " },
            clip_head(&run.detail, 600).0
        ),
        can_send: false,
        agent_name: Some(run.profile.name.clone()),
        thread,
    }
}

fn agent_session(session: &SessionSummary, key: &str) -> RemoteThreadV1 {
    RemoteThreadV1 {
        key: key.into(),
        engine: session.engine.to_string(),
        session_id: session.session_id.clone(),
        title: if session.title.is_empty() {
            "Agent task".into()
        } else {
            session.title.clone()
        },
        project: session.project_root.clone(),
        status: match session.status {
            SessionStatus::Waiting => RemoteThreadStatus::NeedsYou,
            SessionStatus::Working => RemoteThreadStatus::Working,
            SessionStatus::Idle => RemoteThreadStatus::Idle,
            SessionStatus::Failed => RemoteThreadStatus::Failed,
        },
        section: ThreadSection::Active,
        unread: false,
        last_activity: session.last_activity.clone(),
    }
}

/// The most threads one list carries. The Mac's order puts settled threads
/// last, so they are the ones left off.
pub const MAX_PHONE_THREADS: usize = 60;
/// The most recent turns a thread's page carries.
pub const MAX_PHONE_TURNS: usize = 12;
const MAX_PROMPT_CHARS: usize = 1_200;
const MAX_REPLY_CHARS: usize = 3_000;

/// The phone's Threads list: every thread the Mac's Threads lists (not From
/// terminals: those are Code panes' sessions, already in the session list;
/// nor a teammate's, nor an archived one) whose session the registry still
/// has, needs you first, then working, then the rest, each group in the
/// Mac's own order.
#[must_use]
pub fn phone_threads<S: BuildHasher>(
    view: &ThreadsView,
    sessions: &HashMap<String, SessionSummary, S>,
) -> Vec<RemoteThreadV1> {
    let rows: Vec<RemoteThreadV1> = view
        .threads
        .iter()
        .filter_map(|thread| phone_thread(thread, sessions.get(&thread.key)?))
        .collect();
    let rank = |status: RemoteThreadStatus| match status {
        RemoteThreadStatus::NeedsYou => 0,
        RemoteThreadStatus::Working => 1,
        RemoteThreadStatus::Idle | RemoteThreadStatus::Failed => 2,
    };
    let mut ordered: Vec<RemoteThreadV1> = Vec::with_capacity(rows.len());
    for group in 0..3 {
        ordered.extend(rows.iter().filter(|row| rank(row.status) == group).cloned());
    }
    ordered.truncate(MAX_PHONE_THREADS);
    ordered
}

/// One thread as the phone lists it, or `None` when the phone does not list it.
#[must_use]
pub fn phone_thread(thread: &ThreadView, session: &SessionSummary) -> Option<RemoteThreadV1> {
    if thread.section == ThreadSection::FromTerminals
        || session.archived
        || session.kind == Some(SessionKind::Agent)
    {
        return None;
    }
    let (engine, session_id) = thread.key.split_once('#')?;
    if engine != session.engine.as_str() || session_id != session.session_id {
        return None;
    }
    let project = match &thread.workspace {
        Some(workspace) if workspace.mode == WorkspaceMode::None => None,
        Some(workspace) => Some(workspace.project_root.clone()),
        None => session.project_root.clone(),
    }
    .filter(|root| !root.trim().is_empty());
    Some(RemoteThreadV1 {
        key: thread.key.clone(),
        engine: engine.to_owned(),
        session_id: session_id.to_owned(),
        title: if session.title.trim().is_empty() {
            "Untitled thread".to_owned()
        } else {
            session.title.clone()
        },
        project,
        status: match session.status {
            SessionStatus::Waiting => RemoteThreadStatus::NeedsYou,
            SessionStatus::Working => RemoteThreadStatus::Working,
            SessionStatus::Idle => RemoteThreadStatus::Idle,
            SessionStatus::Failed => RemoteThreadStatus::Failed,
        },
        section: thread.section,
        unread: thread.unread,
        last_activity: session.last_activity.clone(),
    })
}

/// A thread's recent turns from a provider history page: each user message
/// starts one; the main agent's text and tool calls up to the next belong to
/// it. Subagents' traffic is left out. Answers the turns kept (oldest first,
/// at most `MAX_PHONE_TURNS`) and how many earlier ones were left off.
#[must_use]
pub fn thread_turns(page: &HistoryPage) -> (Vec<RemoteThreadTurnV1>, u32) {
    struct Turn {
        prompt: String,
        reply: String,
        tools: u32,
        after_tool: bool,
    }
    let mut turns: Vec<Turn> = Vec::new();
    for event in &page.events {
        match &event.event {
            EngineEventKind::UserMessage {
                text,
                agent_key: None,
                ..
            } => turns.push(Turn {
                prompt: text.clone(),
                reply: String::new(),
                tools: 0,
                after_tool: false,
            }),
            EngineEventKind::MessageDelta {
                text,
                agent_key: None,
            } => {
                // A page that starts mid-turn has text before any prompt.
                if turns.is_empty() {
                    turns.push(Turn {
                        prompt: String::new(),
                        reply: String::new(),
                        tools: 0,
                        after_tool: false,
                    });
                }
                if let Some(turn) = turns.last_mut() {
                    // Text on both sides of a tool call is two messages.
                    if turn.after_tool && !turn.reply.is_empty() {
                        turn.reply.push_str("\n\n");
                    }
                    turn.after_tool = false;
                    turn.reply.push_str(text);
                }
            }
            EngineEventKind::ToolCall {
                agent_key: None, ..
            } => {
                if let Some(turn) = turns.last_mut() {
                    turn.tools = turn.tools.saturating_add(1);
                    turn.after_tool = true;
                }
            }
            _ => {}
        }
    }
    let older = turns.len().saturating_sub(MAX_PHONE_TURNS);
    let kept = turns
        .into_iter()
        .skip(older)
        .map(|turn| {
            let (prompt, cut_prompt) = clip_head(turn.prompt.trim(), MAX_PROMPT_CHARS);
            let (reply, cut_reply) = clip_tail(turn.reply.trim(), MAX_REPLY_CHARS);
            RemoteThreadTurnV1 {
                prompt,
                reply,
                tools: turn.tools,
                clipped: cut_prompt || cut_reply,
            }
        })
        .collect();
    (kept, u32::try_from(older).unwrap_or(u32::MAX))
}

/// The approval a live session is parked on: the newest `approval.requested`,
/// until its status moves on from waiting, a failure, or the session closes.
/// The renderer's `threadLiveFacts` reads its log the same way.
#[must_use]
pub fn pending_approval<'a>(
    events: impl IntoIterator<Item = &'a EngineEvent>,
) -> Option<RemoteThreadApprovalV1> {
    let mut ask = None;
    for event in events {
        match &event.event {
            EngineEventKind::ApprovalRequested {
                approval_id,
                tool,
                summary,
                input,
            } => {
                ask = Some(RemoteThreadApprovalV1 {
                    id: approval_id.clone(),
                    tool: tool.clone(),
                    summary: clip_head(summary.trim(), MAX_PROMPT_CHARS).0,
                    // A question needs its answers; only the Mac asks them.
                    answerable: parse_agent_questions(tool, input).is_none(),
                });
            }
            EngineEventKind::StatusChanged { status } if *status != SessionStatus::Waiting => {
                ask = None;
            }
            EngineEventKind::Error {
                retryable: false, ..
            }
            | EngineEventKind::SessionClosed => ask = None,
            _ => {}
        }
    }
    ask
}

/// The first `limit` characters, and whether any were cut.
fn clip_head(text: &str, limit: usize) -> (String, bool) {
    match text.char_indices().nth(limit) {
        Some((at, _)) => (format!("{}…", text[..at].trim_end()), true),
        None => (text.to_owned(), false),
    }
}

/// The last `limit` characters, and whether any were cut.
fn clip_tail(text: &str, limit: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= limit {
        return (text.to_owned(), false);
    }
    let at = text
        .char_indices()
        .nth(count - limit)
        .map_or(0, |(at, _)| at);
    (format!("…{}", text[at..].trim_start()), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::threads::{THREADS_VIEW_VERSION, ThreadSettings, ThreadWorkspace};
    use serde_json::json;

    #[test]
    fn inbox_includes_only_bound_agent_work_and_keeps_completed_tasks_read_only() {
        let mut sessions = HashMap::from([
            (
                "claude#thread".into(),
                session("claude#thread", SessionStatus::Idle),
            ),
            (
                "claude#agent".into(),
                session("claude#agent", SessionStatus::Waiting),
            ),
            (
                "claude#task".into(),
                session("claude#task", SessionStatus::Idle),
            ),
            (
                "claude#hidden".into(),
                session("claude#hidden", SessionStatus::Waiting),
            ),
        ]);
        for key in ["claude#agent", "claude#task", "claude#hidden"] {
            sessions.get_mut(key).unwrap().kind = Some(SessionKind::Agent);
        }
        let profile = json!({"id":"research", "name":"Researcher", "engine":"claude", "model":"default", "effort":"", "instructions":"private", "color":"moss"});
        let agents: AgentControlResponse = serde_json::from_value(json!({
            "version":2,"revision":1,"agents":[profile],"assignments":{},"routines":[],
            "runs":[{"id":"done", "routineId":"routine", "agentId":"research", "name":"Brief", "scheduledAtMs":1,"updatedAtMs":2,"status":"succeeded","sessionKey":"claude#task","detail":"Saved brief", "profile":profile,"prompt":"private", "cwd":"/Projects/app","began":true}],
            "threads":{"research":{"current":{"sessionKey":"claude#agent","startedAtMs":1,"status":"waiting"},"lastReadAtMs":0,"lastActivityAtMs":2}},
            "statuses":{}, "pendingApprovals":[]
        })).unwrap();
        let view = view(vec![thread("claude#thread", ThreadSection::Active)]);
        let result = inbox(&view, &sessions, Some(&agents));
        assert_eq!(result.items.len(), 3);
        assert_eq!(result.items[0].thread.as_ref().unwrap().key, "claude#agent");
        assert_eq!(result.items[0].status, RemoteThreadStatus::NeedsYou);
        let task = result
            .items
            .iter()
            .find(|item| item.id == "run:done")
            .unwrap();
        assert!(!task.can_send);
        assert!(task.detail.contains("Completed"));
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("claude#hidden"));
        assert!(inbox(&view, &sessions, None).unavailable.is_some());
    }

    fn inbox_agents() -> AgentControlResponse {
        serde_json::from_value(json!({
            "version": 2, "revision": 1,
            "agents": [{"id": "research", "name": "Researcher", "engine": "claude", "model": "default", "effort": "", "instructions": "", "color": "moss"}],
            "assignments": {}, "routines": [], "runs": [], "threads": {},
            "statuses": {}, "pendingApprovals": []
        })).unwrap()
    }

    #[test]
    fn inbox_display_limit_does_not_revoke_legacy_or_open_thread_membership() {
        let keys: Vec<_> = (0..MAX_PHONE_THREADS)
            .map(|index| format!("claude#thread-{index}"))
            .collect();
        let view = view(
            keys.iter()
                .map(|key| thread(key, ThreadSection::Active))
                .collect(),
        );
        let mut sessions: HashMap<_, _> = keys
            .iter()
            .map(|key| (key.clone(), session(key, SessionStatus::Idle)))
            .collect();
        sessions.insert(
            "claude#agent".into(),
            session("claude#agent", SessionStatus::Working),
        );
        sessions.insert(
            "claude#hidden".into(),
            session("claude#hidden", SessionStatus::Waiting),
        );
        let mut agents = inbox_agents();
        agents.workspace.threads = serde_json::from_value(json!({"research": {
            "current": {"sessionKey": "claude#agent", "startedAtMs": 1, "status": "working"}
        }}))
        .unwrap();
        let last = keys.last().unwrap();
        assert!(
            phone_threads(&view, &sessions)
                .iter()
                .any(|thread| &thread.key == last)
        );
        let page = inbox(&view, &sessions, Some(&agents));
        assert_eq!(page.items.len(), MAX_PHONE_THREADS);
        assert!(!page.items.iter().any(|item| {
            item.thread
                .as_ref()
                .is_some_and(|thread| &thread.key == last)
        }));
        let authorized = authorized_inbox(&view, &sessions, Some(&agents));
        assert_eq!(authorized.items.len(), MAX_PHONE_THREADS + 1);
        assert!(authorized.items.iter().any(|item| {
            item.can_send
                && item
                    .thread
                    .as_ref()
                    .is_some_and(|thread| &thread.key == last)
        }));
        assert!(!authorized.items.iter().any(|item| {
            item.thread
                .as_ref()
                .is_some_and(|thread| thread.key == "claude#hidden")
        }));
    }

    #[test]
    fn inbox_task_binding_overrides_rehydrated_thread_kind_and_duplicate_current_binding() {
        let key = "claude#task";
        let sessions = HashMap::from([(key.into(), session(key, SessionStatus::Idle))]);
        let view = view(vec![thread(key, ThreadSection::Active)]);
        let mut agents = inbox_agents();
        agents.workspace.threads = serde_json::from_value(json!({"research": {
            "current": {"sessionKey": key, "startedAtMs": 1, "status": "idle"}
        }}))
        .unwrap();
        agents.workspace.runs = serde_json::from_value(json!([{
            "id": "done", "routineId": "routine", "agentId": "research", "name": "Brief",
            "scheduledAtMs": 1, "updatedAtMs": 2, "status": "succeeded", "sessionKey": key,
            "detail": "Saved brief", "profile": agents.workspace.agents[0], "prompt": "private", "cwd": "/Projects/app", "began": true
        }])).unwrap();
        let duplicate = agents.workspace.runs[0].clone();
        agents
            .workspace
            .runs
            .push(interface_contracts::agents::AgentRun {
                id: "newest".into(),
                ..duplicate
            });
        let result = authorized_inbox(&view, &sessions, Some(&agents));
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].id, "run:newest");
        assert_eq!(result.items[0].source, RemoteInboxSource::AgentTask);
        assert!(!result.items[0].can_send);
    }

    #[test]
    fn inbox_duplicate_agent_threads_fail_closed_and_past_bindings_stay_hidden() {
        let mut agents = inbox_agents();
        let second = interface_contracts::agents::AgentProfile {
            id: "second".into(),
            ..agents.workspace.agents[0].clone()
        };
        agents.workspace.agents.push(second);
        agents.workspace.threads = serde_json::from_value(json!({
            "research": {"current": {"sessionKey": "claude#shared", "startedAtMs": 1, "status": "idle"},
                "past": [{"sessionKey": "claude#past", "startedAtMs": 1, "endedAtMs": 2}]},
            "second": {"current": {"sessionKey": "claude#shared", "startedAtMs": 1, "status": "idle"}}
        })).unwrap();
        let sessions = HashMap::from([
            (
                "claude#shared".into(),
                session("claude#shared", SessionStatus::Idle),
            ),
            (
                "claude#past".into(),
                session("claude#past", SessionStatus::Idle),
            ),
        ]);
        let view = view(vec![
            thread("claude#shared", ThreadSection::Active),
            thread("claude#past", ThreadSection::Active),
        ]);
        let result = authorized_inbox(&view, &sessions, Some(&agents));
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].id, "claude#shared");
        assert!(!result.items[0].can_send);
    }

    fn session(key: &str, status: SessionStatus) -> SessionSummary {
        let (engine, id) = key.split_once('#').unwrap();
        serde_json::from_value(json!({
            "engine": engine, "sessionId": id, "title": format!("Title of {id}"),
            "cwd": "/Projects/app", "status": status, "cloud": false,
            "lastActivity": "2026-10-14T09:00:00.000Z", "firstSeen": "2026-10-14T08:00:00.000Z",
            "archived": false, "projectRoot": "/Projects/app"
        }))
        .unwrap()
    }

    fn thread(key: &str, section: ThreadSection) -> ThreadView {
        serde_json::from_value(json!({
            "key": key, "section": section, "createdAt": 1, "woke": false, "unread": false,
            "autoSettle": true, "turns": []
        }))
        .unwrap()
    }

    fn view(threads: Vec<ThreadView>) -> ThreadsView {
        ThreadsView {
            version: THREADS_VIEW_VERSION,
            revision: 1,
            now: 2,
            settings: ThreadSettings::default(),
            threads,
            hidden_keys: Vec::new(),
        }
    }

    #[test]
    fn the_phone_lists_threads_that_need_you_then_working_then_the_macs_order() {
        let rows = [
            (
                "claude#pinned-idle",
                ThreadSection::Pinned,
                SessionStatus::Idle,
            ),
            (
                "codex#active-working",
                ThreadSection::Active,
                SessionStatus::Working,
            ),
            (
                "claude#active-idle",
                ThreadSection::Active,
                SessionStatus::Idle,
            ),
            (
                "claude#active-waiting",
                ThreadSection::Active,
                SessionStatus::Waiting,
            ),
            (
                "claude#snoozed-failed",
                ThreadSection::Snoozed,
                SessionStatus::Failed,
            ),
            (
                "claude#settled-waiting",
                ThreadSection::Settled,
                SessionStatus::Waiting,
            ),
            (
                "claude#from-terminal",
                ThreadSection::FromTerminals,
                SessionStatus::Waiting,
            ),
            (
                "claude#forgotten",
                ThreadSection::Active,
                SessionStatus::Waiting,
            ),
            (
                "claude#teammate",
                ThreadSection::Active,
                SessionStatus::Waiting,
            ),
            (
                "claude#archived",
                ThreadSection::Active,
                SessionStatus::Working,
            ),
        ];
        let mut sessions: HashMap<String, SessionSummary> = rows
            .iter()
            .filter(|(key, ..)| *key != "claude#forgotten")
            .map(|(key, _, status)| ((*key).to_owned(), session(key, *status)))
            .collect();
        sessions.get_mut("claude#teammate").unwrap().kind = Some(SessionKind::Agent);
        sessions.get_mut("claude#archived").unwrap().archived = true;
        let view = view(
            rows.iter()
                .map(|(key, section, _)| thread(key, *section))
                .collect(),
        );
        let listed = phone_threads(&view, &sessions);
        let keys: Vec<&str> = listed.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "claude#active-waiting",
                "claude#settled-waiting",
                "codex#active-working",
                "claude#pinned-idle",
                "claude#active-idle",
                "claude#snoozed-failed",
            ]
        );
        assert_eq!(listed[0].status, RemoteThreadStatus::NeedsYou);
        assert_eq!(listed[0].engine, "claude");
        assert_eq!(listed[0].session_id, "active-waiting");
        assert_eq!(listed[0].project.as_deref(), Some("/Projects/app"));
        assert_eq!(listed[5].status, RemoteThreadStatus::Failed);
    }

    #[test]
    fn a_plain_chat_has_no_project_and_a_thread_workspace_names_its_own() {
        let key = "claude#chat";
        let sessions = HashMap::from([(key.to_owned(), session(key, SessionStatus::Idle))]);
        let mut chat = thread(key, ThreadSection::Active);
        chat.workspace = Some(ThreadWorkspace {
            mode: WorkspaceMode::None,
            project_root: String::new(),
            worktree_path: None,
            branch: None,
            base_ref: None,
            owned: false,
            missing: None,
            kept: None,
        });
        assert_eq!(phone_thread(&chat, &sessions[key]).unwrap().project, None);
        let mut worktree = chat.clone();
        worktree.workspace.as_mut().unwrap().mode = WorkspaceMode::Worktree;
        worktree.workspace.as_mut().unwrap().project_root = "/Projects/other".into();
        assert_eq!(
            phone_thread(&worktree, &sessions[key])
                .unwrap()
                .project
                .as_deref(),
            Some("/Projects/other")
        );
        let many: HashMap<String, SessionSummary> = (0..80)
            .map(|index| {
                let key = format!("claude#s{index}");
                let session = session(&key, SessionStatus::Idle);
                (key, session)
            })
            .collect();
        let view = view(
            (0..80)
                .map(|index| thread(&format!("claude#s{index}"), ThreadSection::Active))
                .collect(),
        );
        assert_eq!(phone_threads(&view, &many).len(), MAX_PHONE_THREADS);
    }

    fn event(kind: serde_json::Value) -> EngineEvent {
        serde_json::from_value(kind).unwrap()
    }

    #[test]
    fn recent_turns_keep_the_main_agents_words_and_count_its_tools() {
        let mut events = vec![event(
            json!({"kind": "message.delta", "text": "resumed mid-turn"}),
        )];
        for index in 0..14 {
            events.push(event(
                json!({"kind": "user.message", "text": format!("prompt {index}")}),
            ));
            events.push(event(json!({"kind": "message.delta", "text": "Reading "})));
            events.push(event(json!({"kind": "message.delta", "text": "the file."})));
            events.push(event(json!({"kind": "tool.call", "callId": "c", "tool": "Read", "summary": "Read a", "input": {}})));
            events.push(event(json!({"kind": "tool.call", "callId": "s", "tool": "Read", "summary": "Read b", "input": {}, "agentKey": "sub"})));
            events.push(event(
                json!({"kind": "message.delta", "text": "subagent words", "agentKey": "sub"}),
            ));
            events.push(event(json!({"kind": "message.delta", "text": "Done."})));
        }
        events.push(event(
            json!({"kind": "user.message", "text": "x".repeat(MAX_PROMPT_CHARS + 5)}),
        ));
        events.push(event(json!({"kind": "message.delta", "text": format!("start {}", "y".repeat(MAX_REPLY_CHARS))})));
        let page = HistoryPage {
            events,
            ..HistoryPage::empty()
        };
        let (turns, older) = thread_turns(&page);
        assert_eq!(turns.len(), MAX_PHONE_TURNS);
        // 1 resumed + 14 prompts + 1 long one, the newest 12 kept.
        assert_eq!(older, 4);
        let ordinary = &turns[0];
        assert_eq!(ordinary.prompt, "prompt 3");
        assert_eq!(ordinary.reply, "Reading the file.\n\nDone.");
        assert_eq!(ordinary.tools, 1);
        assert!(!ordinary.clipped);
        let long = turns.last().unwrap();
        assert!(long.clipped);
        assert_eq!(long.prompt.chars().count(), MAX_PROMPT_CHARS + 1);
        assert!(long.prompt.ends_with('…'));
        assert!(long.reply.starts_with('…') && !long.reply.contains("start"));
    }

    #[test]
    fn the_pending_approval_is_the_newest_until_the_session_moves_on() {
        let asked = |id: &str, tool: &str, input: serde_json::Value| {
            event(
                json!({"kind": "approval.requested", "approvalId": id, "tool": tool,
                "summary": format!("{tool} request"), "input": input}),
            )
        };
        let waiting = event(json!({"kind": "status.changed", "status": "waiting"}));
        let working = event(json!({"kind": "status.changed", "status": "working"}));
        let first = asked("a1", "Bash", json!({"command": "ls"}));
        let second = asked("a2", "Edit", json!({"path": "x"}));
        assert_eq!(
            pending_approval([&first, &waiting, &second]).unwrap().id,
            "a2"
        );
        let answered = pending_approval([&first, &waiting, &working]);
        assert!(answered.is_none());
        let failed = event(json!({"kind": "error", "message": "boom", "retryable": false}));
        assert!(pending_approval([&first, &failed]).is_none());
        let retry = event(json!({"kind": "error", "message": "rate", "retryable": true}));
        assert!(pending_approval([&first, &retry]).unwrap().answerable);
        let question = asked(
            "q1",
            "AskUserQuestion",
            json!({"questions": [{"question": "Which one?", "header": "Pick", "options": [], "multiSelect": false}]}),
        );
        let parked = pending_approval([&question]).unwrap();
        assert!(!parked.answerable, "a question is answered on the Mac");
        let closed = event(json!({"kind": "session.closed"}));
        assert!(pending_approval([&second, &closed]).is_none());
    }
}
