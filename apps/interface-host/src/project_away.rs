//! Desktop adapter for the shared Rust project loop and managed provider turns.
use crate::{
    dispatcher::{DispatchError, HostDispatcher, SessionEventObserver},
    native::decode_payload,
};
use async_trait::async_trait;
use interface_collaboration::project_away::{Answer, Executor, Failure, Service, Workspace};
use interface_common::sync::lock;
use interface_contracts::{
    away::{Command, Request, Role, Run, STATUS_VERSION, VERSION},
    engine::{EngineEvent, EngineEventKind, SessionStatus},
    ipc::CHANNELS,
};
use interface_engine::event_budget::{OMITTED_TRANSCRIPT, SUBSCRIBER_EVENT_BYTES};
use interface_engine::{EventSink, bounded_event_stream_with_budget};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

pub const AWAY_OPERATIONS: [&str; 1] = [CHANNELS.project_away];
type Watches = HashMap<String, EventSink<EngineEvent>>;
#[derive(Clone, Default)]
pub struct AwayEventTap {
    watched: Arc<Mutex<Watches>>,
    pub next: Option<Arc<dyn SessionEventObserver>>,
}
impl AwayEventTap {
    #[must_use]
    pub fn new(next: Arc<dyn SessionEventObserver>) -> Self {
        Self {
            next: Some(next),
            ..Self::default()
        }
    }
}
impl SessionEventObserver for AwayEventTap {
    fn observe_session_event(&self, key: &str, event: &EngineEvent) {
        if let Some(next) = &self.next {
            next.observe_session_event(key, event);
        }
        let mut watched = lock(&self.watched);
        if let Some(sender) = watched.get(key) {
            if sender.emit(event.clone()).is_err() {
                // A lost completion must fail closed, never look like success.
                watched.remove(key);
            }
        }
    }
}
struct NativeExecutor {
    sessions: Arc<dyn HostDispatcher>,
    events: AwayEventTap,
    /// Where a loop's thread lives; absent, a loop has nowhere to run.
    threads: Option<crate::ThreadHost>,
}
/// Stops watching a session's events when its turn ends, however it ends
/// (a Stop drops the turn's future).
struct Watching<'a> {
    events: &'a AwayEventTap,
    key: &'a str,
}
impl Drop for Watching<'_> {
    fn drop(&mut self) {
        lock(&self.events.watched).remove(self.key);
    }
}
impl NativeExecutor {
    fn threads(&self) -> Result<&crate::ThreadHost, Failure> {
        self.threads.as_ref().ok_or_else(|| {
            Failure::blocked("Threads are unavailable, so Away has no thread to run its loop in.")
        })
    }
}
#[async_trait]
impl Executor for NativeExecutor {
    async fn prepare(&self, run: &Run) -> Result<Workspace, Failure> {
        let hint = format!("away {}", run.options.objective);
        let prepared = self
            .threads()?
            .away_prepare(&run.cwd, &hint)
            .await
            .map_err(|error| {
                Failure::blocked(format!(
                    "Away works in its own git worktree of the project and could not make one: {}",
                    error.message
                ))
            })?;
        // The worktree as the thread names it (the prepared cwd may end in "/").
        Ok(Workspace {
            token: prepared.token,
            cwd: prepared.workspace.worktree_path.unwrap_or(prepared.cwd),
            branch: prepared.workspace.branch,
        })
    }
    async fn discard(&self, token: &str) {
        if let Some(threads) = &self.threads {
            threads.away_discard(token).await;
        }
    }
    async fn bind(&self, _: &Run, key: &str, token: &str) -> Result<(), Failure> {
        self.threads()?.away_bind(key, token).await.map_err(failure)
    }
    async fn release(&self, key: &str) {
        if let Some(threads) = &self.threads {
            threads.away_release(key);
        }
    }
    async fn create(
        &self,
        run: &Run,
        role: Role,
        iteration: u8,
        cwd: &str,
    ) -> Result<String, Failure> {
        let options =
            interface_collaboration::project_away::launch_options_in(run, role, iteration, cwd);
        let engine = interface_collaboration::project_away::session_provider(run, role);
        self.sessions.dispatch(CHANNELS.create_session, json!({"engine":engine,"options":options})).await
            .map_err(failure)?.get("key").and_then(Value::as_str).map(str::to_owned)
            .ok_or_else(|| Failure { message: "Provider launch returned no session identity. Inspect sessions before another run.".into(), unknown: true })
    }
    async fn turn(
        &self,
        key: &str,
        prompt: String,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<Answer, Failure> {
        let (sender, mut events) = bounded_event_stream_with_budget(4096, SUBSCRIBER_EVENT_BYTES);
        lock(&self.events.watched).insert(key.into(), sender);
        let _watching = Watching {
            events: &self.events,
            key,
        };
        // The loop's own thread takes only this prompt while the loop holds it.
        if let Some(threads) = &self.threads {
            threads.away_permit(key, &prompt);
        }
        // Register BEFORE send; a fast provider can complete before send settles.
        self.sessions
            .dispatch(CHANNELS.send, json!({"sessionKey":key,"text":prompt}))
            .await
            .map_err(failure)?;
        let thread_turn = self
            .threads
            .as_ref()
            .and_then(|threads| threads.away_sent_turn(key));
        let mut text = String::new();
        let mut observed_activity = false;
        loop {
            let event = tokio::select! {
                biased;
                _ = cancel.changed() => return Err(Failure::blocked("Away was stopped")),
                event = events.recv() => event.map_err(|error| Failure { message: error.to_string(), unknown: true })?.ok_or_else(|| Failure { message: "Away lost provider events; outcome unknown.".into(), unknown: true })?,
            };
            match event.event {
                // A just-created idle thread can announce its initial Idle
                // after send has locally announced Working. Only actual model
                // output/tool activity establishes this turn for completion.
                EngineEventKind::ToolCall { .. } | EngineEventKind::ToolResult { .. } => {
                    observed_activity = true;
                }
                EngineEventKind::MessageDelta {
                    text: delta,
                    agent_key: None,
                } => {
                    observed_activity = true;
                    text.push_str(&delta);
                    if text.len() > 65536 {
                        let mut start = text.len() - 32768;
                        while !text.is_char_boundary(start) {
                            start += 1;
                        }
                        text.drain(..start);
                    }
                }
                EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                } if observed_activity => return Ok(Answer { text, thread_turn }),
                EngineEventKind::StatusChanged {
                    status: SessionStatus::Failed,
                } => {
                    return Err(Failure {
                        message: "Agent entered a failed state; inspect its session.".into(),
                        unknown: true,
                    });
                }
                EngineEventKind::ApprovalRequested { summary, .. } => {
                    return Err(Failure::blocked(format!(
                        "Agent needs permission: {summary}. Inspect its session and start a new loop after resolving the requirement."
                    )));
                }
                EngineEventKind::Error { message, .. } => {
                    return Err(Failure {
                        message,
                        unknown: true,
                    });
                }
                EngineEventKind::Notice { ref message, .. } if message == OMITTED_TRANSCRIPT => {
                    return Err(Failure {
                        message: "Away lost part of the provider transcript; inspect its session before another run.".into(),
                        unknown: true,
                    });
                }
                EngineEventKind::SessionClosed => {
                    return Err(Failure {
                        message: "Agent session closed before confirmed completion.".into(),
                        unknown: true,
                    });
                }
                _ => {}
            }
        }
    }
    async fn close(&self, key: &str) -> Result<(), Failure> {
        lock(&self.events.watched).remove(key);
        // Closing the exact managed session terminates its provider process and
        // drains its lifecycle. History and project edits remain inspectable.
        self.sessions
            .dispatch(CHANNELS.close_session, json!({"sessionKey":key}))
            .await
            .map_err(failure)?;
        Ok(())
    }
}
/// Keeps each loop's thread showing it: after every Away commit, the loops'
/// projections go to the thread host, which pushes when one changed.
async fn project_threads(service: Arc<Service>, threads: crate::ThreadHost) {
    let mut revisions = service.subscribe();
    loop {
        match service.thread_views().await {
            Ok(views) => threads.set_away_threads(views).await,
            Err(error) => tracing::warn!(%error, "Away loops could not be shown in their threads"),
        }
        if revisions.changed().await.is_err() {
            return;
        }
    }
}
fn failure(error: DispatchError) -> Failure {
    Failure {
        message: error.message,
        unknown: !error.replay_safe,
    }
}

