//! Outside agents (T3 section 11, protocol 65): what an `iface mcp` session's
//! calls do. The caller is the connection the host accepted on its outside
//! socket (`crate::outside_agents`), named by the host; its client's declared
//! name is display text only. The level is the owner's setting, read at every
//! call, so turning it down takes effect at once. Every rule is
//! `interface_settings::thread_team`'s; reading, waiting and control reuse the
//! team tools' code with an outside session's reach (any thread with a
//! project), and a start reuses theirs with an outside session's choices: a
//! project an Interface thread already works in, the target's asking mode,
//! the provider's default account, no lineage, no notices.

use std::collections::{BTreeMap, HashMap};

use interface_common::sync::lock;
use interface_contracts::{
    engine::{SessionKind, session_key},
    threads::{OutsideAgents, ThreadsView, WorkspaceMode},
};
use interface_settings::thread_team::{
    self, Rung, StartedBy, StartedByKind, TeamRefusal, ThreadTeamStore,
};
use serde_json::{Value, json};

use super::{
    ThreadHost,
    team::{
        Reach, StartPlan, Who, field_ok, refusal, start_title, string_field, unavailable,
        validate_fields,
    },
};
use crate::dispatcher::DispatchError;

/// The ledger's outside agents part, as views show it.
#[derive(Clone, Debug, Default)]
pub(crate) struct OutsideSnapshot {
    level: OutsideAgents,
    /// Thread → its client's name, for each thread an outside agent started.
    clients: HashMap<String, String>,
}

impl OutsideSnapshot {
    pub(super) fn of(store: &ThreadTeamStore) -> Self {
        Self {
            level: store.outside_agents(),
            clients: store
                .outside_started()
                .map(|(child, client)| (child.to_owned(), client.to_owned()))
                .collect(),
        }
    }
}

/// Answers an outside call's closed input contract.
fn validate_outside_input(name: &str, input: &Value) -> Result<(), TeamRefusal> {
    let (required, optional): (&[&str], &[&str]) = match name {
        "list_projects" => (&[], &[]),
        "list_threads" => (&[], &["project"]),
        "read_thread" => (&["thread"], &["after", "limit"]),
        "thread_status" | "stop_thread" => (&["thread"], &[]),
        "wait_thread" => (&["thread"], &["timeoutSec"]),
        "start_thread" => (
            &["project", "task"],
            &["title", "provider", "model", "workspace", "requestId"],
        ),
        "send_to_thread" => (&["thread", "message"], &["requestId"]),
        _ => return Err(refusal(thread_team::INVALID_INPUT, "Unknown tool.")),
    };
    validate_fields(name, input, required, optional, |key, value| match key {
        "project" => value.as_str().is_some_and(|project| {
            !project.trim().is_empty()
                && project.len() <= 4_096
                && !project.chars().any(char::is_control)
        }),
        "workspace" => matches!(value.as_str(), Some("local" | "worktree")),
        _ => field_ok(key, value),
    })
}

impl ThreadHost {
    /// What outside agents may do now; off until team tools are attached.
    #[must_use]
    pub fn outside_level(&self) -> OutsideAgents {
        self.team()
            .map_or(OutsideAgents::Off, |team| lock(&team.outside).level)
    }

    /// Records the owner's outside agents level in the ledger. Answers
    /// whether it changed.
    pub(super) async fn set_outside_agents(
        &self,
        level: OutsideAgents,
    ) -> Result<bool, DispatchError> {
        if self.outside_level() == level {
            return Ok(false);
        }
        let saved = self
            .team_store(move |store| store.set_outside_agents(level))
            .await
            .map_err(|refused| {
                DispatchError::new(OUTSIDE_SETTING_FAILED_CODE, refused.message, false, true)
            })?;
        saved.map_err(|error| {
            // Only a refusal before the write proves nothing changed.
            let unsent = matches!(
                error,
                interface_settings::SettingsError::InvalidAgent(_)
                    | interface_settings::SettingsError::Storage(
                        interface_storage::StorageError::WriteFailed { .. }
                            | interface_storage::StorageError::BoundsExceeded { .. }
                    )
            );
            DispatchError::new(
                OUTSIDE_SETTING_FAILED_CODE,
                format!("The outside agents setting could not be saved: {error}."),
                false,
                unsent,
            )
        })?;
        Ok(true)
    }

