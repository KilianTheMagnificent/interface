//! Read-only projections over the Mac's authoritative terminal roster.
use interface_contracts::{
    engine::{SessionSummary, session_key},
    remote::{
        RemoteWorkspaceSnapshotV2, RemoteWorkspaceSnapshotV3, RemoteWorkspaceSnapshotV4,
        RemoteWorkspaceV2,
    },
    terminal::{
        TerminalLaunchProfileId, TerminalSnapshot, TerminalSummary, TerminalWorkspaceRecord,
    },
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Roster v2. Like v1 and v3 it keeps its bytes: no terminal carries the
/// host's `attention`, which only roster v4 sends.
#[must_use]
pub fn workspace_snapshot(
    snapshot: TerminalSnapshot,
    saved: &[TerminalWorkspaceRecord],
) -> RemoteWorkspaceSnapshotV2 {
    RemoteWorkspaceSnapshotV2 {
        version: 2,
        workspaces: grouped(without_attention(snapshot), saved),
    }
}

/// A terminal snapshot as the rosters before v4 send it.
#[must_use]
pub fn without_attention(mut snapshot: TerminalSnapshot) -> TerminalSnapshot {
    for terminal in &mut snapshot.terminals {
        terminal.attention = None;
    }
    snapshot
}

/// Saved workspaces in the Mac's order holding their live terminals, then the
/// unassigned terminals grouped by root.
fn grouped(
    snapshot: TerminalSnapshot,
    saved: &[TerminalWorkspaceRecord],
) -> Vec<RemoteWorkspaceV2> {
    let mut seen = BTreeSet::new();
    let mut workspaces = Vec::new();
    for workspace in saved {
        let terminals: Vec<_> = workspace
            .terminals
            .iter()
            .filter_map(|stored| {
                snapshot
                    .terminals
                    .iter()
                    .find(|live| live.id == stored.id)
                    .filter(|live| seen.insert(live.id))
                    .cloned()
            })
            .collect();
        if let Some(first) = terminals.first() {
            workspaces.push(RemoteWorkspaceV2 {
                id: workspace.id.clone(),
                name: workspace.name.clone(),
                root: first.root.clone(),
                terminals,
            });
        }
    }
    for terminal in snapshot.terminals {
        if seen.contains(&terminal.id) {
            continue;
        }
        let id = format!("unassigned:{}", terminal.root);
        if let Some(workspace) = workspaces.iter_mut().find(|w| w.id == id) {
            workspace.terminals.push(terminal);
        } else {
            workspaces.push(RemoteWorkspaceV2 {
                id,
                name: terminal
                    .root
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or("Terminal")
                    .into(),
                root: terminal.root.clone(),
                terminals: vec![terminal],
            });
        }
    }
    workspaces
}

/// The v2 roster plus [`terminal_projects`] for every terminal it lists.
#[must_use]
pub fn workspace_snapshot_v3(
    snapshot: TerminalSnapshot,
    saved: &[TerminalWorkspaceRecord],
    sessions: &[SessionSummary],
) -> RemoteWorkspaceSnapshotV3 {
    let projects = SessionProjects::index(sessions, &bound_session_keys(&snapshot.terminals));
    workspace_snapshot_v3_indexed(snapshot, saved, &projects)
}

/// [`workspace_snapshot_v3`] over a registry already indexed with
/// [`SessionProjects::index`], so a frequent roster read copies no row.
#[must_use]
pub fn workspace_snapshot_v3_indexed(
    snapshot: TerminalSnapshot,
    saved: &[TerminalWorkspaceRecord],
    sessions: &SessionProjects,
) -> RemoteWorkspaceSnapshotV3 {
    let workspaces = workspace_snapshot(snapshot, saved).workspaces;
    let terminals: Vec<_> = workspaces.iter().flat_map(|w| &w.terminals).collect();
    RemoteWorkspaceSnapshotV3 {
        version: 3,
        projects: indexed_terminal_projects(terminals, sessions),
        workspaces,
    }
}

/// Roster v4 (review D6): the v3 roster whose terminals keep the host's
/// `attention`, so the phone can say which sessions need you.
#[must_use]
pub fn workspace_snapshot_v4_indexed(
    snapshot: TerminalSnapshot,
    saved: &[TerminalWorkspaceRecord],
    sessions: &SessionProjects,
) -> RemoteWorkspaceSnapshotV4 {
    let workspaces = grouped(snapshot, saved);
    let terminals: Vec<_> = workspaces.iter().flat_map(|w| &w.terminals).collect();
    RemoteWorkspaceSnapshotV4 {
        version: 4,
        projects: indexed_terminal_projects(terminals, sessions),
        workspaces,
    }
}

/// The session keys `terminals` are bound to: what [`SessionProjects::index`]
/// needs to index for them.
#[must_use]
pub fn bound_session_keys(terminals: &[TerminalSummary]) -> BTreeSet<&str> {
    terminals
        .iter()
        .filter_map(|terminal| terminal.session_key.as_deref())
        .collect()
}

/// What [`terminal_projects`] reads from the session registry, indexed while
/// the registry is borrowed so no row is copied: the project of each session
/// a terminal is bound to, and every distinct project root, deepest first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionProjects {
    bound: HashMap<String, Option<String>>,
    known: Vec<String>,
}