pub struct AwayHostDispatcher {
    service: Result<Arc<Service>, String>,
    executor: Arc<dyn Executor>,
    terminal: Arc<crate::terminal::TerminalHostDispatcher>,
    /// Hands each loop's projection to its thread after every commit.
    projections: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl AwayHostDispatcher {
    /// A loop runs as a thread of `threads` (review B3); without one, a
    /// start is refused before any session exists.
    #[must_use]
    pub fn new(
        root: &Path,
        sessions: Arc<dyn HostDispatcher>,
        events: AwayEventTap,
        terminal: Arc<crate::terminal::TerminalHostDispatcher>,
        threads: Option<crate::ThreadHost>,
    ) -> Arc<Self> {
        let service = interface_storage::PrivateDirectory::open(root)
            .and_then(|root| root.open_or_create_child("away"))
            .map_err(|e| e.to_string())
            .and_then(|root| Service::open(root.path()));
        let projections = match (&service, &threads) {
            (Ok(service), Some(threads)) => Some(tokio::spawn(project_threads(
                service.clone(),
                threads.clone(),
            ))),
            _ => None,
        };
        Arc::new(Self {
            service,
            executor: Arc::new(NativeExecutor {
                sessions,
                events,
                threads,
            }),
            terminal,
            projections: Mutex::new(projections),
        })
    }
    pub async fn shutdown(&self) {
        if let Ok(service) = &self.service {
            service.shutdown().await;
        }
        if let Some(projections) = lock(&self.projections).take() {
            projections.abort();
        }
    }
    /// The sessions the Threads surface leaves out, as Chat does: every
    /// review, and every session of a loop from before protocol 45. A loop's
    /// work session is its thread, so it stays. Empty when Away storage is
    /// unavailable.
    pub async fn session_keys(&self) -> Vec<String> {
        let Ok(service) = &self.service else {
            return Vec::new();
        };
        service.hidden_session_keys().await.unwrap_or_default()
    }
    fn cwd(&self, cwd: &str) -> Result<String, DispatchError> {
        let canonical =
            std::fs::canonicalize(cwd).map_err(|_| refused("The project folder is unavailable"))?;
        if !canonical.is_dir()
            || !self
                .terminal
                .away_project_roots()?
                .iter()
                .any(|root| std::fs::canonicalize(root).is_ok_and(|root| root == canonical))
        {
            return Err(refused(
                "Choose the project with Interface's folder picker or open it in a workspace first.",
            ));
        }
        Ok(canonical.to_string_lossy().into_owned())
    }
}
fn refused(message: impl Into<String>) -> DispatchError {
    DispatchError::new("away-refused", message, false, true)
}
#[async_trait]
impl HostDispatcher for AwayHostDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if operation != CHANNELS.project_away {
            return Err(refused("Unknown Away operation"));
        }
        let request: Request = decode_payload(operation, payload)?;
        if request.version
            != if matches!(request.command, Command::Status {}) {
                STATUS_VERSION
            } else {
                VERSION
            }
        {
            return Err(refused("Unsupported Away protocol"));
        }
        let service = self.service.as_ref().map_err(|e| refused(e.clone()))?;
        let result = match request.command {
            Command::Status {} => {
                return serde_json::to_value(service.status().await.map_err(refused)?)
                    .map_err(|error| refused(error.to_string()));
            }
            Command::Inspect {} => Ok(()),
            Command::Start {
                run_id,
                cwd,
                options,
            } => {
                service
                    .start(run_id, self.cwd(&cwd)?, options, self.executor.clone())
                    .await
            }
            Command::Stop { run_id } => service.stop(&run_id).await,
            Command::Save { cwd, options } => service.save(self.cwd(&cwd)?, options).await,
        };
        result.map_err(|e| DispatchError::new("away-request-failed", e, false, false))?;
        // Bounded UI projection; complete reports remain in the Rust store and
        // provider histories. Never confuse this with replay authority.
        serde_json::to_value(service.view(36, 4096).await.map_err(refused)?)
            .map_err(|e| refused(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FastSession {
        events: AwayEventTap,
    }
    #[async_trait]
    impl HostDispatcher for FastSession {
        async fn dispatch(&self, operation: &str, _: Value) -> Result<Value, DispatchError> {
            assert_eq!(operation, CHANNELS.send);
            for event in [
                EngineEventKind::StatusChanged {
                    status: SessionStatus::Working,
                },
                EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                },
                EngineEventKind::MessageDelta {
                    text: "AWAY_RESULT final evidence".into(),
                    agent_key: None,
                },
                EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                },
            ] {
                self.events
                    .observe_session_event("codex#exact", &EngineEvent { at: None, event });
            }
            Ok(Value::Null)
        }
    }
    #[tokio::test]
    async fn late_initial_idle_cannot_finish_a_new_turn_before_its_output() {
        let events = AwayEventTap::default();
        let executor = NativeExecutor {
            sessions: Arc::new(FastSession {
                events: events.clone(),
            }),
            events,
            threads: None,
        };
        let (_stop, cancel) = watch::channel(false);
        assert_eq!(
            executor
                .turn("codex#exact", "work".into(), cancel)
                .await
                .unwrap()
                .text,
            "AWAY_RESULT final evidence"
        );
    }
    /// Records what each session creation asked for.
    #[derive(Default)]
    struct Launches(Mutex<Vec<Value>>);
    #[async_trait]
    impl HostDispatcher for Launches {
        async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
            assert_eq!(operation, CHANNELS.create_session);
            let engine = payload["engine"].as_str().unwrap().to_owned();
            lock(&self.0).push(payload);
            Ok(json!({"key": format!("{engine}#fresh")}))
        }
    }
    /// Protocol 47: a review's session starts on the reviewer's provider,
    /// the work session on the worker's.
    #[tokio::test]
    async fn a_review_session_starts_on_the_reviewers_provider() {
        let launches = Arc::new(Launches::default());
        let executor = NativeExecutor {
            sessions: launches.clone(),
            events: AwayEventTap::default(),
            threads: None,
        };
        let run = Run {
            id: "reviewer".into(),
            cwd: "/project".into(),
            options: interface_contracts::away::Options {
                reviewer: Some("claude".into()),
                ..interface_contracts::away::Options::default()
            },
            started_at_ms: 1,
            deadline_at_ms: 3_600_001,
            state: interface_contracts::away::State::Running,
            detail: "fixture".into(),
            turns: vec![],
            thread: None,
        };
        let work = executor.create(&run, Role::Work, 1, "/wt").await.unwrap();
        let review = executor.create(&run, Role::Review, 1, "/wt").await.unwrap();
        assert_eq!(
            (work.as_str(), review.as_str()),
            ("codex#fresh", "claude#fresh")
        );
        let payloads = lock(&launches.0);
        assert_eq!(payloads[1]["options"]["permissionMode"], "acceptEdits");
        assert_eq!(payloads[1]["options"]["cwd"], "/wt");
        assert_eq!(payloads[0]["options"]["permissionMode"], "never");
    }
    struct OverflowSession {
        events: AwayEventTap,
    }
    #[async_trait]
    impl HostDispatcher for OverflowSession {
        async fn dispatch(&self, _: &str, _: Value) -> Result<Value, DispatchError> {
            self.events.observe_session_event(
                "codex#exact",
                &EngineEvent {
                    at: None,
                    event: EngineEventKind::MessageDelta {
                        text: "x".repeat(SUBSCRIBER_EVENT_BYTES + 1),
                        agent_key: None,
                    },
                },
            );
            Ok(Value::Null)
        }
    }
    #[tokio::test]
    async fn omitted_provider_output_stops_away_with_unknown_outcome() {
        let events = AwayEventTap::default();
        let executor = NativeExecutor {
            sessions: Arc::new(OverflowSession {
                events: events.clone(),
            }),
            events,
            threads: None,
        };
        let (_stop, cancel) = watch::channel(false);
        let error = executor
            .turn("codex#exact", "work".into(), cancel)
            .await
            .unwrap_err();
        assert!(error.unknown);
        assert!(error.message.contains("lost part"));
    }
}