    /// Puts the settings kept outside the Threads store on `view`: the
    /// outside agents level and who started which thread (protocol 65), and
    /// pane tools without asking (protocol 71).
    pub(super) fn with_outside(&self, view: &mut ThreadsView) {
        view.settings.pane_tools_without_asking = self
            .inner
            .pane_tools
            .load(std::sync::atomic::Ordering::SeqCst);
        let Ok(team) = self.team() else {
            return;
        };
        let snapshot = lock(&team.outside);
        view.settings.outside_agents = snapshot.level;
        for thread in &mut view.threads {
            thread.started_from_outside = snapshot.clients.get(&thread.key).cloned();
        }
    }

    /// Runs one outside call for session `session` (the host's id for the
    /// connection) whose client declared `client`. Answers its JSON text, or
    /// the refusal's.
    pub(crate) async fn outside_call(
        &self,
        session: &str,
        client: &str,
        name: &str,
        input: Value,
    ) -> Result<String, String> {
        self.outside_call_inner(session, client, name, &input)
            .await
            .map(|value| value.to_string())
            .map_err(|refused| refused.to_json().to_string())
    }

    async fn outside_call_inner(
        &self,
        session: &str,
        client: &str,
        name: &str,
        input: &Value,
    ) -> Result<Value, TeamRefusal> {
        self.team()?;
        thread_team::outside_may_call(self.outside_level(), name)?;
        validate_outside_input(name, input)?;
        let key = thread_team::outside_caller(session);
        let who = Who {
            key: &key,
            reach: Reach::AnyProject,
        };
        // A switched thread is one thread (T3 section 5).
        let thread = match string_field(input, "thread") {
            Some(thread) => self.head_key(thread).await,
            None => String::new(),
        };
        let thread = thread.as_str();
        match name {
            "list_projects" => self.list_projects().await,
            "list_threads" => {
                // A root or a name from list_projects, as a start takes it.
                let project = match string_field(input, "project") {
                    Some(wanted) => {
                        let known = self.projects().await?.into_keys().collect::<Vec<_>>();
                        let root = thread_team::resolve_project(&known, wanted).map_err(
                            |mut refused| {
                                // A list starts nothing.
                                refused.message =
                                    refused.message.replace(" Nothing was started.", "");
                                refused
                            },
                        )?;
                        Some(root)
                    }
                    None => None,
                };
                self.list_threads(who, project.as_deref()).await
            }
            "read_thread" => {
                self.read_thread(
                    who,
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
            "thread_status" => self.wait_thread(who, thread, 0).await,
            "wait_thread" => {
                let secs = input
                    .get("timeoutSec")
                    .and_then(Value::as_u64)
                    .unwrap_or(thread_team::WAIT_DEFAULT_SECS);
                self.wait_thread(who, thread, secs).await
            }
            // Receipts hold across sessions (`OUTSIDE_RECEIPTS_CALLER`): a
            // retry after a lost connection answers what the first call did.
            "start_thread" => {
                let request = string_field(input, "requestId").map(str::to_owned);
                let mut started = self
                    .with_receipt(
                        thread_team::OUTSIDE_RECEIPTS_CALLER,
                        request.as_deref(),
                        name,
                        input,
                        || self.outside_start(&key, client, input),
                    )
                    .await?;
                // A replay to another session: the thread is not its to
                // message or stop.
                if let Some(child) = started.get("thread").and_then(Value::as_str) {
                    let child = child.to_owned();
                    let parent = self
                        .team_store(move |store| {
                            store.started(&child).map(|entry| entry.parent.clone())
                        })
                        .await?;
                    if parent.as_deref() != Some(key.as_str()) {
                        started["startedByYou"] = false.into();
                    }
                }
                Ok(started)
            }
            "send_to_thread" => {
                let request = string_field(input, "requestId").map(str::to_owned);
                self.with_receipt(
                    thread_team::OUTSIDE_RECEIPTS_CALLER,
                    request.as_deref(),
                    name,
                    input,
                    || {
                        self.send_to_thread(
                            who,
                            thread,
                            string_field(input, "message").unwrap_or_default(),
                        )
                    },
                )
                .await
            }
            "stop_thread" => self.stop_thread(who, thread).await,
            _ => Err(refusal(thread_team::INVALID_INPUT, "Unknown tool.")),
        }
    }

    /// Every Interface project with how many threads work in it: the project
    /// roots of listed threads (not hidden workers, not agents).
    async fn projects(&self) -> Result<BTreeMap<String, usize>, TeamRefusal> {
        let ((), view) = self
            .read(|_, _| ())
            .await
            .map_err(|error| unavailable(error.message))?;
        let kinds = self
            .inner
            .ports
            .sessions
            .sessions()
            .map_err(unavailable)?
            .into_iter()
            .map(|summary| {
                (
                    session_key(&summary.engine, &summary.session_id),
                    summary.kind,
                )
            })
            .collect::<HashMap<_, _>>();
        let hidden = self.inner.ports.hidden.hidden_keys().await;
        let mut projects = BTreeMap::new();
        for thread in &view.threads {
            let listed = kinds
                .get(&thread.key)
                .is_some_and(|kind| *kind != Some(SessionKind::Agent))
                && !hidden.contains(&thread.key);
            let root = thread
                .workspace
                .as_ref()
                .filter(|workspace| workspace.mode != WorkspaceMode::None)
                .map(|workspace| workspace.project_root.clone())
                .filter(|root| !root.is_empty());
            if let (true, Some(root)) = (listed, root) {
                *projects.entry(root).or_insert(0) += 1;
            }
        }
        Ok(projects)
    }

    async fn list_projects(&self) -> Result<Value, TeamRefusal> {
        let mut projects = self
            .projects()
            .await?
            .into_iter()
            .map(|(root, threads)| {
                json!({
                    "name": thread_team::project_name(&root),
                    "project": root,
                    "threads": threads,
                })
            })
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| {
            left["name"]
                .as_str()
                .cmp(&right["name"].as_str())
                .then_with(|| left["project"].as_str().cmp(&right["project"].as_str()))
        });
        Ok(json!({"projects": projects}))
    }

    /// Starts a thread for outside session `key` in a project an Interface
    /// thread already works in, at the target's asking rung.
    async fn outside_start(
        &self,
        key: &str,
        client: &str,
        input: &Value,
    ) -> Result<Value, TeamRefusal> {
        let team = self.team()?;
        let _one_at_a_time = team.outside_starts.lock().await;
        let known = self.projects().await?.into_keys().collect::<Vec<_>>();
        let project = thread_team::resolve_project(
            &known,
            string_field(input, "project").unwrap_or_default(),
        )?;
        let task = string_field(input, "task").unwrap_or_default().to_owned();
        let provider = string_field(input, "provider")
            .unwrap_or("claude")
            .to_owned();
        let model = string_field(input, "model").map(str::to_owned);
        let (title, title_source) = start_title(input, &task);
        // Never more than asking: the started thread asks the owner before
        // it acts, whatever the outside agent can do on its own.
        let (_, mode) = thread_team::child_mode(&provider, Rung::Ask).ok_or_else(|| {
            refusal(
                thread_team::MODE_UNAVAILABLE,
                format!("{provider} has no mode that asks before acting, so no thread was started. Start it on claude or codex."),
            )
        })?;
        let (parent, since) = (
            key.to_owned(),
            crate::threads::now_ms().saturating_sub(thread_team::DAY_MS),
        );
        let (children, day, ledger) = self
            .team_store(move |store| {
                (
                    store
                        .children(&parent)
                        .into_iter()
                        .map(|entry| entry.child.clone())
                        .collect::<Vec<_>>(),
                    store.outside_started_since(since),
                    store.len(),
                )
            })
            .await?;
        let running = self.running_among(&children).await?;
        thread_team::check_outside_start_limits(running, children.len(), day, ledger)?;
        let prepare = if string_field(input, "workspace") == Some("worktree") {
            "worktree"
        } else {
            "local"
        };
        self.start_child(StartPlan {
            parent: key.to_owned(),
            started_by: Some(StartedBy {
                kind: StartedByKind::Outside,
                client: thread_team::client_name(Some(client)),
            }),
            project,
            prepare,
            base: None,
            existing: None,
            lineage: None,
            task,
            provider,
            model,
            title,
            title_source,
            // Nobody inside Interface waits for it: the outside agent asks.
            notify: false,
            mode,
            // The target provider's default account.
            account: None,
        })
        .await
    }
}

/// The outside agents setting could not be saved.
pub const OUTSIDE_SETTING_FAILED_CODE: &str = "threads-outside-agents-failed";