impl SessionProjects {
    /// Indexes the visible registry rows, in any order, for terminals bound
    /// to the session keys in `keys`.
    #[must_use]
    pub fn index<'a>(
        sessions: impl IntoIterator<Item = &'a SessionSummary>,
        keys: &BTreeSet<&str>,
    ) -> Self {
        let wanted: HashSet<(&str, &str)> =
            keys.iter().filter_map(|key| key.split_once('#')).collect();
        let mut bound = HashMap::new();
        let mut known = BTreeSet::new();
        for session in sessions {
            let project = session_project(session);
            if let Some(project) = project {
                known.insert(project);
            }
            if wanted.contains(&(session.engine.as_str(), session.session_id.as_str())) {
                bound.insert(
                    session_key(&session.engine, &session.session_id),
                    project.map(ToOwned::to_owned),
                );
            }
        }
        let mut known: Vec<String> = known.into_iter().map(ToOwned::to_owned).collect();
        known.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        Self { bound, known }
    }
}

fn session_project(session: &SessionSummary) -> Option<&str> {
    session
        .project_root
        .as_deref()
        .filter(|root| !root.is_empty())
        .map(normalize_project_path)
}

/// The project the Mac sidebar files each terminal under, keyed by terminal id;
/// `None` is its "Other" group. Mirrors `terminalProjectKeyOf` in
/// `electron/src/renderer/viewmodels.ts`, pinned by the shared
/// `remote-projects-v3.json` fixture: a terminal bound to a registry session
/// takes that session's repository root; any other joins the deepest session
/// project containing its workspace root, else that root. The renderer's memory
/// of chats that left the registry has no host counterpart.
#[must_use]
pub fn terminal_projects(
    terminals: &[TerminalSummary],
    sessions: &[SessionSummary],
) -> BTreeMap<String, Option<String>> {
    let keys = bound_session_keys(terminals);
    indexed_terminal_projects(terminals, &SessionProjects::index(sessions, &keys))
}

/// [`terminal_projects`] against an indexed registry.
#[must_use]
pub fn indexed_terminal_projects<'a>(
    terminals: impl IntoIterator<Item = &'a TerminalSummary>,
    sessions: &SessionProjects,
) -> BTreeMap<String, Option<String>> {
    terminals
        .into_iter()
        .map(|terminal| {
            let bound = terminal
                .session_key
                .as_ref()
                .and_then(|key| sessions.bound.get(key))
                .cloned();
            let project = bound.unwrap_or_else(|| {
                let root = terminal.root.trim();
                let root = if root.is_empty() || root == "/" {
                    terminal.cwd.trim()
                } else {
                    root
                };
                if root.is_empty() {
                    return None;
                }
                let root = normalize_project_path(root);
                Some(
                    sessions
                        .known
                        .iter()
                        .find(|project| project_owns_path(project, root))
                        .cloned()
                        .unwrap_or_else(|| root.to_owned()),
                )
            });
            (terminal.id.to_string(), project)
        })
        .collect()
}

/// `/a/web/` and `/a/web` are one project; a lone `/` keeps its slash.
fn normalize_project_path(root: &str) -> &str {
    let trimmed = root.trim_end_matches('/');
    if trimmed.is_empty() && !root.is_empty() {
        "/"
    } else {
        trimmed
    }
}

fn project_owns_path(project: &str, path: &str) -> bool {
    path == project
        || (project != "/"
            && path.len() > project.len()
            && path.starts_with(project)
            && path.as_bytes()[project.len()] == b'/')
}

/// Only a host-confirmed provider binding can authorize a transcript read.
/// The terminal's workspace sessionKey is deliberately never used here.
#[must_use]
pub fn history_key(terminal: &TerminalSummary) -> Option<String> {
    let engine = match terminal.launch.as_ref()?.profile_id {
        TerminalLaunchProfileId::Claude => "claude",
        TerminalLaunchProfileId::Codex => "codex",
        _ => return None,
    };
    let binding = terminal.launch_session.as_ref()?;
    let id = binding.id.as_deref().filter(|id| !id.is_empty())?;
    if !binding.resume {
        return None;
    }
    Some(format!("{engine}#{id}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn terminal() -> TerminalSummary {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-companion-v2.json"
        ))
        .unwrap();
        serde_json::from_value(fixture["snapshot"]["workspaces"][0]["terminals"][0].clone())
            .unwrap()
    }
    #[test]
    fn saved_order_names_and_live_identity_win_without_duplicate_or_stale_entries() {
        let first = terminal();
        let mut second = first.clone();
        second.id = interface_contracts::terminal::TerminalId::random();
        second.title = "Live second".into();
        let mut other = first.clone();
        other.id = interface_contracts::terminal::TerminalId::random();
        other.root = "/Other".into();
        let stored = |id| interface_contracts::terminal::TerminalWorkspaceTerminal {
            id,
            title: "Stale title".into(),
            cwd: first.cwd.clone(),
            session_key: None,
            launch: None,
            launch_session: None,
        };
        let saved = TerminalWorkspaceRecord {
            id: "saved".into(),
            name: "My workspace".into(),
            created_at: first.created_at.clone(),
            updated_at: first.updated_at.clone(),
            layout: None,
            terminals: vec![
                stored(second.id),
                stored(first.id),
                stored(first.id),
                stored(interface_contracts::terminal::TerminalId::random()),
            ],
        };
        let result = workspace_snapshot(
            TerminalSnapshot {
                revision: 9,
                terminals: vec![first.clone(), other, second.clone()],
            },
            &[saved],
        );
        assert_eq!(result.version, 2);
        assert_eq!(result.workspaces.len(), 2);
        assert_eq!(result.workspaces[0].name, "My workspace");
        assert_eq!(result.workspaces[0].terminals, vec![second, first]);
        assert_eq!(result.workspaces[1].name, "Other");
    }
    #[test]
    fn files_terminals_under_the_same_projects_as_the_mac_sidebar() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-projects-v3.json"
        ))
        .unwrap();
        let sessions: Vec<SessionSummary> =
            serde_json::from_value(fixture["sessions"].clone()).unwrap();
        let terminals: Vec<TerminalSummary> =
            serde_json::from_value(fixture["terminals"].clone()).unwrap();
        let expected: BTreeMap<String, Option<String>> =
            serde_json::from_value(fixture["expected"].clone()).unwrap();
        assert_eq!(terminal_projects(&terminals, &sessions), expected);
        let roster = workspace_snapshot_v3(
            TerminalSnapshot {
                revision: 1,
                terminals: terminals.clone(),
            },
            &[],
            &sessions,
        );
        assert_eq!(roster.version, 3);
        assert_eq!(roster.projects, expected);
        let json = serde_json::to_value(&roster).unwrap();
        assert_eq!(
            json["projects"]["00000000-0000-4000-8000-000000000002"],
            serde_json::Value::Null
        );
        assert!(serde_json::from_value::<RemoteWorkspaceSnapshotV3>(json).is_ok());
    }
    #[test]
    fn only_roster_v4_says_which_sessions_need_you() {
        let mut waiting = terminal();
        waiting.attention = Some(interface_contracts::terminal::TerminalAttentionV1 {
            version: 1,
            since: "2026-10-14T09:12:00.000Z".into(),
        });
        let snapshot = TerminalSnapshot {
            revision: 3,
            terminals: vec![waiting.clone()],
        };
        let v2 = workspace_snapshot(snapshot.clone(), &[]);
        assert!(v2.workspaces[0].terminals[0].attention.is_none());
        let index = SessionProjects::default();
        let v3 = workspace_snapshot_v3_indexed(snapshot.clone(), &[], &index);
        assert!(v3.workspaces[0].terminals[0].attention.is_none());
        let json = serde_json::to_string(&v3).unwrap();
        assert!(!json.contains("attention"), "v3 keeps its bytes");
        let v4 = workspace_snapshot_v4_indexed(snapshot.clone(), &[], &index);
        assert_eq!(v4.version, 4);
        assert_eq!(v4.workspaces[0].terminals[0], waiting);
        assert_eq!(v4.projects, v3.projects);
        assert!(without_attention(snapshot).terminals[0].attention.is_none());
    }

    #[test]
    fn the_session_index_ignores_row_order_and_duplicate_roots() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-projects-v3.json"
        ))
        .unwrap();
        let mut sessions: Vec<SessionSummary> =
            serde_json::from_value(fixture["sessions"].clone()).unwrap();
        let terminals: Vec<TerminalSummary> =
            serde_json::from_value(fixture["terminals"].clone()).unwrap();
        let expected: BTreeMap<String, Option<String>> =
            serde_json::from_value(fixture["expected"].clone()).unwrap();
        let keys: BTreeSet<&str> = terminals
            .iter()
            .filter_map(|terminal| terminal.session_key.as_deref())
            .collect();
        // A second chat in an existing project, written with a trailing slash.
        let mut twin = sessions[0].clone();
        twin.session_id = "twin".into();
        twin.project_root = twin.project_root.map(|root| format!("{root}/"));
        sessions.push(twin);
        sessions.reverse();
        let index = SessionProjects::index(&sessions, &keys);
        assert_eq!(indexed_terminal_projects(&terminals, &index), expected);
        assert_eq!(
            index.known.len(),
            sessions
                .iter()
                .filter_map(session_project)
                .collect::<BTreeSet<_>>()
                .len()
        );
        // Only the sessions a terminal is bound to are indexed by key.
        assert!(index.bound.keys().all(|key| keys.contains(key.as_str())));
        assert!(!index.bound.contains_key("claude#twin"));
    }
    #[test]
    fn only_confirmed_supported_provider_binding_authorizes_history() {
        let mut t = terminal();
        assert_eq!(
            history_key(&t).as_deref(),
            Some("claude#confirmed-provider-chat")
        );
        t.launch_session.as_mut().unwrap().resume = false;
        assert_eq!(history_key(&t), None);
        t.launch_session = None;
        assert_eq!(
            history_key(&t),
            None,
            "workspace context must never authorize another chat"
        );
        t = terminal();
        t.launch.as_mut().unwrap().profile_id = TerminalLaunchProfileId::Shell;
        assert_eq!(history_key(&t), None);
    }
}
