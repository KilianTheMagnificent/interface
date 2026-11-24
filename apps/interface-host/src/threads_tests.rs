use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use interface_contracts::terminal::TerminalLaunchProfileId;
use interface_contracts::{
    engine::{
        EngineEvent, EngineEventKind, EngineId, SessionKind, SessionOrigin, SessionStatus,
        SessionSummary, session_key,
    },
    ipc::HistoryPage,
    threads::{
        BranchList, LineageRelation, PullRequestState, RestoreResult, RunOutcome, SettledReason,
        ThreadDiff, ThreadSection, ThreadView, TitleSource, TurnState, WorkspaceKept,
        WorkspaceMode,
    },
};
use interface_engine::ForkBoundary;
use interface_review::thread_git::{
    CaptureOutcome, DiffRequest, GitWorkspaceRequest, PreparedGitWorkspace, RemoveOutcome,
    ThreadGitError,
};
use interface_runtime::{RuntimeError, RuntimeErrorKind};
use interface_settings::threads::{
    FoundPullRequest, ReleaseOutcome, THREADS_BUSY_CODE, THREADS_INVALID_CODE,
    THREADS_NO_CHECKPOINT_CODE, THREADS_NOT_OWNED_CODE, THREADS_SHARED_CODE,
    THREADS_TURN_FILES_UNKNOWN_CODE, THREADS_UNKNOWN_CODE, THREADS_UNKNOWN_TURN_CODE,
    THREADS_UNKNOWN_WORKSPACE_CODE, ThreadRecord, pending_baseline_ref, prompt_digest,
    thread_ref_namespace, turn_ref,
};
use serde_json::json;

use super::run_tap::{RunChange, STEER_RESTART_WINDOW_MS};
use super::*;
use crate::dispatcher::{DEFINITELY_UNSENT_CODE, RegistryRoundObserver, SendHooks, SendSettlement};
use crate::session_management::RenameHooks;
use interface_contracts::away::ThreadAway;

const NOW: u64 = 1_791_300_000_000;
const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

fn iso(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(i64::try_from(ms).unwrap())
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn row(key: &str, kind: SessionKind) -> SessionSummary {
    let (engine, id) = key.split_once('#').unwrap();
    SessionSummary {
        engine: EngineId::new(engine).unwrap(),
        session_id: id.into(),
        title: format!("Title {id}"),
        cwd: "/private/var/empty".into(),
        status: SessionStatus::Idle,
        cloud: false,
        model: None,
        effort: None,
        last_activity: iso(NOW - HOUR),
        first_seen: iso(NOW - HOUR),
        archived: false,
        size: None,
        title_from_summary: None,
        origin: Some(SessionOrigin::App),
        project_root: None,
        worktree_home: None,
        worktree_path: None,
        setting_sources: None,
        disallowed_tools: None,
        kind: Some(kind),
        permission_mode: None,
    }
}

#[derive(Default)]
struct FakeSessions {
    rows: Mutex<Vec<SessionSummary>>,
    live: Mutex<HashSet<String>>,
    /// Each fork's key, kept prompt digest and later repeats of it.
    forks: Mutex<Vec<(String, String, u32)>>,
    renames: Mutex<Vec<(String, String)>>,
    next_fork: Mutex<Option<String>>,
    /// The transcript page `history` answers; none is a read failure.
    history: Mutex<Option<HistoryPage>>,
    /// What `import_history` answers when set (the whole transcript); else
    /// it answers as `history` does.
    whole_history: Mutex<Option<HistoryPage>>,
    /// Live sessions `close` was asked to close, in order.
    closes: Mutex<Vec<String>>,
    /// `close` fails and leaves the session open.
    close_fails: AtomicBool,
    /// The registry is being rebuilt from provider listings.
    rebuilding: AtomicBool,
    /// Each archive or restore asked for: key, archived.
    archives: Mutex<Vec<(String, bool)>>,
    /// Each removal asked for: key, deleted (else forgotten).
    removals: Mutex<Vec<(String, bool)>>,
}

impl FakeSessions {
    fn upsert(&self, summary: SessionSummary) {
        let mut rows = lock(&self.rows);
        let key = session_key(&summary.engine, &summary.session_id);
        rows.retain(|row| session_key(&row.engine, &row.session_id) != key);
        rows.push(summary);
    }

    fn edit(&self, key: &str, edit: impl FnOnce(&mut SessionSummary)) {
        let mut rows = lock(&self.rows);
        let row = rows
            .iter_mut()
            .find(|row| session_key(&row.engine, &row.session_id) == key)
            .unwrap();
        edit(row);
    }

    fn remove(&self, key: &str) {
        lock(&self.rows).retain(|row| session_key(&row.engine, &row.session_id) != key);
    }
}

#[async_trait]
impl ThreadSessions for FakeSessions {
    fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        Ok(lock(&self.rows).clone())
    }

    fn session(&self, key: &str) -> Result<Option<SessionSummary>, String> {
        Ok(lock(&self.rows)
            .iter()
            .find(|row| session_key(&row.engine, &row.session_id) == key)
            .cloned())
    }

    fn registry_complete(&self) -> bool {
        !self.rebuilding.load(Ordering::SeqCst)
    }

    fn live_keys(&self) -> Result<HashSet<String>, String> {
        Ok(lock(&self.live).clone())
    }

    async fn fork_at(&self, key: &str, boundary: ForkBoundary) -> Result<String, RuntimeError> {
        lock(&self.forks).push((
            key.to_owned(),
            boundary.prompt_digest,
            boundary.later_same_digest,
        ));
        let Some(child) = lock(&self.next_fork).take() else {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::Provider,
                "This provider cannot fork at an earlier turn.",
                false,
            ));
        };
        let mut copy = self.session(key).unwrap().unwrap();
        copy.session_id = child.split_once('#').unwrap().1.to_owned();
        self.upsert(copy);
        Ok(child)
    }

    async fn rename(&self, key: &str, title: String) -> Result<(), RuntimeError> {
        lock(&self.renames).push((key.to_owned(), title));
        Ok(())
    }

    async fn close(&self, key: &str) -> Result<(), String> {
        if !lock(&self.live).contains(key) {
            return Ok(());
        }
        lock(&self.closes).push(key.to_owned());
        if self.close_fails.load(Ordering::SeqCst) {
            return Err("The provider did not confirm the close.".into());
        }
        lock(&self.live).remove(key);
        Ok(())
    }

    async fn import_history(&self, key: &str) -> Result<HistoryPage, RuntimeError> {
        if let Some(page) = lock(&self.whole_history).clone() {
            return Ok(page);
        }
        self.history(key).await
    }

    async fn history(&self, key: &str) -> Result<HistoryPage, RuntimeError> {
        lock(&self.history).clone().ok_or_else(|| {
            RuntimeError::definitely_unsent(
                RuntimeErrorKind::Provider,
                format!("Couldn't load {key}."),
                false,
            )
        })
    }

    async fn set_archived(&self, key: &str, archived: bool) -> Result<(), String> {
        lock(&self.archives).push((key.to_owned(), archived));
        self.edit(key, |row| row.archived = archived);
        Ok(())
    }

    async fn remove(&self, key: &str, deleted: bool) -> Result<(), String> {
        lock(&self.removals).push((key.to_owned(), deleted));
        FakeSessions::remove(self, key);
        Ok(())
    }
}

struct FakeTitles {
    available: AtomicBool,
    calls: Mutex<Vec<(String, String)>>,
    reply: Mutex<Result<String, String>>,
    /// Write with AI (protocol 67): what each call was given.
    ship_calls: Mutex<Vec<(interface_contracts::threads::ShipTextKind, String)>>,
}

impl Default for FakeTitles {
    fn default() -> Self {
        Self {
            available: AtomicBool::new(true),
            calls: Mutex::new(Vec::new()),
            reply: Mutex::new(Ok("Login test fixed".into())),
            ship_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ThreadTitles for FakeTitles {
    fn available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    async fn generate(&self, first_prompt: String, first_answer: String) -> Result<String, String> {
        lock(&self.calls).push((first_prompt, first_answer));
        lock(&self.reply).clone()
    }

    async fn write_ship_text(
        &self,
        what: interface_contracts::threads::ShipTextKind,
        material: String,
    ) -> Result<String, String> {
        lock(&self.ship_calls).push((what, material));
        lock(&self.reply).clone()
    }
}

#[derive(Default)]
struct FakeHidden(Mutex<BTreeSet<String>>);

#[async_trait]
impl HiddenSessions for FakeHidden {
    async fn hidden_keys(&self) -> BTreeSet<String> {
        lock(&self.0).clone()
    }
}

#[derive(Default)]
struct FakeAuthority(Mutex<HashSet<String>>);

#[async_trait]
impl FolderAuthority for FakeAuthority {
    async fn may_open(&self, folder: &str) -> Result<(), DispatchError> {
        if lock(&self.0).contains(folder) {
            return Err(DispatchError::new(
                THREADS_FOLDER_REFUSED_CODE,
                format!("Interface may not open {folder}."),
                false,
                true,
            ));
        }
        Ok(())
    }
}

#[derive(Default)]
struct FakeGit {
    calls: Mutex<Vec<String>>,
    prepared: Mutex<Option<PreparedGitWorkspace>>,
    ensure_fails: AtomicBool,
    capture_skips: AtomicBool,
    moves: AtomicBool,
    remove: Mutex<Option<RemoveOutcome>>,
    worktrees: Mutex<Vec<(PathBuf, Option<String>)>>,
}

impl FakeGit {
    fn call(&self, call: String) {
        lock(&self.calls).push(call);
    }

    fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }

    fn called(&self, call: &str) -> bool {
        self.calls().iter().any(|made| made == call)
    }
}

impl ThreadGit for FakeGit {
    fn prepare_workspace(
        &self,
        request: &GitWorkspaceRequest,
    ) -> Result<PreparedGitWorkspace, ThreadGitError> {
        self.call(format!("prepare {}", request.project_root.display()));
        lock(&self.prepared)
            .clone()
            .ok_or_else(ThreadGitError::unsupported)
    }

    fn discard_workspace(
        &self,
        prepared: &PreparedGitWorkspace,
    ) -> Result<RemoveOutcome, ThreadGitError> {
        self.call(format!("discard {}", prepared.cwd.display()));
        Ok(RemoveOutcome::Removed)
    }

    fn ensure_worktree(
        &self,
        _repo_root: &Path,
        worktree_path: &Path,
        _branch: &str,
    ) -> Result<bool, ThreadGitError> {
        self.call(format!("ensure {}", worktree_path.display()));
        if self.ensure_fails.load(Ordering::SeqCst) {
            return Err(ThreadGitError::new(
                "thread-git-failed",
                "the branch no longer exists.",
            ));
        }
        std::fs::create_dir_all(worktree_path).unwrap();
        Ok(true)
    }

    fn capture_checkpoint(
        &self,
        _cwd: &Path,
        refname: &str,
    ) -> Result<CaptureOutcome, ThreadGitError> {
        self.call(format!("capture {refname}"));
        if self.capture_skips.load(Ordering::SeqCst) {
            return Ok(CaptureOutcome::Skipped {
                reason: "over 20000 paths".into(),
            });
        }
        Ok(CaptureOutcome::Captured {
            commit: "c0ffee".into(),
        })
    }

    fn numstat(
        &self,
        _cwd: &Path,
        from_ref: &str,
        to_ref: Option<&str>,
    ) -> Result<(u32, u32, u32), ThreadGitError> {
        self.call(format!(
            "numstat {from_ref} {}",
            to_ref.unwrap_or("worktree")
        ));
        Ok((2, 7, 1))
    }

    fn numstat_paths(
        &self,
        _cwd: &Path,
        from_ref: &str,
        to_ref: Option<&str>,
        paths: &[String],
    ) -> Result<(u32, u32, u32), ThreadGitError> {
        self.call(format!(
            "numstat-paths {from_ref} {} {}",
            to_ref.unwrap_or("worktree"),
            paths.join(",")
        ));
        Ok((1, 1, 8))
    }

    /// Strips `cwd/` from absolute paths; a relative path is the folder's,
    /// which in these tests is the repository root.
    fn repo_relative_paths(
        &self,
        cwd: &Path,
        paths: &[String],
    ) -> Result<Vec<String>, ThreadGitError> {
        let prefix = format!("{}/", cwd.display());
        let mut relative = paths
            .iter()
            .map(|path| path.strip_prefix(&prefix).unwrap_or(path).to_owned())
            .collect::<Vec<_>>();
        relative.sort_unstable();
        relative.dedup();
        Ok(relative)
    }

    fn diff(&self, _cwd: &Path, request: &DiffRequest) -> Result<ThreadDiff, ThreadGitError> {
        let paths = request
            .paths
            .as_ref()
            .map_or(String::new(), |paths| format!(" only {}", paths.join(",")));
        self.call(format!(
            "diff {} {}{paths}",
            request.base,
            request.head.as_deref().unwrap_or("worktree")
        ));
        Ok(ThreadDiff {
            scope: request.scope,
            base: request.base.clone(),
            head: request.head.clone().unwrap_or_else(|| "worktree".into()),
            files: Vec::new(),
            truncated: false,
        })
    }

    fn merge_base(&self, _cwd: &Path, base_ref: &str) -> Result<Option<String>, ThreadGitError> {
        self.call(format!("merge-base {base_ref}"));
        Ok(Some("base000".into()))
    }

    fn restore_to(
        &self,
        _cwd: &Path,
        refname: &str,
        recovery_ref: &str,
    ) -> Result<RestoreResult, ThreadGitError> {
        self.call(format!("restore {refname} {recovery_ref}"));
        Ok(RestoreResult {
            restored: 3,
            recovery_ref: recovery_ref.to_owned(),
        })
    }

    fn remove_worktree(
        &self,
        _repo_root: &Path,
        worktree_path: &Path,
    ) -> Result<RemoveOutcome, ThreadGitError> {
        self.call(format!("remove {}", worktree_path.display()));
        Ok(lock(&self.remove).unwrap_or(RemoveOutcome::Removed))
    }

    fn list_branches(&self, _project_root: &Path) -> Result<BranchList, ThreadGitError> {
        Ok(BranchList {
            current: Some("main".into()),
            default_branch: Some("main".into()),
            branches: vec!["main".into(), "iface/fix-1a2b3c".into()],
        })
    }

    fn list_worktrees(
        &self,
        _project_root: &Path,
    ) -> Result<Vec<(PathBuf, Option<String>)>, ThreadGitError> {
        Ok(lock(&self.worktrees).clone())
    }

    fn delete_refs(&self, _cwd: &Path, prefix: &str) -> Result<u32, ThreadGitError> {
        self.call(format!("delete {prefix}"));
        Ok(1)
    }

    fn move_ref(&self, _cwd: &Path, from: &str, to: &str) -> Result<(), ThreadGitError> {
        self.call(format!("move {from} {to}"));
        if self.moves.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ThreadGitError::unsupported())
        }
    }
}

struct FakePulls(Mutex<PullRequestLookup>);

impl PullRequests for FakePulls {
    fn lookup(&self, _project_root: &Path, _branch: &str) -> PullRequestLookup {
        lock(&self.0).clone()
    }

    fn view(&self, _project_root: &Path, _number: u64) -> PullRequestLookup {
        lock(&self.0).clone()
    }
}

/// The `gh` ship commands find: none (missing) unless a test sets a fake.
#[derive(Default)]
struct FakeShipTools(Mutex<Option<interface_review::thread_git::ship::Gh>>);

impl ShipTools for FakeShipTools {
    fn gh(&self) -> Option<interface_review::thread_git::ship::Gh> {
        lock(&self.0).clone()
    }
}

/// The consult path: records each resume and submit and answers a consult
/// awaiting the owner's approval.
#[derive(Default)]
struct FakeConsults {
    resumes: Mutex<Vec<String>>,
    submits: Mutex<Vec<interface_contracts::ipc::CollaborationSubmitRequest>>,
    resume_fails: AtomicBool,
}

#[async_trait]
impl ThreadConsults for FakeConsults {
    async fn resume(&self, key: &str) -> Result<(), DispatchError> {
        lock(&self.resumes).push(key.to_owned());
        if self.resume_fails.load(Ordering::SeqCst) {
            return Err(DispatchError::new(
                DEFINITELY_UNSENT_CODE,
                "The provider is not ready.",
                true,
                true,
            ));
        }
        Ok(())
    }

    async fn submit(
        &self,
        request: interface_contracts::ipc::CollaborationSubmitRequest,
    ) -> Result<interface_contracts::ipc::CollaborationStatusDto, DispatchError> {
        use interface_contracts::ipc::{
            CollaborationEngine, CollaborationReceiptDto, CollaborationState,
            CollaborationStatusDto,
        };
        lock(&self.submits).push(request.clone());
        Ok(CollaborationStatusDto {
            receipt: CollaborationReceiptDto {
                version: 1,
                collaboration_id: format!("collaboration-{}", request.client_request_id),
                revision: 2,
                state: CollaborationState::AwaitingApproval,
                mode: request.mode,
                source_engine: CollaborationEngine::Claude,
                target_engine: request.target_engine,
                approval_required: true,
                cancellation_safe: true,
                created_at: "2026-10-14T12:00:00.000Z".into(),
                updated_at: "2026-10-14T12:00:01.000Z".into(),
                error: None,
            },
            source_session_key: request.source_session_key,
            target_session_key: None,
            approval: None,
            peer: None,
        })
    }
}

/// The provider sessions running Code panes are bound to.
#[derive(Default)]
struct FakePanes(Mutex<Vec<String>>);

impl interface_runtime::TerminalSessionBindings for FakePanes {
    fn bound_provider_sessions(&self) -> Vec<String> {
        lock(&self.0).clone()
    }
}

struct Harness {
    host: ThreadHost,
    panes: Arc<FakePanes>,
    claims: Arc<interface_runtime::SessionClaims>,
    consults: Arc<FakeConsults>,
    sessions: Arc<FakeSessions>,
    git: Arc<FakeGit>,
    hidden: Arc<FakeHidden>,
    authority: Arc<FakeAuthority>,
    pulls: Arc<FakePulls>,
    ship: Arc<FakeShipTools>,
    titles: Arc<FakeTitles>,
    clock: Arc<AtomicU64>,
    pushes: mpsc::Receiver<PushFrame>,
    work: tempfile::TempDir,
    state: tempfile::TempDir,
}

fn harness() -> Harness {
    let state = tempfile::tempdir().unwrap();
    let store = ThreadStore::open(&state.path().join(THREADS_FILE_NAME), NOW)
        .map_err(|error| error.to_string());
    harness_with(store, state)
}

fn harness_with(store: Result<ThreadStore, String>, state: tempfile::TempDir) -> Harness {
    harness_on(store, state, None)
}

/// A harness whose git port is `git` instead of its fake (which then sees
/// no calls).
fn harness_on(
    store: Result<ThreadStore, String>,
    state: tempfile::TempDir,
    git_port: Option<Arc<dyn ThreadGit>>,
) -> Harness {
    harness_built(store, state, git_port, None, false)
}

/// A harness whose hidden-sessions port is `hidden` instead of its fake.
fn hidden_harness(hidden: Arc<dyn HiddenSessions>) -> Harness {
    let state = tempfile::tempdir().unwrap();
    let store = ThreadStore::open(&state.path().join(THREADS_FILE_NAME), NOW)
        .map_err(|error| error.to_string());
    harness_built(store, state, None, Some(hidden), false)
}

/// A harness whose host journals threads under its state directory.
fn journal_harness() -> Harness {
    let state = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = ThreadStore::open(&state.path().join(THREADS_FILE_NAME), NOW)
        .map_err(|error| error.to_string());
    harness_built(store, state, None, None, true)
}

fn harness_built(
    store: Result<ThreadStore, String>,
    state: tempfile::TempDir,
    git_port: Option<Arc<dyn ThreadGit>>,
    hidden_port: Option<Arc<dyn HiddenSessions>>,
    journal: bool,
) -> Harness {
    let sessions = Arc::new(FakeSessions::default());
    let git = Arc::new(FakeGit::default());
    let hidden = Arc::new(FakeHidden::default());
    let authority = Arc::new(FakeAuthority::default());
    let pulls = Arc::new(FakePulls(Mutex::new(PullRequestLookup::Unavailable)));
    let ship = Arc::new(FakeShipTools::default());
    let titles = Arc::new(FakeTitles::default());
    let panes = Arc::new(FakePanes::default());
    let claims = Arc::new(interface_runtime::SessionClaims::new(panes.clone()));
    let consults = Arc::new(FakeConsults::default());
    let clock = Arc::new(AtomicU64::new(NOW));
    let (sender, pushes) = mpsc::channel(256);
    let time = Arc::clone(&clock);
    let host = ThreadHost::new(ThreadHostConfig {
        store,
        ports: ThreadPorts {
            sessions: sessions.clone(),
            hidden: hidden_port.unwrap_or_else(|| hidden.clone() as Arc<dyn HiddenSessions>),
            authority: authority.clone(),
            git: git_port.unwrap_or_else(|| git.clone() as Arc<dyn ThreadGit>),
            pull_requests: pulls.clone(),
            ship: ship.clone(),
            titles: titles.clone(),
            clock: Arc::new(move || time.load(Ordering::SeqCst)),
            claims: claims.clone(),
            consults: consults.clone(),
        },
        pushes: Some(sender),
        registry_pushes: None,
        registry: Arc::new(ThreadRegistrySignal::default()),
        journal: journal.then(|| crate::threads::ThreadJournalConfig {
            data_root: state.path().to_path_buf(),
            notices: None,
        }),
    });
    Harness {
        host,
        panes,
        claims,
        consults,
        sessions,
        git,
        hidden,
        authority,
        pulls,
        ship,
        titles,
        clock,
        pushes,
        work: tempfile::tempdir().unwrap(),
        state,
    }
}

/// A host over a fresh store with no sessions, for the router's tests. Keep
/// the directory alive as long as the host.
pub(crate) fn router_host() -> (ThreadHost, tempfile::TempDir) {
    let harness = harness();
    (harness.host, harness.state)
}

impl Harness {
    async fn command(&self, command: Value) -> Result<ThreadControlResponse, DispatchError> {
        self.host
            .dispatch(CHANNELS.thread_control, command)
            .await
            .map(|value| serde_json::from_value(value).unwrap())
    }

    fn advance(&self, by: u64) {
        self.clock.fetch_add(by, Ordering::SeqCst);
    }

    fn pushed(&mut self) -> Vec<ThreadsView> {
        let mut views = Vec::new();
        while let Ok(frame) = self.pushes.try_recv() {
            assert_eq!(frame.operation, CHANNELS.threads);
            views.push(frame.payload.decode().unwrap());
        }
        views
    }

    fn dir(&self, name: &str) -> String {
        let path = self.work.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn missing_dir(&self, name: &str) -> String {
        self.work.path().join(name).to_string_lossy().into_owned()
    }

    async fn document(&self) -> ThreadsDocument {
        self.host
            .inner
            .store
            .lock()
            .await
            .as_ref()
            .unwrap()
            .document()
            .clone()
    }

    /// Prepares a local git workspace at `root`; answers its token.
    async fn prepare_local(&self, root: &str) -> String {
        *lock(&self.git.prepared) = Some(PreparedGitWorkspace {
            cwd: PathBuf::from(root),
            repo_root: Some(PathBuf::from(root)),
            worktree_path: None,
            branch: Some("main".into()),
            base_ref: None,
            created: false,
        });
        self.command(json!({
            "kind": "prepareWorkspace",
            "projectRoot": root,
            "mode": "local",
            "hint": "x",
        }))
        .await
        .unwrap()
        .prepared
        .unwrap()
        .token
    }

    /// Prepares a git workspace (local at `root`, or the worktree
    /// `worktree`), registers `key` and binds it with `prompt`. The launch
    /// run stays open in the store; the tap's copy is closed.
    async fn git_thread(
        &mut self,
        key: &str,
        root: &str,
        worktree: Option<&str>,
        prompt: &str,
    ) -> ThreadControlResponse {
        *lock(&self.git.prepared) = Some(PreparedGitWorkspace {
            cwd: PathBuf::from(worktree.unwrap_or(root)),
            repo_root: Some(PathBuf::from(root)),
            worktree_path: worktree.map(PathBuf::from),
            branch: Some(if worktree.is_some() {
                "iface/fix-1a2b3c".into()
            } else {
                "main".into()
            }),
            base_ref: worktree.map(|_| "main".into()),
            created: worktree.is_some(),
        });
        let mode = if worktree.is_some() {
            "worktree"
        } else {
            "local"
        };
        let prepared = self
            .command(json!({
                "kind": "prepareWorkspace",
                "projectRoot": root,
                "mode": mode,
                "hint": prompt,
            }))
            .await
            .unwrap()
            .prepared
            .unwrap();
        self.sessions.upsert(row(key, SessionKind::Code));
        let response = self
            .command(json!({
                "kind": "bind",
                "key": key,
                "token": prepared.token,
                "prompt": prompt,
            }))
            .await
            .unwrap();
        self.host.close_run(key);
        // The automatic title asked for as the launch prompt went out.
        settle_tasks(&self.host).await;
        response
    }
}

fn thread<'a>(view: &'a ThreadsView, key: &str) -> &'a ThreadView {
    view.threads
        .iter()
        .find(|thread| thread.key == key)
        .unwrap_or_else(|| panic!("no thread {key}"))
}

async fn settle_tasks(host: &ThreadHost) {
    loop {
        let tasks = std::mem::take(&mut *lock(&host.inner.tasks));
        if tasks.is_empty() {
            return;
        }
        for task in tasks {
            task.await.unwrap();
        }
    }
}

#[tokio::test]
async fn metadata_commands_answer_the_view_and_push_after_each_committed_change() {
    let mut harness = harness();
    harness.sessions.upsert(row("claude#a", SessionKind::Chat));
    harness.sessions.upsert(row("codex#b", SessionKind::Code));
    harness.host.reconcile().await;
    let seeded = harness.pushed();
    assert_eq!(seeded.len(), 1);
    assert_eq!(seeded[0].threads.len(), 2);
    let mut revision = seeded[0].revision;

    for (command, section) in [
        (
            json!({"kind": "pin", "key": "claude#a"}),
            ThreadSection::Pinned,
        ),
        (
            json!({"kind": "snooze", "key": "codex#b", "until": NOW + HOUR}),
            ThreadSection::Snoozed,
        ),
        (
            json!({"kind": "unsnooze", "key": "codex#b"}),
            ThreadSection::Active,
        ),
        (
            json!({"kind": "settle", "key": "codex#b"}),
            ThreadSection::Settled,
        ),
        (
            json!({"kind": "unsettle", "key": "codex#b"}),
            ThreadSection::Active,
        ),
        (
            json!({"kind": "unpin", "key": "claude#a"}),
            ThreadSection::Active,
        ),
        (
            json!({"kind": "reorder", "key": "claude#a", "section": "pinned"}),
            ThreadSection::Pinned,
        ),
    ] {
        let response = harness.command(command.clone()).await.unwrap();
        let key = command["key"].as_str().unwrap();
        assert_eq!(thread(&response.view, key).section, section, "{command}");
        assert_eq!(
            harness.pushed(),
            [response.view.clone()],
            "one push per commit: {command}"
        );
        assert_eq!(response.view.revision, revision + 1);
        revision += 1;
    }

    harness
        .command(json!({"kind": "markUnread", "key": "codex#b"}))
        .await
        .unwrap();
    harness
        .command(json!({"kind": "setAutoSettle", "key": "codex#b", "enabled": false}))
        .await
        .unwrap();
    let view = harness
        .command(json!({"kind": "visit", "key": "claude#a"}))
        .await
        .unwrap()
        .view;
    assert!(thread(&view, "codex#b").unread);
    assert!(!thread(&view, "codex#b").auto_settle);
    assert_eq!(thread(&view, "claude#a").last_visited_at, Some(NOW));
    assert_eq!(harness.pushed().len(), 3);

    // Nothing changed: an answer, no push, the same revision.
    let again = harness
        .command(json!({"kind": "pin", "key": "claude#a"}))
        .await
        .unwrap();
    assert_eq!(again.view.revision, view.revision);
    let read = harness.command(json!({"kind": "read"})).await.unwrap();
    assert_eq!(read.view.revision, view.revision);
    assert!(harness.pushed().is_empty());

    let settings = harness
        .command(json!({"kind": "setSettings", "settings": {
            "autoSettleDays": 0, "settleOnMerge": false,
            "generatedTitles": false, "trackPullRequests": false
        }}))
        .await
        .unwrap();
    assert_eq!(settings.view.settings.auto_settle_days, 0);
    assert_eq!(harness.pushed().len(), 1);

    let unknown = harness
        .command(json!({"kind": "settle", "key": "codex#gone"}))
        .await
        .unwrap_err();
    assert_eq!(unknown.code, THREADS_UNKNOWN_CODE);
    assert!(unknown.replay_safe && !unknown.retryable);
    let past = harness
        .command(json!({"kind": "snooze", "key": "codex#b", "until": NOW - 1}))
        .await
        .unwrap_err();
    assert_eq!(past.code, THREADS_INVALID_CODE);
    assert!(harness.pushed().is_empty());
}

/// Protocol 71: Settings › Threads › Pane tools without asking is saved
/// beside the Threads store (never in it), shown on every view, and rebuilds
/// the launch catalog so panes opened from then on follow it.
#[tokio::test]
async fn the_pane_tools_switch_is_saved_beside_the_store_and_rebuilds_the_launch_catalog() {
    struct Counting(std::sync::atomic::AtomicUsize);
    impl crate::provider_refresh::ProviderRebuild for Counting {
        fn rebuild(&self) -> Result<interface_terminal::TerminalLaunchCatalog, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(interface_terminal::TerminalLaunchCatalog::shell_only())
        }
    }
    let mut harness = harness();
    let rebuilds = Arc::new(Counting(std::sync::atomic::AtomicUsize::new(0)));
    harness
        .host
        .attach_provider_refresh(Arc::new(crate::provider_refresh::ProviderRefresh::new(
            rebuilds.clone(),
        )));
    let file = harness.state.path().join(THREADS_FILE_NAME);
    let read = harness.command(json!({"kind": "read"})).await.unwrap();
    assert!(
        !read.view.settings.pane_tools_without_asking,
        "off by default"
    );
    let settings = |on: bool| {
        json!({"kind": "setSettings", "settings": {
            "autoSettleDays": 3, "settleOnMerge": true,
            "generatedTitles": true, "trackPullRequests": true,
            "paneToolsWithoutAsking": on
        }})
    };
    let on = harness.command(settings(true)).await.unwrap();
    assert!(on.view.settings.pane_tools_without_asking);
    assert!(interface_settings::threads::pane_tools_without_asking_at(
        harness.state.path()
    ));
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap_or_else(|_| b"{}".to_vec())).unwrap();
    assert!(
        written
            .pointer("/settings/paneToolsWithoutAsking")
            .is_none(),
        "the Threads store never carries it"
    );
    assert!(!harness.pushed().is_empty(), "every window sees it");
    let read = harness.command(json!({"kind": "read"})).await.unwrap();
    assert!(read.view.settings.pane_tools_without_asking);
    for _ in 0..400 {
        if rebuilds.0.load(std::sync::atomic::Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(rebuilds.0.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Saving the same value again changes nothing and rebuilds nothing.
    harness.command(settings(true)).await.unwrap();
    let off = harness.command(settings(false)).await.unwrap();
    assert!(!off.view.settings.pane_tools_without_asking);
    assert!(!interface_settings::threads::pane_tools_without_asking_at(
        harness.state.path()
    ));
    for _ in 0..400 {
        if rebuilds.0.load(std::sync::atomic::Ordering::SeqCst) == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(rebuilds.0.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_session_the_store_has_not_seen_yet_is_seeded_before_a_command() {
    let mut harness = harness();
    harness.sessions.upsert(row("claude#a", SessionKind::Chat));
    let response = harness
        .command(json!({"kind": "pin", "key": "claude#a"}))
        .await
        .unwrap();
    assert_eq!(
        thread(&response.view, "claude#a").section,
        ThreadSection::Pinned
    );
    assert_eq!(harness.pushed().len(), 2, "the seed, then the pin");
}

#[tokio::test]
async fn seeding_lists_threads_surface_sessions_only_and_drops_what_leaves() {
    let mut harness = harness();
    let mut old = row("claude#old", SessionKind::Chat);
    old.last_activity = iso(NOW - 10 * DAY);
    let mut cli = row("codex#cli", SessionKind::Code);
    cli.origin = Some(SessionOrigin::Store);
    let mut busy_cli = row("codex#busy", SessionKind::Code);
    busy_cli.origin = None;
    busy_cli.status = SessionStatus::Working;
    for summary in [
        row("claude#chat", SessionKind::Chat),
        row("codex#code", SessionKind::Code),
        row("claude#voice", SessionKind::Voice),
        row("claude#agent", SessionKind::Agent),
        row("codex#worker", SessionKind::Code),
        old,
        cli,
        busy_cli,
    ] {
        harness.sessions.upsert(summary);
    }
    lock(&harness.hidden.0).insert("codex#worker".into());
    harness.host.reconcile().await;
    let view = harness.pushed().pop().unwrap();
    let keys = view
        .threads
        .iter()
        .map(|thread| thread.key.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "claude#chat",
            "claude#old",
            "claude#voice",
            "codex#busy",
            "codex#cli",
            "codex#code"
        ])
    );
    assert_eq!(
        thread(&view, "claude#old").settled_reason,
        Some(SettledReason::Idle)
    );
    assert_eq!(
        thread(&view, "codex#cli").settled_reason,
        Some(SettledReason::External)
    );
    assert_eq!(thread(&view, "codex#busy").section, ThreadSection::Active);
    assert_eq!(thread(&view, "claude#chat").section, ThreadSection::Active);

    // A session that turns out to be a worker, and one forgotten, leave
    // once they have been missing for a day.
    lock(&harness.hidden.0).insert("codex#code".into());
    harness.sessions.remove("claude#chat");
    harness.host.reconcile().await;
    assert_eq!(harness.document().await.records.len(), 6, "missing, kept");
    harness.advance(DAY);
    harness.host.reconcile().await;
    let view = harness.pushed().pop().unwrap();
    assert!(
        view.threads
            .iter()
            .all(|thread| thread.key != "codex#code" && thread.key != "claude#chat")
    );
    // An empty registry read never wipes the store.
    lock(&harness.sessions.rows).clear();
    harness.host.reconcile().await;
    assert!(harness.pushed().is_empty());
    assert_eq!(harness.document().await.records.len(), 4);
}

#[tokio::test]
async fn bind_attaches_the_prepared_workspace_and_turn_zero_on_its_baseline() {
    let harness = harness();
    let root = harness.dir("atlas");
    harness.git.moves.store(true, Ordering::SeqCst);
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: PathBuf::from(&root),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: None,
        branch: Some("main".into()),
        base_ref: None,
        created: false,
    });
    let response = harness
        .command(json!({
            "kind": "prepareWorkspace",
            "projectRoot": root,
            "mode": "local",
            "hint": "Fix the login test"
        }))
        .await
        .unwrap();
    let prepared = response.prepared.unwrap();
    assert!(prepared.token.starts_with("prep-"));
    assert_eq!(prepared.cwd, root);
    assert_eq!(prepared.workspace.mode, WorkspaceMode::Local);
    assert!(!prepared.workspace.owned);
    let baseline = pending_baseline_ref(&prepared.token);
    assert!(harness.git.called(&format!("capture {baseline}")));
    assert_eq!(harness.document().await.prepared.len(), 1);

    let key = "codex#new";
    let unknown = harness
        .command(json!({"kind": "bind", "key": key, "token": prepared.token, "prompt": "x"}))
        .await
        .unwrap_err();
    assert_eq!(unknown.code, THREADS_UNKNOWN_SESSION_CODE);
    // The launch registered the session; bind it.
    harness.sessions.upsert(row(key, SessionKind::Code));
    let response = harness
        .command(json!({
            "kind": "bind",
            "key": key,
            "token": prepared.token,
            "prompt": "Fix the login test\r\n"
        }))
        .await
        .unwrap();
    let bound = thread(&response.view, key);
    assert_eq!(bound.section, ThreadSection::Active);
    assert_eq!(bound.workspace.as_ref(), Some(&prepared.workspace));
    assert_eq!(bound.title_source, Some(TitleSource::Prompt));
    assert!(bound.lineage.is_none());
    assert_eq!(bound.turns.len(), 1);
    assert_eq!(bound.turns[0].ordinal, 0);
    assert_eq!(
        bound.turns[0].prompt_digest,
        prompt_digest("Fix the login test")
    );
    assert!(bound.turns[0].has_start);
    assert_eq!(bound.turns[0].state, TurnState::Running);
    assert!(
        bound
            .last_run
            .as_ref()
            .is_some_and(|run| run.ended_at.is_none())
    );
    let start = turn_ref(key, 0, false);
    assert!(harness.git.called(&format!("move {baseline} {start}")));
    let document = harness.document().await;
    assert!(document.prepared.is_empty());
    assert_eq!(
        document.record(key).unwrap().turns[0].start_ref.as_deref(),
        Some(start.as_str())
    );
    assert!(harness.host.run_tracked(key), "the launch run is tracked");

    let gone = harness
        .command(json!({"kind": "bind", "key": key, "token": "prep-gone", "prompt": "x"}))
        .await
        .unwrap_err();
    assert_eq!(gone.code, THREADS_UNKNOWN_WORKSPACE_CODE);
    harness
        .sessions
        .upsert(row("claude#agent", SessionKind::Agent));
    let agent = harness
        .command(json!({"kind": "bind", "key": "claude#agent", "prompt": "x"}))
        .await
        .unwrap_err();
    assert_eq!(agent.code, THREADS_INVALID_CODE);

    // A plain chat binds with no folder of the owner's.
    harness
        .sessions
        .upsert(row("claude#chat", SessionKind::Chat));
    let chat = harness
        .command(json!({"kind": "bind", "key": "claude#chat", "prompt": "hi"}))
        .await
        .unwrap();
    let chat = thread(&chat.view, "claude#chat");
    assert_eq!(chat.workspace.as_ref().unwrap().mode, WorkspaceMode::None);
    assert!(!chat.turns[0].has_start);
}

/// What a signed-out provider streams for a launch: the run starts, answers
/// with the error text, fails, then goes idle.
fn signed_out_launch(harness: &Harness, key: &str, idle: bool) {
    harness.advance(10);
    harness.host.observe_run(
        key,
        &EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
    );
    harness.host.observe_run(
        key,
        &EngineEventKind::MessageDelta {
            text: "Not logged in · Please run /login".into(),
            agent_key: None,
        },
    );
    harness.advance(30);
    harness.host.observe_run(
        key,
        &EngineEventKind::Error {
            message: "Not logged in".into(),
            retryable: false,
            cause: None,
        },
    );
    if idle {
        harness.host.observe_run(
            key,
            &EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            },
        );
    }
}

/// Turn 0 failed and the run ended at `ended`: where the tap saw the
/// failure, or the bind's own start of the run when nothing recorded the
/// run before it.
fn assert_launch_failed(view: &ThreadsView, key: &str, ended: u64) {
    let failed = thread(view, key);
    assert_eq!(failed.turns.len(), 1, "{key}");
    assert_eq!(failed.turns[0].state, TurnState::Failed, "{key}");
    assert_eq!(failed.turns[0].ended_at, Some(ended), "{key}");
    let run = failed.last_run.as_ref().unwrap();
    assert_eq!(
        (run.ended_at, run.outcome),
        (Some(ended), Some(RunOutcome::Failed)),
        "{key}"
    );
}

/// 14 Oct 2026: a launch the provider failed within milliseconds (the
/// hands-free fake's `signedout`) left turn 0 running, so the pane kept
/// "Thinking" and its clock while the inbox said Failed. The tap saw the
/// launch run start and fail before the renderer's bind; the bind then
/// opened a new run generation, which made the worker drop that end as
/// stale. The bind now ends turn 0 with the end the tap already saw.
#[tokio::test]
async fn a_launch_that_fails_before_its_bind_ends_turn_zero_failed() {
    let harness = harness();
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();

    // The worker applies neither op before the bind.
    let root = harness.dir("atlas");
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: PathBuf::from(&root),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: None,
        branch: Some("main".into()),
        base_ref: None,
        created: false,
    });
    let prepared = harness
        .command(json!({
            "kind": "prepareWorkspace",
            "projectRoot": root,
            "mode": "local",
            "hint": "signedout"
        }))
        .await
        .unwrap()
        .prepared
        .unwrap();
    let key = "claude#fast";
    harness.sessions.upsert(row(key, SessionKind::Code));
    signed_out_launch(&harness, key, true);
    let started = ops.try_recv().unwrap();
    let ended = ops.try_recv().unwrap();
    assert_eq!(ended.change, RunChange::Ended(RunOutcome::Failed));
    harness.advance(60);
    let bound = harness
        .command(json!({
            "kind": "bind",
            "key": key,
            "token": prepared.token,
            "prompt": "signedout",
            "titleSource": "prompt"
        }))
        .await
        .unwrap();
    assert_launch_failed(&bound.view, key, NOW + 100);
    assert!(
        !harness.host.run_tracked(key),
        "no run stays open in the tap"
    );
    harness.host.apply_run(started).await;
    harness.host.apply_run(ended).await;
    settle_tasks(&harness.host).await;
    let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
    assert_launch_failed(&view, key, NOW + 100);
    assert!(
        harness
            .git
            .called(&format!("capture {}", turn_ref(key, 0, true)))
    );

    // The worker seeded the session from the run's start, but the end
    // reaches it only after the bind.
    let key = "claude#seeded";
    harness.sessions.upsert(row(key, SessionKind::Chat));
    harness.clock.store(NOW, Ordering::SeqCst);
    signed_out_launch(&harness, key, false);
    let started = ops.try_recv().unwrap();
    let ended = ops.try_recv().unwrap();
    harness.host.apply_run(started).await;
    harness.advance(60);
    let bound = harness
        .command(json!({"kind": "bind", "key": key, "prompt": "signedout"}))
        .await
        .unwrap();
    assert_launch_failed(&bound.view, key, NOW + 40);
    harness.host.apply_run(ended).await;
    let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
    assert_launch_failed(&view, key, NOW + 40);
    assert!(ops.try_recv().is_err());
}

/// The projection Away hands the thread host for one loop.
fn away_projection(state: interface_contracts::away::State) -> ThreadAway {
    use interface_contracts::away::{Role, RoundState, ThreadAwayRound, Verdict};
    ThreadAway {
        run_id: "run-1".into(),
        state,
        detail: "Fresh review · iteration 1/6".into(),
        provider: "codex".into(),
        model: None,
        reviewer: Some("claude".into()),
        objective: "Fix the login test".into(),
        started_at: NOW,
        deadline_at: NOW + 3_600_000,
        max_iterations: 6,
        rounds: vec![ThreadAwayRound {
            iteration: 1,
            role: Role::Work,
            state: RoundState::Completed,
            turn: Some(0),
            session_key: None,
            verdict: Some(Verdict::Continue),
            summary: Some("Fixed the race.".into()),
            checks: vec!["npm test: 14 passed".into()],
        }],
    }
}

/// Prepares an Away worktree of a project the thread's folder grant refuses
/// (Away vetted it against the host's own roots) and binds `key` to it.
async fn away_thread(harness: &Harness, key: &str) -> String {
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-away");
    lock(&harness.authority.0).insert(root.clone());
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: PathBuf::from(&worktree),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: Some(PathBuf::from(&worktree)),
        branch: Some("iface/away-fix-the-login-1a2b3c".into()),
        base_ref: Some("main".into()),
        created: true,
    });
    let prepared = harness
        .host
        .away_prepare(&root, "away Fix the login test")
        .await
        .unwrap();
    assert_eq!(prepared.workspace.mode, WorkspaceMode::Worktree);
    assert!(prepared.workspace.owned);
    harness.sessions.upsert(row(key, SessionKind::Code));
    harness.host.away_bind(key, &prepared.token).await.unwrap();
    assert!(
        harness
            .git
            .called(&format!("delete {}", pending_baseline_ref(&prepared.token)))
    );
    worktree
}

/// Review B3 (14 Oct 2026): an Away loop runs as a thread in its own
/// worktree. Its empty work session binds with no turn; while the loop holds
/// the thread only the prompt it permitted gets through (the turn it opened
/// is handed back once), and the owner's prompts and the commands that act on
/// its session or files refuse, definitely unsent. When the loop ends the
/// thread is the owner's again.
#[tokio::test]
async fn an_away_loop_holds_its_thread_and_takes_only_its_own_prompts() {
    let harness = harness();
    let key = "codex#away";
    let worktree = away_thread(&harness, key).await;
    let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
    let bound = thread(&view, key);
    assert_eq!(bound.section, ThreadSection::Active);
    assert_eq!(
        bound.workspace.as_ref().unwrap().worktree_path.as_deref(),
        Some(worktree.as_str())
    );
    assert!(bound.turns.is_empty() && bound.last_run.is_none());
    assert_eq!(bound.title_source, Some(TitleSource::User));

    let refused = harness
        .host
        .before_send(key, "Also fix the docs")
        .await
        .err()
        .unwrap();
    assert_eq!(
        (refused.code.as_str(), refused.replay_safe),
        (THREADS_AWAY_RUNNING_CODE, true)
    );
    harness.host.away_permit(key, "Away work round 1");
    let ticket = harness
        .host
        .before_send(key, "Away work round 1\n")
        .await
        .unwrap()
        .expect("the loop's turn");
    assert!(ticket.echoes_prompt(), "no window echoes the loop's prompt");
    ticket.settle(SendSettlement::Delivered);
    assert_eq!(harness.host.away_sent_turn(key), Some(0));
    assert_eq!(harness.host.away_sent_turn(key), None);
    let record = harness.document().await.record(key).unwrap().clone();
    assert_eq!(
        record.turns[0].prompt_digest,
        prompt_digest("Away work round 1")
    );
    assert!(
        harness
            .git
            .called(&format!("capture {}", turn_ref(key, 0, false)))
    );
    // The permit is spent: the same words again are the owner's.
    let again = harness.host.before_send(key, "Away work round 1").await;
    assert_eq!(again.err().unwrap().code, THREADS_AWAY_RUNNING_CODE);
    for command in [
        json!({"kind": "restoreTurn", "key": key, "ordinal": 0}),
        json!({"kind": "openInTerminal", "key": key}),
        json!({"kind": "editFrom", "key": key, "ordinal": 0, "restoreFiles": false}),
    ] {
        let refused = harness.command(command).await.unwrap_err();
        assert_eq!(refused.code, THREADS_AWAY_RUNNING_CODE);
    }

    // The loop ended: the owner's prompts go through again.
    harness.host.away_release(key);
    let ticket = harness
        .host
        .before_send(key, "Also fix the docs")
        .await
        .unwrap()
        .expect("the owner's turn");
    assert!(!ticket.echoes_prompt(), "the window echoes its own prompt");
    ticket.settle(SendSettlement::Delivered);
    assert_eq!(harness.document().await.record(key).unwrap().turns.len(), 2);
}

/// The loop's projection joins its thread's view, pushed once per change.
#[tokio::test]
async fn an_away_loops_rounds_join_its_threads_view() {
    let mut harness = harness();
    let key = "codex#away";
    away_thread(&harness, key).await;
    harness.pushed();
    let away = away_projection(interface_contracts::away::State::Running);
    harness
        .host
        .set_away_threads(vec![(key.to_owned(), away.clone())])
        .await;
    let pushed = harness.pushed();
    assert_eq!(pushed.len(), 1, "a changed projection pushes once");
    assert_eq!(thread(&pushed[0], key).away.as_ref(), Some(&away));
    let read = harness.command(json!({"kind": "read"})).await.unwrap().view;
    assert_eq!(thread(&read, key).away.as_ref(), Some(&away));
    harness
        .host
        .set_away_threads(vec![(key.to_owned(), away)])
        .await;
    assert!(
        harness.pushed().is_empty(),
        "an unchanged one pushes nothing"
    );
    harness.host.set_away_threads(Vec::new()).await;
    assert!(thread(&harness.pushed()[0], key).away.is_none());
}

#[tokio::test]
async fn prepare_workspace_needs_folder_authority_and_a_known_mode() {
    let mut harness = harness();
    let root = harness.dir("secret");
    lock(&harness.authority.0).insert(root.clone());
    let prepare = |mode: &str| json!({"kind": "prepareWorkspace", "projectRoot": root, "mode": mode, "hint": "x"});
    let refused = harness.command(prepare("local")).await.unwrap_err();
    assert_eq!(refused.code, THREADS_FOLDER_REFUSED_CODE);
    assert!(refused.replay_safe);
    assert!(harness.git.calls().is_empty(), "git never ran");
    assert_eq!(
        harness.command(prepare("copy")).await.unwrap_err().code,
        THREADS_INVALID_CODE
    );
    assert_eq!(
        harness.command(prepare("existing")).await.unwrap_err().code,
        THREADS_INVALID_CODE,
        "an existing worktree needs its path"
    );
    lock(&harness.authority.0).clear();
    // Git refusing a new worktree may have left something behind; a local
    // prepare cannot have.
    for (mode, replay_safe) in [("worktree", false), ("local", true)] {
        let failed = harness.command(prepare(mode)).await.unwrap_err();
        assert_eq!(failed.code, THREADS_WORKSPACE_FAILED_CODE);
        assert_eq!(failed.replay_safe, replay_safe, "{mode}");
    }
    assert!(harness.pushed().is_empty());
}

#[tokio::test]
async fn saved_setup_failure_never_issues_a_launch_token_and_existing_worktree_stays_gated() {
    let harness = harness();
    let root = harness.dir("project");
    let cwd = std::fs::canonicalize(harness.dir("worktree")).unwrap();
    let cwd_text = cwd.to_string_lossy().into_owned();
    harness.command(json!({"kind":"workflow", "command": {"action":"saveSetup", "projectRoot":root,
        "setup":{"prerequisites":["printf missing; exit 4"],"commands":["touch should-not-run"],"timeoutSeconds":2}}})).await.unwrap();
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: cwd.clone(),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: Some(cwd.clone()),
        branch: Some("feature".into()),
        base_ref: Some("main".into()),
        created: true,
    });
    let response = harness
        .command(
            json!({"kind":"prepareWorkspace","projectRoot":root,"mode":"worktree","hint":"test"}),
        )
        .await
        .unwrap();
    assert!(response.prepared.is_none());
    assert_eq!(
        response.setup.unwrap().state,
        interface_contracts::workflows::SetupState::Failed
    );
    assert!(harness.document().await.prepared.is_empty());
    assert!(!cwd.join("should-not-run").exists());
    lock(&harness.git.prepared).as_mut().unwrap().created = false;
    let response = harness.command(json!({"kind":"prepareWorkspace","projectRoot":root,"mode":"existing","worktreePath":cwd_text,"hint":"test"})).await.unwrap();
    assert!(
        response.prepared.is_none(),
        "selecting the failed worktree cannot bypass readiness"
    );
    harness
        .command(
            json!({"kind":"workflow", "command": {"action":"saveSetup", "projectRoot":root,
        "setup":{"prerequisites":[],"commands":["printf ready > ready"],"timeoutSeconds":2}}}),
        )
        .await
        .unwrap();
    let retry = harness.command(json!({"kind":"workflow", "command": {"action":"retrySetup", "projectRoot":root,"cwd":cwd_text}})).await.unwrap();
    assert_eq!(
        retry.setup.unwrap().state,
        interface_contracts::workflows::SetupState::Ready
    );
    assert_eq!(std::fs::read_to_string(cwd.join("ready")).unwrap(), "ready");
    let ready = harness.command(json!({"kind":"prepareWorkspace","projectRoot":root,"mode":"existing","worktreePath":cwd_text,"hint":"test"})).await.unwrap();
    assert!(ready.prepared.is_some());
}

#[tokio::test]
async fn recreated_worktree_at_a_previously_ready_path_runs_saved_setup_again() {
    let harness = harness();
    let root = harness.dir("project");
    let cwd = std::fs::canonicalize(harness.dir("worktree")).unwrap();
    harness.command(json!({"kind":"workflow", "command": {"action":"saveSetup", "projectRoot":root,
        "setup":{"prerequisites":[],"commands":["printf ready > setup-marker"],"timeoutSeconds":2}}})).await.unwrap();
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: cwd.clone(),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: Some(cwd.clone()),
        branch: Some("feature".into()),
        base_ref: Some("main".into()),
        created: true,
    });
    let prepare =
        json!({"kind":"prepareWorkspace", "projectRoot":root, "mode":"worktree", "hint":"test"});
    let first = harness.command(prepare.clone()).await.unwrap();
    let first_token = first.prepared.unwrap().token;
    assert_eq!(
        std::fs::read_to_string(cwd.join("setup-marker")).unwrap(),
        "ready"
    );
    harness
        .command(json!({"kind":"discardWorkspace", "token":first_token}))
        .await
        .unwrap();
    // The Git fixture reports the next directory as newly created at the same path.
    // Replace its actual files too, so an old Ready report cannot mask skipped setup.
    std::fs::remove_dir_all(&cwd).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    let recreated = harness.command(prepare).await.unwrap();
    assert!(recreated.prepared.is_some());
    assert_eq!(
        recreated.setup.unwrap().state,
        interface_contracts::workflows::SetupState::Ready
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("setup-marker")).unwrap(),
        "ready"
    );
    let retry = harness
        .command(json!({"kind":"workflow", "command": {
        "action":"retrySetup", "projectRoot":root, "cwd":cwd}}))
        .await;
    assert!(
        retry.is_err(),
        "A ready directory still cannot be explicitly retried"
    );
}

#[tokio::test]
async fn selecting_a_running_setup_never_rewrites_its_durable_report() {
    use interface_contracts::workflows::{SetupReport, SetupState};
    use std::os::unix::fs::MetadataExt;
    let harness = harness();
    let root = harness.dir("project");
    let cwd = std::fs::canonicalize(harness.dir("worktree")).unwrap();
    let saved_root = root.clone();
    let saved_cwd = cwd.to_string_lossy().into_owned();
    harness
        .host
        .with_store(move |store, _| {
            store.workflows()?.finish_setup(
                &saved_root,
                SetupReport {
                    cwd: saved_cwd,
                    state: SetupState::Running,
                    steps: vec![],
                    message: "Setup is running".into(),
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: cwd.clone(),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: Some(cwd.clone()),
        branch: Some("feature".into()),
        base_ref: Some("main".into()),
        created: false,
    });
    let file = harness
        .state
        .path()
        .join(interface_settings::workflows::WORKFLOWS_FILE);
    let before = std::fs::metadata(&file).unwrap().ino();
    let response = harness.command(json!({"kind":"prepareWorkspace", "projectRoot":root, "mode":"existing", "worktreePath":cwd, "hint":"test"})).await.unwrap();
    assert!(response.prepared.is_none());
    assert_eq!(response.setup.unwrap().state, SetupState::Running);
    assert_eq!(
        std::fs::metadata(&file).unwrap().ino(),
        before,
        "Selecting an existing worktree must not replace the report; a concurrent setup completion owns that write"
    );
}

#[tokio::test]
async fn setup_configuration_does_not_run_in_local_checkout_or_unrecorded_retry_path() {
    let harness = harness();
    let root = harness.dir("project");
    harness
        .command(
            json!({"kind":"workflow", "command": {"action":"saveSetup", "projectRoot":root,
        "setup":{"prerequisites":[],"commands":["exit 8"],"timeoutSeconds":2}}}),
        )
        .await
        .unwrap();
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: PathBuf::from(&root),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: None,
        branch: Some("main".into()),
        base_ref: None,
        created: false,
    });
    let local = harness
        .command(json!({"kind":"prepareWorkspace","projectRoot":root,"mode":"local","hint":"test"}))
        .await
        .unwrap();
    assert!(local.prepared.is_some());
    assert!(local.setup.is_none());
    let refused = harness.command(json!({"kind":"workflow", "command": {"action":"retrySetup", "projectRoot":root,"cwd":"/not-authorized"}})).await.unwrap_err();
    assert_eq!(refused.code, "threads-setup-unknown");
}

#[tokio::test]
async fn send_hook_records_a_turn_with_the_prompt_digest_and_its_start_checkpoint() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let key = "codex#t";
    harness.git_thread(key, &root, None, "first").await;
    harness.pushed();

    let ticket = harness
        .host
        .before_send(key, "  Now add a test\r\n")
        .await
        .unwrap()
        .expect("a thread send");
    let start = turn_ref(key, 1, false);
    assert!(harness.git.called(&format!("capture {start}")));
    let record = harness.document().await.record(key).unwrap().clone();
    let turn = &record.turns[1];
    assert_eq!(turn.ordinal, 1);
    assert_eq!(turn.prompt_digest, prompt_digest("Now add a test"));
    assert_eq!(turn.start_ref.as_deref(), Some(start.as_str()));
    assert_eq!(turn.state, TurnState::Running);
    assert_eq!(record.last_run.as_ref().unwrap().started_at, NOW);
    assert!(harness.host.run_tracked(key));
    assert_eq!(
        harness.pushed().len(),
        1,
        "the turn is pushed before the send"
    );

    // The send was never delivered: the turn is taken back.
    ticket.settle(SendSettlement::Unsent);
    settle_tasks(&harness.host).await;
    let record = harness.document().await.record(key).unwrap().clone();
    assert_eq!(record.turns.len(), 1);
    assert!(!harness.host.run_tracked(key));

    // Delivered (or unknown): it stays. A skipped capture never blocks.
    harness.git.capture_skips.store(true, Ordering::SeqCst);
    harness
        .host
        .before_send(key, "again")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    settle_tasks(&harness.host).await;
    let record = harness.document().await.record(key).unwrap().clone();
    assert_eq!(record.turns.len(), 2);
    assert!(record.turns[1].start_ref.is_none());

    // Sessions without a thread pass straight through.
    assert!(
        harness
            .host
            .before_send("claude#other", "x")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn send_hook_refuses_definitely_unsent_when_a_missing_worktree_cannot_be_recreated() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.missing_dir("atlas-wt");
    let key = "codex#wt";
    let response = harness
        .git_thread(key, &root, Some(&worktree), "first")
        .await;
    let workspace = thread(&response.view, key).workspace.clone().unwrap();
    assert_eq!(workspace.mode, WorkspaceMode::Worktree);
    assert!(workspace.owned);
    assert_eq!(workspace.missing, Some(true));

    harness.git.ensure_fails.store(true, Ordering::SeqCst);
    let Err(refused) = harness.host.before_send(key, "go on").await else {
        panic!("a missing worktree must refuse the send");
    };
    assert_eq!(refused.code, THREADS_WORKTREE_UNAVAILABLE_CODE);
    assert!(refused.replay_safe && !refused.retryable);
    assert!(refused.message.contains("Nothing was sent"));
    assert!(harness.git.called(&format!("ensure {worktree}")));
    assert!(
        !harness
            .git
            .called(&format!("capture {}", turn_ref(key, 1, false)))
    );
    assert_eq!(harness.document().await.record(key).unwrap().turns.len(), 1);

    harness.git.ensure_fails.store(false, Ordering::SeqCst);
    harness
        .host
        .before_send(key, "go on")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    assert!(Path::new(&worktree).is_dir(), "recreated from its branch");
    let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
    let recreated = thread(&view, key);
    assert_eq!(recreated.workspace.as_ref().unwrap().missing, Some(false));
    assert!(recreated.turns[1].has_start);
}

/// 15 Nov 2026: every Codex turn ends with an inactive thinking signal after
/// its idle (the provider replay found it). Thinking that stops opens no run;
/// inside an open run it still counts as the run's activity, so a run whose
/// only sign of work was thinking ends at its idle (the case it served since
/// Threads S1), and thinking that starts still opens one.
#[test]
fn thinking_that_stops_opens_no_run_but_still_counts_inside_one() {
    let working = EngineEventKind::StatusChanged {
        status: SessionStatus::Working,
    };
    let idle = EngineEventKind::StatusChanged {
        status: SessionStatus::Idle,
    };
    let thinking = |active| EngineEventKind::ThinkingSignal {
        active,
        agent_key: None,
    };
    let delta = EngineEventKind::MessageDelta {
        text: "ready".into(),
        agent_key: None,
    };

    // A finished turn's trailing signal opens nothing, and the session
    // stays finished through its next idle.
    let mut track = RunTrack::default();
    assert_eq!(track.observe(&working), Some(RunChange::Started));
    assert_eq!(track.observe(&delta), None);
    assert_eq!(
        track.observe(&idle),
        Some(RunChange::Ended(RunOutcome::Completed))
    );
    assert_eq!(track.observe(&thinking(false)), None);
    assert!(!track.is_open());
    assert_eq!(track.observe(&idle), None);

    // Inside a run, thinking that stops is the run's activity.
    let mut track = RunTrack::default();
    assert_eq!(track.observe(&working), Some(RunChange::Started));
    assert_eq!(track.observe(&thinking(false)), None);
    assert_eq!(
        track.observe(&idle),
        Some(RunChange::Ended(RunOutcome::Completed))
    );

    // Thinking that starts opens a run (a provider turn Interface did not send).
    let mut track = RunTrack::default();
    assert_eq!(track.observe(&thinking(true)), Some(RunChange::Started));
    assert_eq!(
        track.observe(&idle),
        Some(RunChange::Ended(RunOutcome::Completed))
    );
}

#[test]
fn the_run_tap_needs_activity_or_a_stop_before_idle_ends_a_run() {
    let working = EngineEventKind::StatusChanged {
        status: SessionStatus::Working,
    };
    let idle = EngineEventKind::StatusChanged {
        status: SessionStatus::Idle,
    };
    let delta = EngineEventKind::MessageDelta {
        text: "hi".into(),
        agent_key: None,
    };
    let mut track = RunTrack::default();
    assert_eq!(track.observe(&idle), None, "no run to end");
    assert_eq!(track.observe(&working), Some(RunChange::Started));
    assert_eq!(track.observe(&working), None);
    assert_eq!(track.observe(&idle), None, "a resumed session's first idle");
    assert_eq!(track.observe(&delta), None);
    assert_eq!(
        track.observe(&idle),
        Some(RunChange::Ended(RunOutcome::Completed))
    );
    assert!(!track.is_open());
    assert_eq!(track.observe(&delta), Some(RunChange::Started));
    assert_eq!(
        track.observe(&EngineEventKind::Error {
            cause: None,
            message: "boom".into(),
            retryable: false
        }),
        Some(RunChange::Ended(RunOutcome::Failed))
    );
    let mut track = RunTrack::opened(1);
    track.interrupt = true;
    assert_eq!(
        track.observe(&idle),
        Some(RunChange::Ended(RunOutcome::Interrupted))
    );
    let mut track = RunTrack::opened(1);
    assert_eq!(
        track.observe(&EngineEventKind::SessionClosed),
        Some(RunChange::Ended(RunOutcome::Interrupted))
    );
}

#[tokio::test]
async fn run_tap_stamps_run_start_and_end_and_records_the_end_checkpoint() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let key = "codex#run";
    harness.git_thread(key, &root, None, "first").await;
    harness.host.open_run(key);
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    let working = EngineEventKind::StatusChanged {
        status: SessionStatus::Working,
    };
    let idle = EngineEventKind::StatusChanged {
        status: SessionStatus::Idle,
    };
    harness.host.observe_run(key, &working);
    harness.host.observe_run(key, &idle);
    assert!(
        ops.try_recv().is_err(),
        "a run the launch opened, not over yet"
    );
    harness.host.observe_run(
        key,
        &EngineEventKind::ToolCall {
            call_id: "1".into(),
            tool: "Edit".into(),
            summary: "edit".into(),
            input: Value::Null,
            agent_key: None,
        },
    );
    harness.advance(5_000);
    harness.host.observe_run(key, &idle);
    let op = ops.try_recv().unwrap();
    assert_eq!(op.change, RunChange::Ended(RunOutcome::Completed));
    harness.pushed();
    harness.host.apply_run(op).await;
    settle_tasks(&harness.host).await;
    let end = turn_ref(key, 0, true);
    assert!(harness.git.called(&format!("capture {end}")));
    let start = harness.document().await.record(key).unwrap().turns[0]
        .start_ref
        .clone()
        .unwrap();
    assert!(harness.git.called(&format!("numstat {start} {end}")));
    let pushed = harness.pushed();
    assert_eq!(pushed.len(), 2, "the run end, then its checkpoint");
    let latest = thread(pushed.last().unwrap(), key);
    let run = latest.last_run.as_ref().unwrap();
    assert_eq!(
        (run.ended_at, run.outcome),
        (Some(NOW + 5_000), Some(RunOutcome::Completed))
    );
    assert!(latest.unread);
    let turn = &latest.turns[0];
    assert_eq!(turn.state, TurnState::Done);
    assert!(turn.has_end);
    assert_eq!(
        (turn.files_changed, turn.added, turn.removed),
        (Some(2), Some(7), Some(1))
    );

    // A stop the owner asked for ends the next run interrupted, never unread.
    harness
        .command(json!({"kind": "visit", "key": key}))
        .await
        .unwrap();
    harness.host.observe_run(key, &working);
    let op = ops.try_recv().unwrap();
    assert_eq!(op.change, RunChange::Started);
    harness.host.apply_run(op).await;
    harness.host.interrupt_requested(key);
    harness.host.observe_run(key, &idle);
    let op = ops.try_recv().unwrap();
    harness.host.apply_run(op).await;
    let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
    let stopped = thread(&view, key);
    assert_eq!(
        stopped.last_run.as_ref().unwrap().outcome,
        Some(RunOutcome::Interrupted)
    );
    assert!(!stopped.unread);

    // An agent session never becomes a thread through the tap.
    harness
        .sessions
        .upsert(row("claude#agent", SessionKind::Agent));
    harness.host.observe_run("claude#agent", &working);
    let op = ops.try_recv().unwrap();
    harness.host.apply_run(op).await;
    assert!(harness.document().await.record("claude#agent").is_none());
}

/// 7 Oct 2026: two Local threads ran at once in one checkout, and one turn's
/// card read "2 files changed +2 −8" — the other thread's README line
/// included. In a shared checkout a turn counts only the files its own edits
/// named, and nothing when the tap does not know them.
#[tokio::test]
async fn a_turn_in_a_shared_checkout_counts_only_the_files_its_own_edits_named() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let (mine, theirs) = ("claude#three", "claude#two");
    harness
        .git_thread(theirs, &root, None, "append a line")
        .await;
    harness
        .git_thread(mine, &root, None, "rewrite the session")
        .await;
    harness.host.open_run(mine);
    harness.host.open_run(theirs);
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    let idle = EngineEventKind::StatusChanged {
        status: SessionStatus::Idle,
    };
    let tool = |tool: &str, field: &str, path: &str| EngineEventKind::ToolCall {
        call_id: format!("{tool}-{path}"),
        tool: tool.into(),
        summary: tool.into(),
        input: json!({ field: path }),
        agent_key: None,
    };
    let diff = |path: &str| EngineEventKind::DiffProduced {
        call_id: "write".into(),
        path: path.into(),
        before: String::new(),
        after: "export {};\n".into(),
        agent_key: None,
    };
    let session = format!("{root}/src/lib/session.ts");
    let notebook = format!("{root}/notes.ipynb");
    // Theirs appends to the README while mine writes the session file, a
    // multi-edit and a notebook edit (neither of which produces a diff).
    harness
        .host
        .observe_run(theirs, &diff(&format!("{root}/README.md")));
    harness
        .host
        .observe_run(mine, &tool("Write", "file_path", &session));
    harness.host.observe_run(mine, &diff(&session));
    harness
        .host
        .observe_run(mine, &tool("MultiEdit", "file_path", "src/a.ts"));
    harness
        .host
        .observe_run(mine, &tool("NotebookEdit", "notebook_path", &notebook));
    harness
        .host
        .observe_run(mine, &tool("Bash", "command", "echo >> README.md"));
    harness.host.observe_run(mine, &idle);
    let op = ops.try_recv().unwrap();
    let touched = BTreeSet::from([notebook.clone(), session.clone(), "src/a.ts".to_owned()]);
    assert_eq!(op.touched.as_ref(), Some(&touched));
    assert_eq!(harness.host.ended_run_paths(mine).as_ref(), Some(&touched));
    harness.host.apply_run(op).await;
    settle_tasks(&harness.host).await;
    let document = harness.document().await;
    let turn = &document.record(mine).unwrap().turns[0];
    let (start, end) = (turn.start_ref.clone().unwrap(), turn_ref(mine, 0, true));
    assert!(harness.git.called(&format!(
        "numstat-paths {start} {end} {notebook},{session},src/a.ts"
    )));
    assert!(
        !harness
            .git
            .calls()
            .iter()
            .any(|call| call.starts_with("numstat ")),
        "never the whole checkout's count"
    );
    assert_eq!(turn.end_ref.as_deref(), Some(end.as_str()));
    assert_eq!(
        (turn.files_changed, turn.added, turn.removed),
        (Some(1), Some(1), Some(8))
    );

    // A run whose start the tap never saw (a host that came up mid-run)
    // names no files it can vouch for: its checkpoint stays, its counts stay
    // unknown rather than take mine.
    lock(&harness.host.inner.run_paths).remove(theirs);
    harness.host.observe_run(theirs, &idle);
    let op = ops.try_recv().unwrap();
    assert_eq!(op.touched, None);
    harness.host.apply_run(op).await;
    settle_tasks(&harness.host).await;
    let end = turn_ref(theirs, 0, true);
    assert!(harness.git.called(&format!("capture {end}")));
    assert_eq!(
        harness
            .git
            .calls()
            .iter()
            .filter(|call| call.starts_with("numstat"))
            .count(),
        1,
        "nothing more is counted"
    );
    let document = harness.document().await;
    let turn = &document.record(theirs).unwrap().turns[0];
    assert_eq!(turn.end_ref.as_deref(), Some(end.as_str()));
    assert_eq!(
        (turn.files_changed, turn.added, turn.removed),
        (None, None, None)
    );
}

/// Owner decision 14 Oct 2026 (B2): a turn keeps when it ended and the files
/// its own edits named, on disk, so "Worked for…" survives a restart and a
/// turn's Review diff in a shared Local checkout shows only that turn's
/// files — and refuses, rather than show someone else's, when they are
/// unknown. A turn alone in its checkout still diffs all of it.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One checkout: a known turn, an unknown one, then one alone.
async fn a_shared_checkout_turn_keeps_its_end_and_files_and_its_review_diff_covers_only_them() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let (mine, theirs) = ("claude#mine", "claude#theirs");
    harness
        .git_thread(theirs, &root, None, "append a line")
        .await;
    harness
        .git_thread(mine, &root, None, "rewrite the session")
        .await;
    harness.host.open_run(mine);
    harness.host.open_run(theirs);
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    let idle = EngineEventKind::StatusChanged {
        status: SessionStatus::Idle,
    };
    let diff = |path: &str| EngineEventKind::DiffProduced {
        call_id: "write".into(),
        path: path.into(),
        before: String::new(),
        after: "export {};\n".into(),
        agent_key: None,
    };
    harness
        .host
        .observe_run(mine, &diff(&format!("{root}/src/lib/session.ts")));
    harness.host.observe_run(mine, &diff("src/a.ts"));
    harness.advance(41_000);
    harness.host.observe_run(mine, &idle);
    harness.host.apply_run(ops.try_recv().unwrap()).await;
    settle_tasks(&harness.host).await;

    let document = harness.document().await;
    let turn = &document.record(mine).unwrap().turns[0];
    assert_eq!(turn.ended_at, Some(NOW + 41_000));
    assert_eq!(
        turn.touched.as_deref(),
        Some(&["src/a.ts".to_owned(), "src/lib/session.ts".to_owned()][..])
    );
    assert!(turn.shared);
    let (start, end) = (turn.start_ref.clone().unwrap(), turn_ref(mine, 0, true));
    let on_disk: Value = serde_json::from_slice(
        &std::fs::read(harness.state.path().join(THREADS_FILE_NAME)).unwrap(),
    )
    .unwrap();
    let stored = on_disk["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["key"] == mine)
        .unwrap();
    assert_eq!(stored["turns"][0]["endedAt"], json!(NOW + 41_000));
    assert_eq!(
        stored["turns"][0]["touched"],
        json!(["src/a.ts", "src/lib/session.ts"])
    );
    let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
    assert_eq!(thread(&view, mine).turns[0].ended_at, Some(NOW + 41_000));

    let scoped = format!("diff {start} {end} only src/a.ts,src/lib/session.ts");
    harness
        .command(json!({"kind": "turnDiff", "key": mine, "ordinal": 0}))
        .await
        .unwrap();
    assert!(harness.git.called(&scoped), "{:?}", harness.git.calls());
    lock(&harness.git.calls).clear();
    harness
        .command(json!({"kind": "threadDiff", "key": mine, "scope": "turn"}))
        .await
        .unwrap();
    assert!(harness.git.called(&scoped), "the latest turn's diff too");

    // Theirs ended without a start the tap saw: its files are unknown, so
    // its own changes cannot be told from mine.
    lock(&harness.host.inner.run_paths).remove(theirs);
    harness.host.observe_run(
        theirs,
        &EngineEventKind::ToolCall {
            call_id: "sh".into(),
            tool: "Bash".into(),
            summary: "echo >> README.md".into(),
            input: json!({"command": "echo >> README.md"}),
            agent_key: None,
        },
    );
    harness.host.observe_run(theirs, &idle);
    harness.host.apply_run(ops.try_recv().unwrap()).await;
    settle_tasks(&harness.host).await;
    lock(&harness.git.calls).clear();
    let refused = harness
        .command(json!({"kind": "turnDiff", "key": theirs, "ordinal": 0}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, THREADS_TURN_FILES_UNKNOWN_CODE);
    assert!(refused.replay_safe && !refused.retryable);
    assert!(
        !harness
            .git
            .calls()
            .iter()
            .any(|call| call.starts_with("diff")),
        "no diff was read"
    );

    // Alone in its checkout (the other thread settled), a turn's diff is
    // all of it: shell commands' edits included.
    harness
        .command(json!({"kind": "settle", "key": theirs}))
        .await
        .unwrap();
    harness
        .command(json!({"kind": "settle", "key": mine}))
        .await
        .unwrap();
    let solo = "claude#solo";
    harness.git_thread(solo, &root, None, "alone").await;
    harness.host.open_run(solo);
    harness.host.observe_run(solo, &diff("src/a.ts"));
    harness.host.observe_run(solo, &idle);
    harness.host.apply_run(ops.try_recv().unwrap()).await;
    settle_tasks(&harness.host).await;
    let document = harness.document().await;
    let turn = &document.record(solo).unwrap().turns[0];
    assert!(!turn.shared);
    assert_eq!(turn.touched.as_deref(), Some(&["src/a.ts".to_owned()][..]));
    let (start, end) = (turn.start_ref.clone().unwrap(), turn_ref(solo, 0, true));
    harness
        .command(json!({"kind": "turnDiff", "key": solo, "ordinal": 0}))
        .await
        .unwrap();
    assert!(harness.git.called(&format!("diff {start} {end}")));
}

/// A run whose edits named more files than the tap keeps names none it can
/// vouch for; the next run starts knowing again.
#[tokio::test]
async fn a_run_past_the_tap_path_bound_names_no_files() {
    let harness = harness();
    let key = "codex#wide";
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    let diff = |index: usize| EngineEventKind::DiffProduced {
        call_id: "patch".into(),
        path: format!("src/{index}.rs"),
        before: String::new(),
        after: "x".into(),
        agent_key: None,
    };
    let idle = EngineEventKind::StatusChanged {
        status: SessionStatus::Idle,
    };
    harness.host.open_run(key);
    for index in 0..=1_000 {
        harness.host.observe_run(key, &diff(index));
    }
    harness.host.observe_run(key, &idle);
    assert_eq!(ops.try_recv().unwrap().touched, None);
    assert_eq!(harness.host.ended_run_paths(key), None);
    harness.host.observe_run(key, &diff(7));
    assert_eq!(ops.try_recv().unwrap().change, RunChange::Started);
    harness.host.observe_run(key, &idle);
    assert_eq!(
        ops.try_recv().unwrap().touched,
        Some(BTreeSet::from(["src/7.rs".to_owned()]))
    );
}

#[tokio::test]
async fn auto_settle_skips_working_waiting_and_running_threads() {
    let mut harness = harness();
    for (key, status) in [
        ("codex#idle", SessionStatus::Idle),
        ("codex#working", SessionStatus::Working),
        ("codex#waiting", SessionStatus::Waiting),
        ("codex#tracked", SessionStatus::Idle),
    ] {
        let mut summary = row(key, SessionKind::Code);
        summary.first_seen = iso(NOW - 10 * DAY);
        summary.status = status;
        harness.sessions.upsert(summary);
    }
    harness.host.reconcile().await;
    harness.host.open_run("codex#tracked");
    harness.pushed();
    harness.host.auto_settle().await;
    let view = harness.pushed().pop().unwrap();
    assert_eq!(
        thread(&view, "codex#idle").settled_reason,
        Some(SettledReason::Idle)
    );
    for key in ["codex#working", "codex#waiting", "codex#tracked"] {
        assert_eq!(thread(&view, key).section, ThreadSection::Active, "{key}");
    }
    harness
        .sessions
        .edit("codex#waiting", |row| row.status = SessionStatus::Idle);
    harness.host.auto_settle().await;
    let view = harness.pushed().pop().unwrap();
    assert_eq!(
        thread(&view, "codex#waiting").section,
        ThreadSection::Settled
    );
}

#[tokio::test]
async fn a_snooze_running_out_bumps_the_revision_and_pushes_the_woken_thread() {
    let mut harness = harness();
    harness.sessions.upsert(row("claude#a", SessionKind::Chat));
    harness.host.reconcile().await;
    let snoozed = harness
        .command(json!({"kind": "snooze", "key": "claude#a", "until": NOW + HOUR}))
        .await
        .unwrap()
        .view;
    harness.pushed();
    harness.advance(2 * HOUR);
    harness.host.wake_snoozed().await;
    let woke = harness.pushed();
    assert_eq!(woke.len(), 1);
    assert_eq!(woke[0].revision, snoozed.revision + 1);
    let awake = thread(&woke[0], "claude#a");
    assert_eq!(awake.section, ThreadSection::Active);
    assert!(awake.woke);
}

#[tokio::test]
async fn restore_turn_refuses_shared_unowned_busy_and_uncheckpointed_turns() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    let key = "codex#owner";
    harness.git.moves.store(true, Ordering::SeqCst);
    harness
        .git_thread(key, &root, Some(&worktree), "first")
        .await;
    let local_root = harness.dir("local");
    harness
        .git_thread("codex#local", &local_root, None, "x")
        .await;

    let mut other = row("claude#other", SessionKind::Code);
    other.cwd = format!("{worktree}/src");
    harness.sessions.upsert(other);
    lock(&harness.sessions.live).insert("claude#other".into());
    let restore =
        |key: &str, ordinal: u32| json!({"kind": "restoreTurn", "key": key, "ordinal": ordinal});
    let shared = harness.command(restore(key, 0)).await.unwrap_err();
    assert_eq!(shared.code, THREADS_SHARED_CODE);
    assert!(shared.replay_safe);
    lock(&harness.sessions.live).clear();
    assert_eq!(
        harness
            .command(restore("codex#local", 0))
            .await
            .unwrap_err()
            .code,
        THREADS_NOT_OWNED_CODE
    );
    assert_eq!(
        harness.command(restore(key, 9)).await.unwrap_err().code,
        THREADS_NO_CHECKPOINT_CODE
    );
    harness
        .sessions
        .edit(key, |row| row.status = SessionStatus::Working);
    assert_eq!(
        harness.command(restore(key, 0)).await.unwrap_err().code,
        THREADS_BUSY_CODE
    );
    harness
        .sessions
        .edit(key, |row| row.status = SessionStatus::Idle);
    assert!(
        !harness
            .git
            .calls()
            .iter()
            .any(|call| call.starts_with("restore")),
        "nothing restored while refused"
    );

    let restored = harness.command(restore(key, 0)).await.unwrap();
    let result = restored.restore.unwrap();
    assert_eq!(result.restored, 3);
    let recovery = format!("{}/recovery/{NOW}", thread_ref_namespace(key));
    assert_eq!(result.recovery_ref, recovery);
    assert!(
        harness
            .git
            .called(&format!("restore {} {recovery}", turn_ref(key, 0, false)))
    );
}

/// 6 Oct 2026 (Threads review): `keepTurns` reached the provider as a count
/// from the start of its transcript, which a seeded thread (records begin
/// mid-history), a terminal's prompt or a counted steer threw off. It counts
/// the thread's recorded turns, and the provider is told which prompt that
/// is: its digest, and how many later recorded turns repeat it.
#[tokio::test]
async fn fork_at_names_the_kept_recorded_turn_and_refuses_first_busy_and_unknown_turns() {
    let harness = harness();
    let mut parent = row("codex#p", SessionKind::Code);
    parent.title = "Parent title".into();
    harness.sessions.upsert(parent);
    // Seeded from older history: the record holds only what was sent since.
    harness.host.reconcile().await;
    for prompt in ["again", "other", "again"] {
        harness
            .host
            .before_send("codex#p", prompt)
            .await
            .unwrap()
            .unwrap()
            .settle(SendSettlement::Delivered);
        harness.host.close_run("codex#p");
    }
    let fork = json!({"kind": "forkAt", "key": "codex#p", "keepTurns": 1});
    let first = harness
        .command(json!({"kind": "forkAt", "key": "codex#p", "keepTurns": 0}))
        .await
        .unwrap_err();
    assert_eq!(first.code, THREADS_FIRST_TURN_CODE);
    let unknown = harness
        .command(json!({"kind": "forkAt", "key": "codex#p", "keepTurns": 4}))
        .await
        .unwrap_err();
    assert_eq!(unknown.code, THREADS_UNKNOWN_TURN_CODE);
    assert!(unknown.replay_safe);
    harness
        .sessions
        .edit("codex#p", |row| row.status = SessionStatus::Working);
    let busy = harness.command(fork.clone()).await.unwrap_err();
    assert_eq!(busy.code, THREADS_BUSY_CODE);
    assert!(busy.replay_safe);
    assert!(
        lock(&harness.sessions.forks).is_empty(),
        "no provider contact"
    );
    harness
        .sessions
        .edit("codex#p", |row| row.status = SessionStatus::Idle);

    let refused = harness.command(fork.clone()).await.unwrap_err();
    assert_eq!(refused.code, DEFINITELY_UNSENT_CODE);
    assert!(refused.replay_safe);

    *lock(&harness.sessions.next_fork) = Some("codex#child".into());
    let response = harness.command(fork).await.unwrap();
    assert_eq!(response.key.as_deref(), Some("codex#child"));
    let child = thread(&response.view, "codex#child");
    let lineage = child.lineage.as_ref().unwrap();
    assert_eq!(
        (
            lineage.parent.as_str(),
            lineage.relation,
            lineage.kept_turns
        ),
        ("codex#p", LineageRelation::Fork, 1)
    );
    assert_eq!(child.turns.len(), 1);
    assert_eq!(response.view.threads[0].key, "codex#child", "top of Active");
    assert_eq!(
        lock(&harness.sessions.renames).as_slice(),
        [("codex#child".to_owned(), "Parent title (fork)".to_owned())]
    );
    // The first "again" has one later recorded "again" after it.
    assert_eq!(
        lock(&harness.sessions.forks).last(),
        Some(&("codex#p".to_owned(), prompt_digest("again"), 1))
    );
}

/// The provider adapters find a recorded turn's prompt by the digest the
/// store recorded: both sides must digest a prompt the same way.
#[test]
fn the_engine_and_the_store_digest_prompts_alike() {
    for prompt in [
        "Fix the flaky login test",
        "  padded\r\nlines\r\n ",
        "\tümlaut and emoji 🚀\n",
        "",
    ] {
        assert_eq!(
            interface_engine::prompt_digest(prompt),
            prompt_digest(prompt),
            "{prompt:?}"
        );
    }
}

#[tokio::test]
async fn edit_from_restores_forks_moves_ownership_and_answers_the_prompt() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    let key = "codex#p";
    harness.git.moves.store(true, Ordering::SeqCst);
    harness
        .git_thread(key, &root, Some(&worktree), "first prompt")
        .await;
    harness
        .host
        .before_send(key, "second prompt")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    harness.host.close_run(key);
    let first = harness
        .command(json!({"kind": "editFrom", "key": key, "ordinal": 0, "restoreFiles": false}))
        .await
        .unwrap_err();
    assert_eq!(first.code, THREADS_FIRST_TURN_CODE);

    *lock(&harness.sessions.next_fork) = Some("codex#edit".into());
    let response = harness
        .command(json!({"kind": "editFrom", "key": key, "ordinal": 1, "restoreFiles": true}))
        .await
        .unwrap();
    assert_eq!(response.key.as_deref(), Some("codex#edit"));
    assert_eq!(response.prompt.as_deref(), Some("second prompt"));
    assert!(response.restore.is_some());
    let restore = format!("restore {}", turn_ref(key, 1, false));
    assert!(
        harness
            .git
            .calls()
            .iter()
            .any(|call| call.starts_with(&restore))
    );
    assert_eq!(
        lock(&harness.sessions.forks).as_slice(),
        [(key.to_owned(), prompt_digest("first prompt"), 0)],
        "keeps the turn before the edited one"
    );
    let parent = thread(&response.view, key);
    assert_eq!(parent.settled_reason, Some(SettledReason::Edited));
    assert!(!parent.workspace.as_ref().unwrap().owned);
    let child = thread(&response.view, "codex#edit");
    assert!(child.workspace.as_ref().unwrap().owned, "ownership moved");
    assert_eq!(
        child.lineage.as_ref().unwrap().relation,
        LineageRelation::Edit
    );
    assert_eq!(child.turns.len(), 1, "the turns before the edited one");
}

/// Makes `codex#p` a worktree thread with two turns, titled "Fix the
/// login test", whose session is open (live) in its worktree.
async fn open_worktree_thread(harness: &mut Harness) {
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    let key = "codex#p";
    harness.git.moves.store(true, Ordering::SeqCst);
    harness
        .git_thread(key, &root, Some(&worktree), "first prompt")
        .await;
    harness
        .host
        .before_send(key, "second prompt")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    harness.host.close_run(key);
    harness.sessions.edit(key, move |row| {
        row.cwd = worktree;
        row.title = "Fix the login test".into();
    });
    lock(&harness.sessions.live).insert(key.into());
    *lock(&harness.sessions.next_fork) = Some("codex#edit".into());
}

fn edit_turn_1() -> Value {
    json!({"kind": "editFrom", "key": "codex#p", "ordinal": 1, "restoreFiles": false})
}

fn restore_edit_turn_0() -> Value {
    json!({"kind": "restoreTurn", "key": "codex#edit", "ordinal": 0})
}

/// 6 Oct 2026: after Edit from here, "Revert files too" on the new thread
/// was refused: the settled original's session was still open in the
/// worktree, and the refusal said to settle it.
#[tokio::test]
async fn edit_from_closes_the_settled_originals_session_so_the_new_thread_can_restore() {
    let mut harness = harness();
    open_worktree_thread(&mut harness).await;
    let response = harness.command(edit_turn_1()).await.unwrap();
    assert_eq!(response.key.as_deref(), Some("codex#edit"));
    assert_eq!(
        thread(&response.view, "codex#p").settled_reason,
        Some(SettledReason::Edited)
    );
    assert_eq!(
        lock(&harness.sessions.closes).as_slice(),
        ["codex#p".to_owned()]
    );
    assert!(!lock(&harness.sessions.live).contains("codex#p"));
    let restored = harness.command(restore_edit_turn_0()).await.unwrap();
    assert!(
        restored.restore.is_some(),
        "the original no longer holds the worktree"
    );
}

#[tokio::test]
async fn a_restore_held_by_a_settled_threads_open_session_says_to_close_it() {
    let mut harness = harness();
    open_worktree_thread(&mut harness).await;
    harness.sessions.close_fails.store(true, Ordering::SeqCst);
    // The close failing leaves the edit made.
    let response = harness.command(edit_turn_1()).await.unwrap();
    assert_eq!(response.key.as_deref(), Some("codex#edit"));
    assert!(lock(&harness.sessions.live).contains("codex#p"));
    let refused = harness.command(restore_edit_turn_0()).await.unwrap_err();
    assert_eq!(refused.code, THREADS_SHARED_CODE);
    assert!(refused.replay_safe);
    assert_eq!(
        refused.message,
        "“Fix the login test” is still open in this worktree. Archive it to close it, then try again."
    );
}

#[tokio::test]
async fn release_keeps_dirty_worktrees_and_never_runs_under_a_live_agent() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    let key = "codex#wt";
    harness
        .git_thread(key, &root, Some(&worktree), "first")
        .await;
    let release = json!({"kind": "releaseWorkspace", "key": key});
    lock(&harness.sessions.live).insert(key.into());
    assert_eq!(
        harness.command(release.clone()).await.unwrap_err().code,
        THREADS_BUSY_CODE
    );
    lock(&harness.sessions.live).clear();
    *lock(&harness.git.remove) = Some(RemoveOutcome::KeptDirty);
    let dirty = harness.command(release.clone()).await.unwrap_err();
    assert_eq!(dirty.code, THREADS_WORKTREE_DIRTY_CODE);
    assert!(dirty.replay_safe);
    // Ignored files keep it too, and the owner is told why.
    *lock(&harness.git.remove) = Some(RemoveOutcome::KeptIgnored);
    let ignored = harness.command(release.clone()).await.unwrap_err();
    assert_eq!(ignored.code, THREADS_WORKTREE_DIRTY_CODE);
    assert!(
        ignored.message.contains("ignored files"),
        "{}",
        ignored.message
    );
    *lock(&harness.git.remove) = Some(RemoveOutcome::Removed);
    harness.command(release).await.unwrap();
    let removal = format!("remove {worktree}");
    assert_eq!(
        harness
            .git
            .calls()
            .iter()
            .filter(|call| **call == removal)
            .count(),
        3
    );
    let local = harness.dir("local");
    harness.git_thread("codex#local", &local, None, "x").await;
    assert_eq!(
        harness
            .command(json!({"kind": "releaseWorkspace", "key": "codex#local"}))
            .await
            .unwrap_err()
            .code,
        THREADS_NOT_OWNED_CODE
    );
}

/// 6 Oct 2026 (Threads review): a session missing from a partly rebuilt
/// registry lost its thread record and its worktree on the next round. Only
/// the owner's archive, delete or forget releases a worktree now; a missing
/// session's record goes after a day, and its worktree stays.
#[tokio::test]
async fn archiving_or_deleting_releases_an_owned_worktree_and_a_vanished_session_releases_nothing()
{
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    let key = "codex#wt";
    harness
        .git_thread(key, &root, Some(&worktree), "first")
        .await;
    harness.sessions.edit(key, |row| row.archived = true);
    for _ in 0..2 {
        harness.host.reconcile().await;
        settle_tasks(&harness.host).await;
    }
    let removal = format!("remove {worktree}");
    assert_eq!(
        harness
            .git
            .calls()
            .iter()
            .filter(|call| **call == removal)
            .count(),
        1,
        "once per archive"
    );
    let document = harness.document().await;
    assert_eq!(
        document
            .record(key)
            .unwrap()
            .release
            .map(|release| release.outcome),
        Some(ReleaseOutcome::Removed)
    );

    // Gone from the registry without a word: the record is kept for a day
    // and then dropped; the worktree is never released for it.
    let second = harness.dir("atlas-wt2");
    harness
        .git_thread("codex#gone", &root, Some(&second), "x")
        .await;
    harness.sessions.remove("codex#gone");
    harness.host.reconcile().await;
    settle_tasks(&harness.host).await;
    assert!(harness.document().await.record("codex#gone").is_some());
    harness.advance(DAY);
    harness.host.reconcile().await;
    settle_tasks(&harness.host).await;
    assert!(harness.document().await.record("codex#gone").is_none());
    assert!(!harness.git.called(&format!("remove {second}")));

    // Deleted (or forgotten) by the owner: its worktree is released at once.
    let third = harness.dir("atlas-wt3");
    harness
        .git_thread("codex#deleted", &root, Some(&third), "y")
        .await;
    harness.sessions.remove("codex#deleted");
    crate::session_management::RemovalHooks::session_removed(&harness.host, "codex#deleted");
    settle_tasks(&harness.host).await;
    assert!(harness.git.called(&format!("remove {third}")));
    assert_eq!(
        harness
            .document()
            .await
            .record("codex#deleted")
            .unwrap()
            .release
            .map(|release| release.outcome),
        Some(ReleaseOutcome::Removed)
    );
    // A thread without a worktree of its own has nothing to release.
    let calls = harness.git.calls().len();
    crate::session_management::RemovalHooks::session_removed(&harness.host, "codex#wt-none");
    settle_tasks(&harness.host).await;
    assert_eq!(harness.git.calls().len(), calls);
}

/// 6 Oct 2026 (Threads review): archiving or deleting a thread whose
/// worktree had to stay (ignored files, uncommitted changes, another
/// session in it) said nothing. The view pushed after the release now says
/// why, on the archived or deleted thread's own record, until a later
/// release succeeds.
#[tokio::test]
async fn an_archive_or_delete_that_keeps_the_worktree_pushes_why_it_stayed() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    let key = "codex#wt";
    harness
        .git_thread(key, &root, Some(&worktree), "first")
        .await;
    let kept_in = |view: &ThreadsView, key: &str| {
        thread(view, key)
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.kept)
    };
    *lock(&harness.git.remove) = Some(RemoveOutcome::KeptIgnored);
    harness.pushed();
    harness.sessions.edit(key, |row| row.archived = true);
    harness.host.reconcile().await;
    settle_tasks(&harness.host).await;
    let pushed = harness.pushed();
    assert_eq!(
        kept_in(pushed.last().unwrap(), key),
        Some(WorkspaceKept::Ignored)
    );
    // Removed by the owner once it is clean: no longer kept.
    *lock(&harness.git.remove) = Some(RemoveOutcome::Removed);
    let released = harness
        .command(json!({"kind": "releaseWorkspace", "key": key}))
        .await
        .unwrap();
    assert_eq!(kept_in(&released.view, key), None);

    // Deleted with uncommitted changes, and archived while another session
    // works in it: each says so.
    for (name, outcome, kept) in [
        ("dirty", RemoveOutcome::KeptDirty, WorkspaceKept::Dirty),
        ("busy", RemoveOutcome::InUse, WorkspaceKept::InUse),
    ] {
        let other = harness.dir(&format!("atlas-{name}"));
        let key = format!("codex#{name}");
        harness.git_thread(&key, &root, Some(&other), name).await;
        *lock(&harness.git.remove) = Some(outcome);
        harness.sessions.remove(&key);
        crate::session_management::RemovalHooks::session_removed(&harness.host, &key);
        settle_tasks(&harness.host).await;
        let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
        assert_eq!(kept_in(&view, &key), Some(kept), "{name}");
    }
}

/// While the registry is rebuilt from provider listings (its cache did not
/// load), a session it has not listed yet is not missing.
#[tokio::test]
async fn an_incomplete_registry_marks_no_thread_missing() {
    let harness = harness();
    harness.sessions.upsert(row("claude#a", SessionKind::Chat));
    harness.sessions.upsert(row("claude#b", SessionKind::Chat));
    harness.host.reconcile().await;
    harness.sessions.rebuilding.store(true, Ordering::SeqCst);
    harness.sessions.remove("claude#b");
    harness.advance(2 * DAY);
    harness.host.reconcile().await;
    harness.host.reconcile().await;
    let document = harness.document().await;
    assert_eq!(document.record("claude#b").unwrap().missing_since, None);
}

#[tokio::test]
async fn discarded_and_stale_prepared_workspaces_remove_what_they_made() {
    let harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: PathBuf::from(&worktree),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: Some(PathBuf::from(&worktree)),
        branch: Some("iface/fix-1a2b3c".into()),
        base_ref: Some("main".into()),
        created: true,
    });
    let prepare = json!({"kind": "prepareWorkspace", "projectRoot": root, "mode": "worktree", "hint": "Fix it"});
    let token = harness
        .command(prepare.clone())
        .await
        .unwrap()
        .prepared
        .unwrap()
        .token;
    harness
        .command(json!({"kind": "discardWorkspace", "token": token}))
        .await
        .unwrap();
    assert!(harness.git.called(&format!("discard {worktree}")));
    assert!(
        harness
            .git
            .called(&format!("delete {}", pending_baseline_ref(&token)))
    );
    assert!(harness.document().await.prepared.is_empty());
    // Discarding again changes nothing.
    harness
        .command(json!({"kind": "discardWorkspace", "token": token}))
        .await
        .unwrap();

    let stale = harness
        .command(prepare)
        .await
        .unwrap()
        .prepared
        .unwrap()
        .token;
    harness.advance(2 * HOUR);
    harness.host.discard_stale_prepared().await;
    assert!(
        harness
            .git
            .called(&format!("delete {}", pending_baseline_ref(&stale)))
    );
    assert!(harness.document().await.prepared.is_empty());
}

/// 6 Oct 2026 (Threads review): a discard (or the hourly sweep) removed a
/// prepared worktree a session had started in after all (its bind never
/// arrived). It now stays, with its entry, until nothing works there.
#[tokio::test]
async fn discarding_or_sweeping_a_prepared_worktree_keeps_it_while_a_session_works_there() {
    let harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: PathBuf::from(&worktree),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: Some(PathBuf::from(&worktree)),
        branch: Some("iface/fix-1a2b3c".into()),
        base_ref: Some("main".into()),
        created: true,
    });
    let token = harness
        .command(json!({"kind": "prepareWorkspace", "projectRoot": root, "mode": "worktree", "hint": "Fix it"}))
        .await
        .unwrap()
        .prepared
        .unwrap()
        .token;
    let mut launched = row("codex#unbound", SessionKind::Code);
    launched.cwd = format!("{worktree}/src");
    harness.sessions.upsert(launched);
    harness
        .command(json!({"kind": "discardWorkspace", "token": token}))
        .await
        .unwrap();
    harness.advance(2 * HOUR);
    harness.host.discard_stale_prepared().await;
    assert!(!harness.git.called(&format!("discard {worktree}")));
    assert!(
        !harness
            .git
            .called(&format!("delete {}", pending_baseline_ref(&token)))
    );
    assert_eq!(harness.document().await.prepared.len(), 1, "kept for later");

    // Once nothing works there, the next sweep removes it.
    harness.sessions.remove("codex#unbound");
    harness.host.discard_stale_prepared().await;
    assert!(harness.git.called(&format!("discard {worktree}")));
    assert!(harness.document().await.prepared.is_empty());
}

#[tokio::test]
async fn diffs_read_turn_thread_uncommitted_and_branch_scopes() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    let key = "codex#wt";
    harness.git.moves.store(true, Ordering::SeqCst);
    harness
        .git_thread(key, &root, Some(&worktree), "first")
        .await;
    let start = turn_ref(key, 0, false);
    let turn = harness
        .command(json!({"kind": "turnDiff", "key": key, "ordinal": 0}))
        .await
        .unwrap();
    assert_eq!(turn.diff.unwrap().base, start);
    assert!(harness.git.called(&format!("diff {start} worktree")));
    for (scope, base) in [
        ("thread", start.as_str()),
        ("uncommitted", "HEAD"),
        ("branch", "base000"),
    ] {
        let diff = harness
            .command(json!({"kind": "threadDiff", "key": key, "scope": scope}))
            .await
            .unwrap()
            .diff
            .unwrap();
        assert_eq!(diff.base, base, "{scope}");
        assert_eq!(serde_json::to_value(diff.scope).unwrap(), json!(scope));
    }
    assert!(harness.git.called("merge-base main"));
    assert_eq!(
        harness
            .command(json!({"kind": "turnDiff", "key": key, "ordinal": 5}))
            .await
            .unwrap_err()
            .code,
        THREADS_NO_CHECKPOINT_CODE
    );
    harness
        .sessions
        .upsert(row("claude#chat", SessionKind::Chat));
    harness
        .command(json!({"kind": "bind", "key": "claude#chat", "prompt": "hi"}))
        .await
        .unwrap();
    assert_eq!(
        harness
            .command(json!({"kind": "threadDiff", "key": "claude#chat", "scope": "uncommitted"}))
            .await
            .unwrap_err()
            .code,
        THREADS_NO_GIT_CODE
    );
}

#[tokio::test]
async fn branch_and_worktree_lists_check_the_folder_and_name_owning_threads() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    harness
        .git_thread("codex#wt", &root, Some(&worktree), "first")
        .await;
    *lock(&harness.git.worktrees) = vec![
        (PathBuf::from(&root), Some("main".into())),
        (PathBuf::from(&worktree), Some("iface/fix-1a2b3c".into())),
    ];
    let listed = harness
        .command(json!({"kind": "listWorktrees", "projectRoot": root}))
        .await
        .unwrap()
        .worktrees
        .unwrap();
    assert_eq!(listed[0].owner_key, None);
    assert_eq!(listed[1].owner_key.as_deref(), Some("codex#wt"));
    let branches = harness
        .command(json!({"kind": "listBranches", "projectRoot": root}))
        .await
        .unwrap()
        .branches
        .unwrap();
    assert_eq!(branches.current.as_deref(), Some("main"));
    lock(&harness.authority.0).insert(root.clone());
    assert_eq!(
        harness
            .command(json!({"kind": "listBranches", "projectRoot": root}))
            .await
            .unwrap_err()
            .code,
        THREADS_FOLDER_REFUSED_CODE
    );
}

#[tokio::test]
async fn pull_requests_are_recorded_settle_on_merge_and_stay_silent_without_gh() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    let key = "codex#wt";
    harness
        .git_thread(key, &root, Some(&worktree), "first")
        .await;
    harness.pushed();
    let refresh = json!({"kind": "refreshPullRequest", "key": key});
    let quiet = harness.command(refresh.clone()).await.unwrap();
    assert!(thread(&quiet.view, key).pull_request.is_none());
    assert!(
        harness.pushed().is_empty(),
        "gh unavailable changes nothing"
    );
    *lock(&harness.pulls.0) = PullRequestLookup::Found(FoundPullRequest {
        url: "https://github.com/owner/atlas/pull/482".into(),
        number: 482,
        state: PullRequestState::Merged,
    });
    let found = harness.command(refresh).await.unwrap();
    let pull = thread(&found.view, key).pull_request.clone().unwrap();
    assert_eq!((pull.number, pull.state), (482, PullRequestState::Merged));
    harness.host.auto_settle().await;
    let view = harness.pushed().pop().unwrap();
    assert_eq!(
        thread(&view, key).settled_reason,
        Some(SettledReason::Merged)
    );
    harness
        .sessions
        .upsert(row("claude#chat", SessionKind::Chat));
    harness.host.reconcile().await;
    assert_eq!(
        harness
            .command(json!({"kind": "refreshPullRequest", "key": "claude#chat"}))
            .await
            .unwrap_err()
            .code,
        THREADS_PULL_REQUESTS_UNAVAILABLE_CODE
    );
}

#[test]
fn gh_output_parses_by_the_renderers_rules() {
    assert_eq!(
        ports::parse_gh_pr_list(
            br#"[{"number":12,"state":"MERGED","url":"https://github.com/o/r/pull/12"}]"#
        ),
        PullRequestLookup::Found(FoundPullRequest {
            url: "https://github.com/o/r/pull/12".into(),
            number: 12,
            state: PullRequestState::Merged,
        })
    );
    assert_eq!(ports::parse_gh_pr_list(b"[]"), PullRequestLookup::None);
    assert_eq!(
        ports::parse_gh_pr_list(b"gh: not logged in"),
        PullRequestLookup::Unavailable
    );
}

/// Orchestration workers are hidden from the whole agent list, not the
/// control snapshot, which keeps 64 agents: past that, an old finished
/// worker whose session is still in the registry must not come back as a
/// new thread on top of Active.
#[tokio::test]
async fn an_old_finished_worker_past_the_snapshot_bound_never_becomes_a_thread() {
    use crate::orchestration_transport::tests::{finished_workers, owner_with_agents};
    let ids = (1..=69)
        .map(|index| format!("w{index:02}"))
        .collect::<Vec<_>>();
    let ids = ids.iter().map(String::as_str).collect::<Vec<_>>();
    let (orchestration, _root) = owner_with_agents(&finished_workers(&ids)).await;
    let mut harness = hidden_harness(Arc::new(WorkerSessions {
        orchestration: Some(orchestration.clone()),
        away: std::sync::OnceLock::new(),
    }));
    harness
        .sessions
        .upsert(row("claude#chat", SessionKind::Chat));
    harness.host.reconcile().await;
    harness.sessions.upsert(row("codex#w01", SessionKind::Code));
    harness.sessions.upsert(row("codex#w69", SessionKind::Code));
    harness
        .sessions
        .upsert(row("claude#later", SessionKind::Chat));
    harness.host.reconcile().await;
    let keys = harness
        .document()
        .await
        .records
        .iter()
        .map(|record| record.key.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from(["claude#chat".to_owned(), "claude#later".to_owned()])
    );
    let view = harness.pushed().pop().unwrap();
    assert_eq!(view.threads[0].key, "claude#later");
    // Chat's session list hides the same sessions (protocol 58).
    assert_eq!(view.hidden_keys, ["codex#w01", "codex#w69"]);
    orchestration.close().await.unwrap();
}

/// A worker session reaching the registry changes no record, yet every
/// view must name it for Chat: the round pushes a new revision once.
#[tokio::test]
async fn a_newly_hidden_session_alone_pushes_a_new_revision_naming_it() {
    let mut harness = harness();
    harness
        .sessions
        .upsert(row("claude#chat", SessionKind::Chat));
    harness.host.reconcile().await;
    let seeded = harness.pushed().pop().unwrap();
    assert!(seeded.hidden_keys.is_empty());

    lock(&harness.hidden.0).insert("codex#worker".into());
    lock(&harness.hidden.0).insert("codex#gone".into());
    harness
        .sessions
        .upsert(row("codex#worker", SessionKind::Code));
    harness
        .sessions
        .upsert(row("claude#teammate", SessionKind::Agent));
    lock(&harness.hidden.0).insert("claude#teammate".into());
    harness.host.reconcile_round().await;
    let pushed = harness.pushed();
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].revision, seeded.revision + 1);
    assert_eq!(pushed[0].hidden_keys, ["codex#worker"]);
    assert_eq!(harness.document().await.records.len(), 1);
    let read = harness.command(json!({"kind": "read"})).await.unwrap();
    assert_eq!(read.view.hidden_keys, ["codex#worker"]);

    harness.host.reconcile_round().await;
    assert!(harness.pushed().is_empty(), "nothing changed");
}

#[tokio::test]
async fn an_unopened_store_refuses_every_command_and_never_blocks_a_send() {
    let state = tempfile::tempdir().unwrap();
    let harness = harness_with(Err("another writer holds it".into()), state);
    let error = harness.command(json!({"kind": "read"})).await.unwrap_err();
    assert_eq!(error.code, THREADS_STORE_UNAVAILABLE_CODE);
    assert!(error.replay_safe);
    assert!(error.message.contains("another writer holds it"));
    assert!(
        harness
            .host
            .before_send("codex#a", "x")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn invalid_payloads_are_definitely_unsent_and_every_fixture_command_decodes() {
    let harness = harness();
    for payload in [json!(null), json!([]), json!({"kind": "read", "extra": 1})] {
        let error = harness
            .host
            .dispatch(CHANNELS.thread_control, payload)
            .await
            .expect_err("invalid payload");
        assert_eq!(error.code, INVALID_PAYLOAD_CODE);
        assert!(error.replay_safe);
    }
    let error = harness
        .host
        .dispatch(CHANNELS.harness_control, json!({"kind": "read"}))
        .await
        .expect_err("only the thread channel");
    assert_eq!(error.code, OPERATION_NOT_MIGRATED_CODE);
    // Every fixture command decodes, whatever it then answers.
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/threads-v34.json"
    ))
    .unwrap();
    for command in fixture["commands"].as_array().unwrap() {
        if let Err(error) = harness
            .host
            .dispatch(CHANNELS.thread_control, command.clone())
            .await
        {
            assert_ne!(error.code, INVALID_PAYLOAD_CODE, "{command}");
        }
    }
}

#[tokio::test]
async fn push_view_emits_the_threads_push_with_the_whole_view() {
    let mut harness = harness();
    let view = ThreadsView::empty(1_791_298_800_000);
    assert!(harness.host.push_view(&view).await);
    let frame = harness.pushes.recv().await.unwrap();
    assert_eq!(
        serde_json::to_value(frame).unwrap(),
        json!({
            "version": crate::protocol::PROTOCOL_VERSION,
            "operation": "iface:threads",
            "payload": serde_json::to_value(&view).unwrap()
        })
    );
    drop(harness.pushes);
    assert!(!harness.host.push_view(&view).await);
}

/// Polls `check` until it holds, for at most five seconds.
async fn eventually(what: &str, mut check: impl AsyncFnMut() -> bool) {
    for _ in 0..500 {
        if check().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("never happened: {what}");
}

#[tokio::test]
async fn the_worker_reconciles_registry_rounds_applies_run_changes_and_stops_cleanly() {
    let harness = harness();
    harness.sessions.upsert(row("claude#a", SessionKind::Chat));
    harness.host.start();
    harness.host.start(); // A second start does nothing.
    eventually("startup seeding", async || {
        harness.document().await.record("claude#a").is_some()
    })
    .await;

    // A registry round with a new session seeds it.
    harness.sessions.upsert(row("codex#b", SessionKind::Code));
    harness.host.inner.registry.registry_round();
    eventually("a registry round", async || {
        harness.document().await.record("codex#b").is_some()
    })
    .await;

    // The tap's run changes reach the store through the worker.
    harness.host.observe_run(
        "codex#b",
        &EngineEventKind::StatusChanged {
            status: SessionStatus::Working,
        },
    );
    eventually("a run start", async || {
        harness
            .document()
            .await
            .record("codex#b")
            .is_some_and(ThreadRecord::running)
    })
    .await;
    harness.host.shutdown().await;
    assert!(lock(&harness.host.inner.worker).is_none());
}

/// git with no user or system configuration, for building fixtures.
fn fixture_git(dir: &Path, args: &[&str]) -> String {
    let mut command = std::process::Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(name);
        }
    }
    let output = command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[tokio::test]
async fn bind_files_the_launch_baseline_under_its_thread_and_discards_delete_pending_ones() {
    let state = tempfile::tempdir().unwrap();
    let store = ThreadStore::open(&state.path().join(THREADS_FILE_NAME), NOW)
        .map_err(|error| error.to_string());
    let harness = harness_on(
        store,
        state,
        Some(Arc::new(SystemThreadGit) as Arc<dyn ThreadGit>),
    );
    let root = std::fs::canonicalize(harness.dir("atlas")).unwrap();
    fixture_git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    fixture_git(&root, &["add", "-A"]);
    fixture_git(&root, &["commit", "-q", "-m", "initial"]);
    let refs = || {
        fixture_git(
            &root,
            &["for-each-ref", "--format=%(refname)", "refs/interface/"],
        )
    };
    let prepare = json!({
        "kind": "prepareWorkspace",
        "projectRoot": root.to_string_lossy(),
        "mode": "local",
        "hint": "Fix it"
    });
    let token = harness
        .command(prepare.clone())
        .await
        .unwrap()
        .prepared
        .unwrap()
        .token;
    let pending = pending_baseline_ref(&token);
    assert_eq!(refs(), pending);
    let baseline = fixture_git(&root, &["rev-parse", &pending]);

    let key = "codex#real";
    harness.sessions.upsert(row(key, SessionKind::Code));
    harness
        .command(json!({"kind": "bind", "key": key, "token": token, "prompt": "Fix it"}))
        .await
        .unwrap();
    let start = turn_ref(key, 0, false);
    assert_eq!(
        refs(),
        start,
        "filed under the thread, the pending name gone"
    );
    assert_eq!(fixture_git(&root, &["rev-parse", &start]), baseline);
    assert_eq!(
        harness.document().await.record(key).unwrap().turns[0]
            .start_ref
            .as_deref(),
        Some(start.as_str())
    );

    // A launch that failed discards its baseline, and so does the sweep of
    // prepares never bound within the hour.
    let discarded = harness
        .command(prepare.clone())
        .await
        .unwrap()
        .prepared
        .unwrap()
        .token;
    assert!(refs().contains(&pending_baseline_ref(&discarded)));
    harness
        .command(json!({"kind": "discardWorkspace", "token": discarded}))
        .await
        .unwrap();
    let unbound = harness
        .command(prepare)
        .await
        .unwrap()
        .prepared
        .unwrap()
        .token;
    assert!(refs().contains(&pending_baseline_ref(&unbound)));
    harness.advance(2 * HOUR);
    harness.host.discard_stale_prepared().await;
    assert_eq!(refs(), start);
}

fn set_status(harness: &Harness, key: &str, status: SessionStatus) {
    harness.sessions.edit(key, |row| row.status = status);
}

const IDLE: EngineEventKind = EngineEventKind::StatusChanged {
    status: SessionStatus::Idle,
};

fn delta() -> EngineEventKind {
    EngineEventKind::MessageDelta {
        text: "…".into(),
        agent_key: None,
    }
}

/// Starts `key`'s launch run as the tap sees it: open, active, registry working.
fn running(harness: &Harness, key: &str) {
    harness.host.open_run(key);
    set_status(harness, key, SessionStatus::Working);
    harness.host.observe_run(key, &delta());
}

#[tokio::test]
async fn a_send_steered_into_a_running_codex_or_claude_turn_opens_no_turn() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    for key in ["codex#steer", "claude#steer"] {
        harness.git_thread(key, &root, None, "first").await;
        running(&harness, key);
        harness.pushed();
        harness
            .host
            .before_send(key, "also cover the logout path")
            .await
            .unwrap()
            .expect("a steer ticket")
            .settle(SendSettlement::Delivered);
        let record = harness.document().await.record(key).unwrap().clone();
        assert_eq!(record.turns.len(), 1, "{key}: a steer opens no turn");
        assert_eq!(record.turns[0].state, TurnState::Running);
        assert!(
            !harness
                .git
                .called(&format!("capture {}", turn_ref(key, 1, false)))
        );
        assert!(harness.pushed().is_empty(), "nothing changed");
        assert!(harness.host.run_tracked(key), "the running turn goes on");

        // The provider folded it in: the run ends and nothing starts again.
        set_status(&harness, key, SessionStatus::Idle);
        harness.host.observe_run(key, &IDLE);
        harness.host.apply_run(ops.try_recv().unwrap()).await;
        settle_tasks(&harness.host).await;
        let record = harness.document().await.record(key).unwrap().clone();
        assert_eq!(record.turns.len(), 1);
        assert_eq!(record.turns[0].state, TurnState::Done);

        // The owner's next prompt opens turn 1; the folded steer never does.
        harness.advance(STEER_RESTART_WINDOW_MS + 1);
        harness
            .host
            .before_send(key, "next")
            .await
            .unwrap()
            .unwrap()
            .settle(SendSettlement::Delivered);
        let record = harness.document().await.record(key).unwrap().clone();
        assert_eq!(record.turns.len(), 2);
        assert_eq!(record.turns[1].prompt_digest, prompt_digest("next"));
        harness.host.close_run(key);
    }
    assert!(ops.try_recv().is_err());
}

#[tokio::test]
async fn a_steer_the_provider_runs_as_its_own_turn_is_recorded_when_that_turn_starts() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    let key = "claude#late";
    harness.git_thread(key, &root, None, "first").await;
    running(&harness, key);
    let turns = async || harness.document().await.record(key).unwrap().turns.clone();

    // The turn ended before Claude took the steer from its queue, so Claude
    // runs it next: that run is the steer's turn.
    harness
        .host
        .before_send(key, "and the logout test")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    harness.host.observe_run(key, &IDLE);
    harness.host.apply_run(ops.try_recv().unwrap()).await;
    harness.advance(1_500);
    harness.host.observe_run(key, &delta());
    let op = ops.try_recv().unwrap();
    assert_eq!(op.change, RunChange::Started);
    harness.host.apply_run(op).await;
    let recorded = turns().await;
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].state, TurnState::Done);
    assert_eq!(recorded[1].ordinal, 1);
    assert_eq!(
        recorded[1].prompt_digest,
        prompt_digest("and the logout test")
    );
    assert_eq!(recorded[1].state, TurnState::Running);
    assert!(recorded[1].start_ref.is_none(), "no checkpoint mid-run");
    let record = harness.document().await.record(key).unwrap().clone();
    assert_eq!(record.last_run.unwrap().started_at, NOW + 1_500);

    // An undelivered steer, one a stop took down, and a run that starts
    // long after the steered one ended never become turns.
    let mut restart = async |steer: Option<(&str, bool)>, stop: bool, after: u64| {
        if let Some((text, delivered)) = steer {
            harness
                .host
                .before_send(key, text)
                .await
                .unwrap()
                .expect("a steer ticket")
                .settle(if delivered {
                    SendSettlement::Delivered
                } else {
                    SendSettlement::Unsent
                });
        }
        if stop {
            harness.host.interrupt_requested(key);
        }
        harness.host.observe_run(key, &IDLE);
        harness.host.apply_run(ops.try_recv().unwrap()).await;
        harness.advance(after);
        harness.host.observe_run(key, &delta());
        harness.host.apply_run(ops.try_recv().unwrap()).await;
    };
    restart(Some(("never sent", false)), false, 10).await;
    restart(Some(("stopped", true)), true, 10).await;
    restart(
        Some(("folded long ago", true)),
        false,
        STEER_RESTART_WINDOW_MS + 1,
    )
    .await;
    assert_eq!(turns().await.len(), 2);

    // An engine that never folds a send records it as its own turn.
    let other = "acp:grok#busy";
    harness.git_thread(other, &root, None, "first").await;
    running(&harness, other);
    harness
        .host
        .before_send(other, "queued by the provider")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    let record = harness.document().await.record(other).unwrap().clone();
    assert_eq!(record.turns.len(), 2);
    assert_eq!(record.turns[0].state, TurnState::Done);
}

fn transcript(events: Vec<EngineEventKind>, older_messages: u64) -> HistoryPage {
    HistoryPage {
        availability: None,
        reason: None,
        messages: u64::try_from(events.len()).ok(),
        events: events
            .into_iter()
            .map(|event| EngineEvent { at: None, event })
            .collect(),
        older_messages,
    }
}

fn said(text: &str) -> EngineEventKind {
    EngineEventKind::UserMessage {
        text: text.into(),
        agent_key: None,
        images: None,
        image_references: None,
    }
}

fn answered(text: &str) -> EngineEventKind {
    EngineEventKind::MessageDelta {
        text: text.into(),
        agent_key: None,
    }
}

async fn generated_titles(harness: &Harness, enabled: bool) {
    harness
        .command(json!({"kind": "setSettings", "settings": {
            "autoSettleDays": 3,
            "settleOnMerge": true,
            "generatedTitles": enabled,
            "trackPullRequests": true
        }}))
        .await
        .unwrap();
}

/// Ends `key`'s open run as completed through the tap and the worker.
async fn complete_run(
    harness: &Harness,
    ops: &mut mpsc::UnboundedReceiver<super::run_tap::RunOp>,
    key: &str,
) {
    harness.host.observe_run(key, &delta());
    harness.host.observe_run(key, &IDLE);
    harness.host.apply_run(ops.try_recv().unwrap()).await;
    settle_tasks(&harness.host).await;
}

fn title_source(document: &ThreadsDocument, key: &str) -> Option<TitleSource> {
    document.record(key).unwrap().title_source
}

#[tokio::test]
async fn a_threads_title_is_asked_for_as_its_first_prompt_goes_out_without_waiting_for_its_run() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    // On by default: nothing was switched on.
    assert!(harness.document().await.settings.generated_titles);
    let key = "codex#titled";
    harness
        .git_thread(key, &root, None, "Write the release checklist for atlas")
        .await;
    // Named from the launch prompt alone while its turn still runs.
    assert_eq!(
        lock(&harness.titles.calls).clone(),
        [(
            "Write the release checklist for atlas".to_owned(),
            String::new()
        )]
    );
    assert_eq!(
        lock(&harness.sessions.renames).clone(),
        [(key.to_owned(), "Login test fixed".to_owned())]
    );
    let document = harness.document().await;
    assert_eq!(title_source(&document, key), Some(TitleSource::Generated));
    let record = document.record(key).unwrap();
    assert_eq!(record.turns[0].state, TurnState::Running);
    assert!(record.last_run.as_ref().unwrap().ended_at.is_none());

    // Its first completed run, and every later one, asks for nothing.
    harness.host.open_run(key);
    complete_run(&harness, &mut ops, key).await;
    harness
        .host
        .before_send(key, "and the logout test")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    complete_run(&harness, &mut ops, key).await;
    assert_eq!(lock(&harness.titles.calls).len(), 1);
    assert_eq!(lock(&harness.sessions.renames).len(), 1);
}

#[tokio::test]
async fn a_failed_first_title_is_asked_for_once_more_after_the_first_completed_run() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    *lock(&harness.titles.reply) = Err("Claude did not answer within 30 seconds".into());
    let key = "codex#retried";
    harness
        .git_thread(key, &root, None, "Fix the login test")
        .await;
    assert_eq!(lock(&harness.titles.calls).len(), 1);
    assert!(
        lock(&harness.sessions.renames).is_empty(),
        "nothing changed"
    );
    assert_eq!(
        title_source(&harness.document().await, key),
        Some(TitleSource::Prompt)
    );

    // After the first completed run, once more, with the first answer.
    *lock(&harness.sessions.history) = Some(transcript(
        vec![
            said("Fix the login test"),
            answered("Fixed: "),
            answered("the token had expired."),
        ],
        0,
    ));
    *lock(&harness.titles.reply) = Ok("Login test fixed".into());
    harness.host.open_run(key);
    complete_run(&harness, &mut ops, key).await;
    assert_eq!(
        lock(&harness.titles.calls)[1],
        (
            "Fix the login test".to_owned(),
            "Fixed: the token had expired.".to_owned()
        )
    );
    assert_eq!(
        lock(&harness.sessions.renames).clone(),
        [(key.to_owned(), "Login test fixed".to_owned())]
    );
    assert_eq!(
        title_source(&harness.document().await, key),
        Some(TitleSource::Generated)
    );

    // Two failures and the thread keeps its prompt's title: no third call.
    *lock(&harness.titles.reply) = Err("Claude exited with an error".into());
    let failing = "codex#failing";
    harness
        .git_thread(failing, &root, None, "Fix the login test")
        .await;
    harness.host.open_run(failing);
    complete_run(&harness, &mut ops, failing).await;
    harness
        .host
        .before_send(failing, "again")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    complete_run(&harness, &mut ops, failing).await;
    assert_eq!(lock(&harness.titles.calls).len(), 4, "two each");
    assert_eq!(
        title_source(&harness.document().await, failing),
        Some(TitleSource::Prompt)
    );
    assert_eq!(lock(&harness.sessions.renames).len(), 1);
}

#[tokio::test]
async fn an_owners_title_is_never_replaced_by_an_automatic_one() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    // The first call fails; the owner renames before the run completes.
    *lock(&harness.titles.reply) = Err("Claude exited with an error".into());
    let renamed = "codex#renamed";
    harness
        .git_thread(renamed, &root, None, "Fix the login test")
        .await;
    harness.host.before_rename(renamed).await.settle(true);
    settle_tasks(&harness.host).await;
    assert_eq!(
        title_source(&harness.document().await, renamed),
        Some(TitleSource::User)
    );
    *lock(&harness.titles.reply) = Ok("Login test fixed".into());
    harness.host.open_run(renamed);
    complete_run(&harness, &mut ops, renamed).await;
    assert_eq!(lock(&harness.titles.calls).len(), 1, "no retry");

    // A thread started with its own title (`user`) is never asked about.
    let (prepared, owned) = (harness.prepare_local(&root).await, "codex#owned");
    harness.sessions.upsert(row(owned, SessionKind::Code));
    harness
        .command(json!({"kind": "bind", "key": owned, "token": prepared,
            "prompt": "Fix the login test", "titleSource": "user"}))
        .await
        .unwrap();
    settle_tasks(&harness.host).await;
    assert_eq!(lock(&harness.titles.calls).len(), 1);
    assert!(lock(&harness.sessions.renames).is_empty());

    // A rename that definitely did not happen records nothing.
    harness.titles.available.store(false, Ordering::SeqCst);
    let unrenamed = "codex#unrenamed";
    harness
        .git_thread(unrenamed, &root, None, "Fix the login test")
        .await;
    harness.host.before_rename(unrenamed).await.settle(false);
    settle_tasks(&harness.host).await;
    assert_eq!(
        title_source(&harness.document().await, unrenamed),
        Some(TitleSource::Prompt)
    );
}

#[tokio::test]
async fn a_chat_with_no_project_and_a_thread_started_with_no_prompt_are_named_too() {
    let harness = harness();
    // A chat started with no project binds with no workspace.
    let chat = "claude#chat";
    harness.sessions.upsert(row(chat, SessionKind::Chat));
    harness
        .command(json!({"kind": "bind", "key": chat, "prompt": "What is a monad, simply?"}))
        .await
        .unwrap();
    settle_tasks(&harness.host).await;
    assert_eq!(
        lock(&harness.titles.calls).clone(),
        [("What is a monad, simply?".to_owned(), String::new())]
    );
    assert_eq!(
        title_source(&harness.document().await, chat),
        Some(TitleSource::Generated)
    );

    // A thread team tools started is created empty; its first prompt (the
    // task the host sends) names it.
    let root = harness.dir("atlas");
    let token = harness.prepare_local(&root).await;
    let child = "codex#child";
    harness.sessions.upsert(row(child, SessionKind::Code));
    harness
        .host
        .bind_empty_thread(child, &token, TitleSource::Prompt)
        .await
        .unwrap();
    settle_tasks(&harness.host).await;
    assert_eq!(lock(&harness.titles.calls).len(), 1, "no prompt yet");
    harness
        .host
        .before_send(child, "Port the parser tests")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    settle_tasks(&harness.host).await;
    assert_eq!(
        lock(&harness.titles.calls)[1],
        ("Port the parser tests".to_owned(), String::new())
    );
    assert_eq!(
        title_source(&harness.document().await, child),
        Some(TitleSource::Generated)
    );
}

#[test]
fn the_host_turns_an_old_stores_generated_titles_on_once_at_open() {
    let state = tempfile::tempdir().unwrap();
    let file = state.path().join(THREADS_FILE_NAME);
    {
        // A store written while titles were off by default.
        let mut store = ThreadStore::open(&file, NOW).unwrap();
        store
            .apply(NOW, |document| {
                document.settings.generated_titles = false;
                Ok(())
            })
            .unwrap();
    }
    let mut store = ThreadHost::open_store(state.path()).unwrap();
    assert!(store.document().settings.generated_titles, "turned on");
    // The owner turns them off; the next launch keeps that.
    store
        .apply(NOW + 1, |document| {
            document.settings.generated_titles = false;
            Ok(())
        })
        .unwrap();
    drop(store);
    let store = ThreadHost::open_store(state.path()).unwrap();
    assert!(!store.document().settings.generated_titles, "still off");
}

#[tokio::test]
async fn with_generated_titles_off_or_without_claude_nothing_is_asked() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    generated_titles(&harness, false).await;
    harness
        .git_thread("codex#off", &root, None, "Fix the login test")
        .await;
    harness.host.open_run("codex#off");
    complete_run(&harness, &mut ops, "codex#off").await;
    generated_titles(&harness, true).await;
    harness.titles.available.store(false, Ordering::SeqCst);
    harness
        .git_thread("codex#no-claude", &root, None, "Fix the login test")
        .await;
    harness.host.open_run("codex#no-claude");
    complete_run(&harness, &mut ops, "codex#no-claude").await;
    assert!(lock(&harness.titles.calls).is_empty());
    assert!(lock(&harness.sessions.renames).is_empty());
    let document = harness.document().await;
    for key in ["codex#off", "codex#no-claude"] {
        assert_eq!(title_source(&document, key), Some(TitleSource::Prompt));
    }
}

#[tokio::test]
async fn regenerate_title_renames_on_demand_and_fails_definitely_unsent() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let regenerate = |key: &str| json!({"kind": "regenerateTitle", "key": key});
    let unknown = harness.command(regenerate("codex#gone")).await.unwrap_err();
    assert_eq!(unknown.code, THREADS_UNKNOWN_CODE);

    // Only the on-demand title here.
    generated_titles(&harness, false).await;
    let key = "codex#again";
    harness
        .git_thread(key, &root, None, "Fix the flaky login test")
        .await;
    harness.titles.available.store(false, Ordering::SeqCst);
    let missing = harness.command(regenerate(key)).await.unwrap_err();
    assert_eq!(missing.code, THREADS_TITLES_UNAVAILABLE_CODE);
    assert!(missing.replay_safe && !missing.retryable);
    assert!(lock(&harness.titles.calls).is_empty());

    // Claude failing renames nothing; with no transcript to read, the title
    // comes from the launch prompt the store kept.
    harness.titles.available.store(true, Ordering::SeqCst);
    *lock(&harness.titles.reply) = Err("Claude exited with an error".into());
    let failed = harness.command(regenerate(key)).await.unwrap_err();
    assert_eq!(failed.code, THREADS_TITLE_FAILED_CODE);
    assert!(failed.replay_safe && failed.retryable);
    assert!(failed.message.contains("Claude exited with an error"));
    assert_eq!(
        lock(&harness.titles.calls).clone(),
        [("Fix the flaky login test".to_owned(), String::new())]
    );
    assert!(lock(&harness.sessions.renames).is_empty());

    // On demand, even the owner's own title is replaced. A transcript page
    // that does not reach the first prompt is not read for it.
    harness.host.before_rename(key).await.settle(true);
    *lock(&harness.sessions.history) = Some(transcript(
        vec![said("A later prompt"), answered("A later answer")],
        40,
    ));
    *lock(&harness.titles.reply) = Ok("Login flake fixed".into());
    let response = harness.command(regenerate(key)).await.unwrap();
    assert_eq!(
        thread(&response.view, key).title_source,
        Some(TitleSource::Generated)
    );
    assert_eq!(
        lock(&harness.sessions.renames).clone(),
        [(key.to_owned(), "Login flake fixed".to_owned())]
    );
    assert_eq!(
        lock(&harness.titles.calls).last().unwrap().0,
        "Fix the flaky login test"
    );
}

/// A thread with no workspace and nothing recorded yet, as reconcile seeds
/// one, plus the worker's queue of run changes.
async fn seeded_chat(
    harness: &Harness,
    key: &str,
) -> mpsc::UnboundedReceiver<super::run_tap::RunOp> {
    harness.sessions.upsert(row(key, SessionKind::Chat));
    harness.host.reconcile().await;
    lock(&harness.host.inner.ops_receiver).take().unwrap()
}

/// 6 Oct 2026 (Threads review): a run end the worker had not applied yet
/// when the next send recorded its turn finished that new turn instead.
#[tokio::test]
async fn a_stale_run_end_never_finishes_the_next_turn() {
    let harness = harness();
    let key = "claude#a";
    let mut ops = seeded_chat(&harness, key).await;
    harness
        .host
        .before_send(key, "first")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Delivered);
    harness.host.observe_run(key, &delta());
    harness.host.observe_run(key, &IDLE);
    let stale_end = ops.try_recv().unwrap();
    // The next send records its turn before the worker gets to that end.
    let ticket = harness
        .host
        .before_send(key, "second")
        .await
        .unwrap()
        .unwrap();
    // A registry round meanwhile (the provider not working yet) leaves the
    // new turn running: the tap opened its run in the same commit.
    harness.host.reconcile().await;
    harness.host.apply_run(stale_end).await;
    ticket.settle(SendSettlement::Delivered);
    let record = harness.document().await.record(key).unwrap().clone();
    assert!(record.running(), "{:?}", record.last_run);
    assert_eq!(record.turns[1].state, TurnState::Running);
    assert_eq!(record.turns[0].state, TurnState::Done);

    // The new turn's own end applies.
    harness.host.observe_run(key, &delta());
    harness.host.observe_run(key, &IDLE);
    harness.host.apply_run(ops.try_recv().unwrap()).await;
    let record = harness.document().await.record(key).unwrap().clone();
    assert!(!record.running());
    assert_eq!(record.turns[1].state, TurnState::Done);
}

/// 6 Oct 2026 (Threads review): an ACP prompt is answered only when its
/// turn ends, so a refusal can come after the agent worked; the turn was
/// taken back anyway. A refused send keeps its turn once the provider
/// answered in it.
#[tokio::test]
async fn a_rejected_send_is_taken_back_only_when_the_provider_never_answered() {
    let harness = harness();
    let key = "acp:grok#a";
    let _ops = seeded_chat(&harness, key).await;
    harness
        .host
        .before_send(key, "refused at once")
        .await
        .unwrap()
        .unwrap()
        .settle(SendSettlement::Rejected);
    settle_tasks(&harness.host).await;
    let record = harness.document().await.record(key).unwrap().clone();
    assert!(record.turns.is_empty(), "never answered: taken back");
    assert!(record.last_run.is_none());
    assert!(!harness.host.run_tracked(key));

    let ticket = harness
        .host
        .before_send(key, "ran, then failed")
        .await
        .unwrap()
        .unwrap();
    harness.host.observe_run(key, &delta());
    // The provider's own error and idle reach the tap before the refusal.
    harness.host.observe_run(
        key,
        &EngineEventKind::Error {
            cause: None,
            message: "rate limited".into(),
            retryable: true,
        },
    );
    harness.host.observe_run(key, &IDLE);
    ticket.settle(SendSettlement::Rejected);
    settle_tasks(&harness.host).await;
    let record = harness.document().await.record(key).unwrap().clone();
    assert_eq!(record.turns.len(), 1, "it ran: kept");
    assert_eq!(
        record.turns[0].prompt_digest,
        prompt_digest("ran, then failed")
    );
}

/// 6 Oct 2026 (Threads review): a restore could run while a send was on
/// its way to the provider. Restore and Edit from here hold the thread's
/// send gate, and refuse while a send holds it.
#[tokio::test]
async fn restore_and_edit_refuse_while_a_send_holds_the_threads_gate() {
    let mut harness = harness();
    open_worktree_thread(&mut harness).await;
    lock(&harness.sessions.live).clear();
    let gate = harness.host.send_gate("codex#p").lock_owned().await;
    for command in [
        json!({"kind": "restoreTurn", "key": "codex#p", "ordinal": 1}),
        edit_turn_1(),
        json!({"kind": "forkAt", "key": "codex#p", "keepTurns": 1}),
    ] {
        let refused = harness.command(command.clone()).await.unwrap_err();
        assert_eq!(refused.code, THREADS_BUSY_CODE, "{command}");
        assert!(refused.replay_safe);
    }
    assert!(
        !harness
            .git
            .calls()
            .iter()
            .any(|call| call.starts_with("restore"))
    );
    assert!(lock(&harness.sessions.forks).is_empty());
    drop(gate);
    let restored = harness
        .command(json!({"kind": "restoreTurn", "key": "codex#p", "ordinal": 1}))
        .await
        .unwrap();
    assert!(restored.restore.is_some());
}

/// "Edit from here" on the first prompt with "Revert files too": the
/// renderer restores turn 0, settles the original and starts a thread in
/// the same worktree as an existing one. That thread takes the ownership.
#[tokio::test]
async fn a_thread_started_in_a_settled_threads_worktree_takes_its_ownership() {
    let mut harness = harness();
    let root = harness.dir("atlas");
    let worktree = harness.dir("atlas-wt");
    harness
        .git_thread("codex#original", &root, Some(&worktree), "first prompt")
        .await;
    harness
        .command(json!({"kind": "settle", "key": "codex#original"}))
        .await
        .unwrap();
    *lock(&harness.git.prepared) = Some(PreparedGitWorkspace {
        cwd: PathBuf::from(&worktree),
        repo_root: Some(PathBuf::from(&root)),
        worktree_path: Some(PathBuf::from(&worktree)),
        branch: Some("iface/fix-1a2b3c".into()),
        base_ref: None,
        created: false,
    });
    let prepared = harness
        .command(json!({
            "kind": "prepareWorkspace",
            "projectRoot": root,
            "mode": "existing",
            "worktreePath": worktree,
            "hint": "first prompt, edited",
        }))
        .await
        .unwrap()
        .prepared
        .unwrap();
    assert!(!prepared.workspace.owned);
    harness
        .sessions
        .upsert(row("codex#again", SessionKind::Code));
    let response = harness
        .command(json!({
            "kind": "bind",
            "key": "codex#again",
            "token": prepared.token,
            "prompt": "first prompt, edited",
        }))
        .await
        .unwrap();
    let owned = |key: &str| {
        thread(&response.view, key)
            .workspace
            .as_ref()
            .unwrap()
            .owned
    };
    assert!(owned("codex#again"));
    assert!(!owned("codex#original"));
    // The new owner may restore and release it.
    harness.host.close_run("codex#again");
    let restored = harness
        .command(json!({"kind": "restoreTurn", "key": "codex#again", "ordinal": 0}))
        .await;
    assert!(restored.is_ok(), "{restored:?}");
}

#[path = "threads_ship_tests.rs"]
mod ship;

#[path = "threads_team_tests.rs"]
mod team;

#[path = "threads_switch_tests.rs"]
mod switch;

#[path = "threads_schedules_tests.rs"]
mod scheduled;

#[path = "threads_folder_trust_tests.rs"]
mod folder_trust;

/// Review C1: Open in terminal ends the thread's structured session first and
/// holds the session for one Code pane; a running turn, a pane already on it
/// and providers with no resumable CLI are refused, definitely unsent.
#[tokio::test]
async fn open_in_terminal_closes_the_thread_first_and_holds_it_for_one_pane() {
    let harness = harness();
    let mut thread = row("claude#c1", SessionKind::Code);
    thread.cwd = "/work/atlas".into();
    harness.sessions.upsert(thread);
    lock(&harness.sessions.live).insert("claude#c1".into());
    let response = harness
        .command(json!({"kind": "openInTerminal", "key": "claude#c1"}))
        .await
        .unwrap();
    let terminal = response.terminal.unwrap();
    assert_eq!(terminal.profile_id, TerminalLaunchProfileId::Claude);
    assert_eq!(terminal.provider_session_id, "c1");
    assert_eq!(terminal.cwd, "/work/atlas");
    assert_eq!(*lock(&harness.sessions.closes), ["claude#c1"]);
    assert!(!lock(&harness.sessions.live).contains("claude#c1"));
    assert_eq!(
        harness.claims.claim("claude#c1"),
        Some(interface_runtime::SessionClaim::HandingOff)
    );

    // A second Open in terminal while the first pane starts, or once it runs.
    let again = harness
        .command(json!({"kind": "openInTerminal", "key": "claude#c1"}))
        .await
        .unwrap_err();
    assert_eq!(again.code, "threads-in-terminal");
    assert!(again.replay_safe);
    lock(&harness.panes.0).push("c1".into());
    assert_eq!(
        harness.claims.claim("claude#c1"),
        Some(interface_runtime::SessionClaim::Terminal)
    );

    // A running turn stays where it is; so does an unconfirmed close.
    let mut busy = row("codex#c2", SessionKind::Code);
    busy.status = SessionStatus::Working;
    harness.sessions.upsert(busy);
    let refused = harness
        .command(json!({"kind": "openInTerminal", "key": "codex#c2"}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "threads-session-busy");
    harness
        .sessions
        .edit("codex#c2", |row| row.status = SessionStatus::Idle);
    lock(&harness.sessions.live).insert("codex#c2".into());
    harness.sessions.close_fails.store(true, Ordering::SeqCst);
    let unconfirmed = harness
        .command(json!({"kind": "openInTerminal", "key": "codex#c2"}))
        .await
        .unwrap_err();
    assert_eq!(unconfirmed.code, "threads-close-unconfirmed");
    assert!(!unconfirmed.replay_safe);
    assert_eq!(harness.claims.claim("codex#c2"), None, "nothing held");

    harness
        .sessions
        .upsert(row("acp:grok#c3", SessionKind::Code));
    let unsupported = harness
        .command(json!({"kind": "openInTerminal", "key": "acp:grok#c3"}))
        .await
        .unwrap_err();
    assert_eq!(unsupported.code, "threads-terminal-unsupported");
}

/// Review C1: a terminal session waits under From terminals; Continue as
/// thread takes it only once no running pane drives it, into Active, visited.
#[tokio::test]
async fn continue_from_terminal_waits_for_the_pane_then_brings_the_session_into_threads() {
    let mut harness = harness();
    let mut cli = row("claude#t1", SessionKind::Code);
    cli.origin = Some(SessionOrigin::Store);
    harness.sessions.upsert(cli);
    harness.host.reconcile().await;
    let view = harness.pushed().pop().unwrap();
    assert_eq!(
        thread(&view, "claude#t1").section,
        ThreadSection::FromTerminals
    );

    lock(&harness.panes.0).push("t1".into());
    let panes = harness.panes.clone();
    let release = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        lock(&panes.0).clear();
    });
    let response = harness
        .command(json!({"kind": "continueFromTerminal", "key": "claude#t1"}))
        .await
        .unwrap();
    release.await.unwrap();
    let continued = thread(&response.view, "claude#t1");
    assert_eq!(continued.section, ThreadSection::Active);
    assert_eq!(continued.last_visited_at, Some(NOW));
    assert_eq!(response.view.threads[0].key, "claude#t1", "top of Active");
}

/// Second opinion (review B4): the thread's session is opened through the
/// runtime when it is not live, and one consult goes to the other provider
/// with an objective naming the latest turn's changes; the answer carries the
/// consult the collaboration service reported.
#[tokio::test]
async fn second_opinion_resumes_the_thread_and_submits_one_consult_on_its_latest_turn() {
    use interface_contracts::ipc::{CollaborationEngine, CollaborationMode, CollaborationState};

    let mut harness = harness();
    harness.git.moves.store(true, Ordering::SeqCst);
    let root = harness.dir("repo");
    harness
        .git_thread("claude#a", &root, None, "Fix the login retry")
        .await;
    let response = harness
        .command(json!({
            "kind": "secondOpinion", "key": "claude#a", "engine": "codex", "requestId": "op-1",
        }))
        .await
        .unwrap();
    let consult = response.consult.expect("the consult");
    assert_eq!(consult.receipt.state, CollaborationState::AwaitingApproval);
    assert_eq!(consult.source_session_key, "claude#a");
    assert_eq!(*lock(&harness.consults.resumes), ["claude#a"]);
    let submits = lock(&harness.consults.submits).clone();
    assert_eq!(submits.len(), 1);
    assert_eq!(submits[0].mode, CollaborationMode::Consult);
    assert_eq!(submits[0].target_engine, CollaborationEngine::Codex);
    assert_eq!(submits[0].client_request_id, "op-1");
    assert!(
        submits[0]
            .objective
            .starts_with("Give a second opinion on Claude's latest answer"),
        "{}",
        submits[0].objective
    );
    assert!(
        submits[0]
            .objective
            .ends_with("That turn changed no files.")
    );
    let start = turn_ref("claude#a", 0, false);
    assert!(
        harness.git.called(&format!("diff {start} worktree")),
        "the latest turn's diff: {:?}",
        harness.git.calls()
    );

    // A live session is not resumed again.
    lock(&harness.sessions.live).insert("claude#a".into());
    harness
        .command(json!({
            "kind": "secondOpinion", "key": "claude#a", "engine": "acp:grok", "requestId": "op-2",
        }))
        .await
        .unwrap();
    assert_eq!(lock(&harness.consults.resumes).len(), 1);
    assert_eq!(lock(&harness.consults.submits).len(), 2);
}

/// Second opinion refuses, definitely unsent and before anything is opened
/// or submitted: the thread's own provider, a session a Code pane holds or
/// is opening for, a running turn, and a resume the runtime refused.
#[tokio::test]
async fn second_opinion_refuses_a_held_session_a_running_turn_and_its_own_provider() {
    let mut harness = harness();
    let root = harness.dir("repo");
    harness.git_thread("claude#a", &root, None, "Fix it").await;
    let ask = |engine: &str| json!({"kind": "secondOpinion", "key": "claude#a", "engine": engine, "requestId": "op"});

    let own = harness.command(ask("claude")).await.unwrap_err();
    assert_eq!(own.code, super::opinion::THREADS_SAME_PROVIDER_CODE);

    lock(&harness.panes.0).push("a".into());
    let held = harness.command(ask("codex")).await.unwrap_err();
    assert_eq!(held.code, super::terminal_bridge::THREADS_IN_TERMINAL_CODE);
    assert_eq!(
        held.message,
        interface_runtime::SessionClaim::Terminal.message()
    );
    lock(&harness.panes.0).clear();
    harness.claims.hand_to_terminal("claude#a");
    let handing = harness.command(ask("codex")).await.unwrap_err();
    assert_eq!(
        handing.code,
        super::terminal_bridge::THREADS_IN_TERMINAL_CODE
    );
    harness.claims.release_handoff("claude#a");

    harness
        .sessions
        .edit("claude#a", |row| row.status = SessionStatus::Working);
    let busy = harness.command(ask("codex")).await.unwrap_err();
    assert_eq!(busy.code, super::terminal_bridge::THREADS_SESSION_BUSY_CODE);
    harness
        .sessions
        .edit("claude#a", |row| row.status = SessionStatus::Idle);

    harness.consults.resume_fails.store(true, Ordering::SeqCst);
    let unopened = harness.command(ask("codex")).await.unwrap_err();
    assert_eq!(unopened.code, DEFINITELY_UNSENT_CODE);

    for error in [own, held, handing, busy, unopened] {
        assert!(error.replay_safe, "{}", error.code);
    }
    assert_eq!(
        lock(&harness.consults.resumes).len(),
        1,
        "only the failed resume"
    );
    assert!(
        lock(&harness.consults.submits).is_empty(),
        "nothing was submitted"
    );
}

/// Continue with (review B4): a project thread's folder is prepared again
/// for the other provider with the thread's conversation and changes as the
/// first prompt; binding the new session records a `handoff` lineage from
/// the thread. Plain chats, the thread's own provider, a running turn and a
/// worktree that is gone are refused.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn continue_with_prepares_the_threads_folder_and_binds_a_handoff_lineage() {
    let mut harness = harness();
    harness.git.moves.store(true, Ordering::SeqCst);
    let root = harness.dir("repo");
    harness
        .git_thread("claude#a", &root, None, "Fix the login retry")
        .await;
    *lock(&harness.sessions.history) = Some(HistoryPage {
        availability: None,
        reason: None,
        events: vec![
            EngineEvent {
                at: None,
                event: EngineEventKind::UserMessage {
                    text: "Fix the login retry".into(),
                    agent_key: None,
                    images: None,
                    image_references: None,
                },
            },
            EngineEvent {
                at: None,
                event: EngineEventKind::MessageDelta {
                    text: "Retries now back off.".into(),
                    agent_key: None,
                },
            },
        ],
        messages: None,
        older_messages: 0,
    });
    let response = harness
        .command(json!({"kind": "continueWith", "key": "claude#a", "engine": "codex"}))
        .await
        .unwrap();
    let prepared = response.prepared.expect("the new thread's workspace");
    assert_eq!(prepared.cwd, root);
    assert_eq!(prepared.workspace.mode, WorkspaceMode::Local);
    let prompt = response.prompt.expect("the first prompt");
    assert!(prompt.starts_with(
        "You are taking over a thread from Claude in Interface; the owner asked Codex to continue it."
    ));
    assert!(prompt.contains("Thread: Title a\n"));
    assert!(prompt.contains("user: Fix the login retry\n\nassistant: Retries now back off."));
    let first = turn_ref("claude#a", 0, false);
    assert!(harness.git.called(&format!("diff {first} worktree")));

    harness.sessions.upsert(row("codex#b", SessionKind::Code));
    let bound = harness
        .command(json!({
            "kind": "bind", "key": "codex#b", "token": prepared.token, "prompt": prompt,
        }))
        .await
        .unwrap();
    let child = thread(&bound.view, "codex#b");
    let lineage = child.lineage.as_ref().expect("its lineage");
    assert_eq!(
        (
            lineage.parent.as_str(),
            lineage.relation,
            lineage.kept_turns,
            lineage.at
        ),
        ("claude#a", LineageRelation::Handoff, 0, NOW)
    );
    assert_eq!(child.section, ThreadSection::Active);
    assert_eq!(
        thread(&bound.view, "claude#a").section,
        ThreadSection::Active,
        "the thread it continues stays as it was"
    );

    let own = harness
        .command(json!({"kind": "continueWith", "key": "claude#a", "engine": "claude"}))
        .await
        .unwrap_err();
    assert_eq!(own.code, super::opinion::THREADS_SAME_PROVIDER_CODE);
    harness
        .sessions
        .edit("claude#a", |row| row.status = SessionStatus::Waiting);
    let busy = harness
        .command(json!({"kind": "continueWith", "key": "claude#a", "engine": "codex"}))
        .await
        .unwrap_err();
    assert_eq!(busy.code, super::terminal_bridge::THREADS_SESSION_BUSY_CODE);

    harness
        .sessions
        .upsert(row("claude#chat", SessionKind::Chat));
    harness
        .command(json!({"kind": "bind", "key": "claude#chat", "prompt": "Hello"}))
        .await
        .unwrap();
    let plain = harness
        .command(json!({"kind": "continueWith", "key": "claude#chat", "engine": "codex"}))
        .await
        .unwrap_err();
    assert_eq!(plain.code, super::opinion::THREADS_CONTINUE_PLAIN_CODE);

    let worktree = harness.dir("wt");
    harness
        .git_thread("claude#w", &root, Some(&worktree), "Try it in a worktree")
        .await;
    std::fs::remove_dir_all(&worktree).unwrap();
    let gone = harness
        .command(json!({"kind": "continueWith", "key": "claude#w", "engine": "codex"}))
        .await
        .unwrap_err();
    assert_eq!(gone.code, super::opinion::THREADS_CONTINUE_MISSING_CODE);
    for error in [own, busy, plain, gone] {
        assert!(error.replay_safe, "{}", error.code);
    }
}

/// The journal's ops as the host sent them, applied to a writer over the
/// harness's data root.
mod journal {
    use std::sync::Arc;

    use interface_contracts::{
        engine::{EngineEvent, EngineEventKind, SessionKind, SessionStatus},
        ipc::{CHANNELS, HistoryAvailability},
        thread_journal::{JournalEntry, JournalLine, ThreadJournalResponse, TurnOrigin},
    };
    use interface_settings::thread_journal::{self, JOURNAL_DIR, ThreadJournal};
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::super::journal::{JournalCore, JournalOp};
    use super::{Harness, NOW, journal_harness, lock, row};
    use crate::dispatcher::{
        HistoryFallback, HostDispatcher, SendHooks, SendSettlement, SessionEventObserver,
    };
    use crate::session_management::RemovalHooks;

    struct Quiet;

    impl SessionEventObserver for Quiet {
        fn observe_session_event(&self, _: &str, _: &EngineEvent) {}
    }

    fn ops(harness: &Harness) -> mpsc::UnboundedReceiver<JournalOp> {
        lock(&harness.host.inner.journal.as_ref().unwrap().receiver)
            .take()
            .unwrap()
    }

    fn drain(ops: &mut mpsc::UnboundedReceiver<JournalOp>) -> Vec<JournalOp> {
        let mut drained = Vec::new();
        while let Ok(op) = ops.try_recv() {
            drained.push(op);
        }
        drained
    }

    fn written(harness: &Harness, ops: Vec<JournalOp>, key: &str) -> Vec<JournalLine> {
        let mut core = JournalCore::new(ThreadJournal::open(harness.state.path()).unwrap());
        for op in ops {
            core.apply(op, NOW);
        }
        drop(core);
        thread_journal::read(&harness.state.path().join(JOURNAL_DIR), key, None, 100).entries
    }

    fn shown(lines: &[JournalLine]) -> Vec<String> {
        lines
            .iter()
            .map(|line| match &line.entry {
                JournalEntry::TurnStarted { origin, prompt, .. } => format!("{origin:?}>{prompt}"),
                JournalEntry::Message { text, .. } => format!("message:{text}"),
                JournalEntry::TurnEnded { outcome, .. } => format!("end:{outcome:?}"),
                other => serde_json::to_value(other).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            })
            .collect()
    }

    fn live(event: EngineEventKind) -> EngineEvent {
        EngineEvent { at: None, event }
    }

    fn delta(text: &str) -> EngineEventKind {
        EngineEventKind::MessageDelta {
            text: text.into(),
            agent_key: None,
        }
    }

    async fn seeded(harness: &Harness, key: &str) {
        harness.sessions.upsert(row(key, SessionKind::Chat));
        harness.host.reconcile().await;
        // The thread worker is not running; its queue is not read.
        lock(&harness.host.inner.ops_receiver).take();
    }

    #[tokio::test]
    async fn a_sent_prompt_and_its_answer_reach_the_journal_in_pump_order() {
        let harness = journal_harness();
        let key = "claude#journal";
        seeded(&harness, key).await;
        let mut journal = ops(&harness);
        let tap = harness.host.run_tap(Arc::new(Quiet));
        harness
            .host
            .before_send(key, "Find the flaky test")
            .await
            .unwrap()
            .unwrap()
            .settle(SendSettlement::Delivered);
        for event in [
            EngineEventKind::StatusChanged {
                status: SessionStatus::Working,
            },
            delta("It is the "),
            EngineEventKind::UsageUpdated {
                input_tokens: 1,
                output_tokens: 1,
                rate_limited: None,
            },
            delta("login test."),
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            },
        ] {
            tap.observe_session_event(key, &live(event));
        }
        let sent = drain(&mut journal);
        // Usage and status changes that change no run are not sent.
        assert_eq!(sent.len(), 4, "{sent:?}");
        let lines = written(&harness, sent, key);
        assert_eq!(
            shown(&lines),
            [
                "Owner>Find the flaky test",
                "message:It is the login test.",
                "end:Completed"
            ]
        );
    }

    #[tokio::test]
    async fn an_unsent_prompt_is_retracted_and_an_away_round_is_the_hosts() {
        let harness = journal_harness();
        let key = "claude#journal";
        seeded(&harness, key).await;
        let mut journal = ops(&harness);
        harness
            .host
            .before_send(key, "never left")
            .await
            .unwrap()
            .unwrap()
            .settle(SendSettlement::Unsent);
        // The take-back runs on a tracked task; the retract goes out first.
        let lines = written(&harness, drain(&mut journal), key);
        assert_eq!(shown(&lines), ["Owner>never left", "turnRetracted"]);
        // Deleting the session removes its journal.
        harness.host.session_removed(key);
        let removed = drain(&mut journal);
        assert!(matches!(&removed[..], [JournalOp::Removed { key: removed }] if removed == key));
        let _ = written(&harness, removed, key);
        assert!(
            !harness
                .state
                .path()
                .join(JOURNAL_DIR)
                .join(thread_journal::thread_hash(key))
                .exists()
        );
        harness.host.shutdown().await;
    }

    #[tokio::test]
    async fn a_launch_prompt_is_journalled_by_its_bind() {
        let harness = journal_harness();
        let key = "claude#launched";
        harness.sessions.upsert(row(key, SessionKind::Chat));
        let mut journal = ops(&harness);
        // The launch answers before the bind.
        harness
            .host
            .journal_event(key, &delta("Hi!"), Some(super::RunChange::Started));
        harness
            .command(json!({"kind": "bind", "key": key, "prompt": "Say hi"}))
            .await
            .unwrap();
        harness.host.journal_event(
            key,
            &EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            },
            Some(super::RunChange::Ended(super::RunOutcome::Completed)),
        );
        let lines = written(&harness, drain(&mut journal), key);
        assert_eq!(
            shown(&lines),
            ["Launch>Say hi", "message:Hi!", "end:Completed"]
        );
    }

    /// The whole worker: open, write, drain, then read through
    /// `iface:threadJournal` and the history fallback.
    #[tokio::test]
    async fn the_journal_worker_drains_and_its_reads_find_the_answer() {
        let harness = journal_harness();
        let key = "claude#worker";
        seeded(&harness, key).await;
        harness.host.start_journal();
        harness
            .host
            .journal_prompt(key, TurnOrigin::Owner, "Which test flakes?");
        harness
            .host
            .journal_event(key, &delta("The Zeppelin login test."), None);
        harness.host.journal_event(
            key,
            &EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            },
            Some(super::RunChange::Ended(super::RunOutcome::Completed)),
        );
        harness.host.drain_journal().await;
        let search = harness
            .host
            .dispatch(
                CHANNELS.thread_journal,
                json!({"kind": "search", "query": "zeppelin"}),
            )
            .await
            .unwrap();
        let search: ThreadJournalResponse = serde_json::from_value(search).unwrap();
        let hits = search.hits.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, key);
        assert_eq!(hits[0].excerpt, "The Zeppelin login test.");
        let tail = harness
            .host
            .dispatch(CHANNELS.thread_journal, json!({"kind": "tail", "key": key}))
            .await
            .unwrap();
        let page = serde_json::from_value::<ThreadJournalResponse>(tail)
            .unwrap()
            .page
            .unwrap();
        assert_eq!(page.entries.len(), 3);
        assert_eq!(page.last_seq, 3);
        // An invalid read is refused, replay-safe.
        let refused = harness
            .host
            .dispatch(
                CHANNELS.thread_journal,
                json!({"kind": "tail", "key": key, "limit": 0}),
            )
            .await
            .unwrap_err();
        assert!(refused.replay_safe);
        // A provider with no history shows the journal.
        let page = harness.host.history(key, 500).await.unwrap();
        assert!(page.availability.is_none());
        assert_eq!(page.events.len(), 3);
        assert!(harness.host.history("claude#none", 500).await.is_none());
        assert_ne!(page.availability, Some(HistoryAvailability::Unavailable));
    }

    fn conversation(turns: std::ops::Range<usize>) -> Vec<EngineEvent> {
        turns
            .flat_map(|n| {
                [
                    EngineEvent {
                        at: None,
                        event: EngineEventKind::UserMessage {
                            text: format!("old prompt {n}"),
                            agent_key: None,
                            images: None,
                            image_references: None,
                        },
                    },
                    EngineEvent {
                        at: None,
                        event: EngineEventKind::MessageDelta {
                            text: format!("old answer {n}"),
                            agent_key: None,
                        },
                    },
                ]
            })
            .collect()
    }

    /// Review 15 Nov 2026: the import read the renderer's newest 500 items,
    /// so a long conversation's oldest part was never searchable.
    #[tokio::test]
    async fn an_eight_hundred_item_history_is_imported_whole() {
        let harness = journal_harness();
        let key = "claude#long";
        seeded(&harness, key).await;
        let page = |events: Vec<EngineEvent>, older: u64| interface_contracts::ipc::HistoryPage {
            availability: None,
            reason: None,
            messages: Some(events.len() as u64),
            events,
            older_messages: older,
        };
        // The renderer's page: the newest 500 of 800 items, starting mid-turn.
        let all = conversation(0..400);
        *lock(&harness.sessions.history) = Some(page(all[300..].to_vec(), 300));
        *lock(&harness.sessions.whole_history) = Some(page(all, 0));
        harness.host.start_journal();
        let root = harness.state.path().join(JOURNAL_DIR);
        for _ in 0..500 {
            if thread_journal::read(&root, key, None, 1).last_seq > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        harness.host.drain_journal().await;
        let lines = thread_journal::read::lines(&root, key);
        let prompts = lines
            .iter()
            .filter_map(|line| match &line.entry {
                JournalEntry::TurnStarted { prompt, .. } => Some(prompt.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(prompts.len(), 400);
        assert_eq!(prompts[0], "old prompt 0");
        assert!(matches!(lines[0].entry, JournalEntry::TurnStarted { .. }));
        let hits = thread_journal::search(
            &root,
            "old prompt 0",
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        );
        assert!(hits.iter().any(|hit| hit.excerpt == "old prompt 0"));
    }
}

/// Protocol 60: Continue with the thread's own provider on another of its
/// accounts is allowed (the renderer launches the new thread on it); naming
/// the account the thread already runs on is still the same place.
#[tokio::test]
async fn continue_with_the_same_provider_on_another_account_prepares_the_thread() {
    let mut harness = harness();
    harness.git.moves.store(true, Ordering::SeqCst);
    let root = harness.dir("repo");
    harness
        .git_thread("claude#a", &root, None, "Fix the login")
        .await;
    let response = harness
        .command(json!({
            "kind": "continueWith", "key": "claude#a", "engine": "claude",
            "account": "acct-0123456789ab",
        }))
        .await
        .unwrap();
    assert_eq!(
        response.prepared.expect("the new thread's workspace").cwd,
        root
    );
    assert!(response.prompt.is_some());
    let same = harness
        .command(json!({
            "kind": "continueWith", "key": "claude#a", "engine": "claude", "account": "default",
        }))
        .await
        .unwrap_err();
    assert_eq!(same.code, super::opinion::THREADS_SAME_PROVIDER_CODE);
}

/// A turn that finishes quickly: the run tap closes the run and queues its
/// end for the worker, and a reconcile runs before the worker applies it
/// (the registry already says idle, and the tap no longer holds the run
/// open). The reconcile must leave the run alone: its end is on its way, so
/// the turn and the run end completed and the thread is unread, never
/// filed as stopped.
#[tokio::test]
async fn a_finished_run_whose_end_is_still_queued_is_not_filed_as_stopped_by_a_reconcile() {
    let mut harness = harness();
    let root = harness.dir("quick");
    let key = "claude#quick";
    harness.git_thread(key, &root, None, "answer quickly").await;
    // The launch run, open in the tap as a bind opens it.
    harness.host.open_run(key);
    let mut ops = lock(&harness.host.inner.ops_receiver).take().unwrap();
    harness.host.observe_run(
        key,
        &EngineEventKind::MessageDelta {
            text: "done".into(),
            agent_key: None,
        },
    );
    harness.host.observe_run(
        key,
        &EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    );
    // The tap closed the run; its end waits in the worker's queue.
    let ended = ops.try_recv().expect("the end is queued");
    assert!(matches!(
        ended.change,
        RunChange::Ended(RunOutcome::Completed)
    ));
    assert!(ops.try_recv().is_err());
    // A registry round lands first.
    harness.host.reconcile().await;
    harness.host.apply_run(ended).await;
    settle_tasks(&harness.host).await;

    let document = harness.document().await;
    let record = document.record(key).unwrap();
    assert_eq!(record.turns[0].state, TurnState::Done);
    assert_eq!(
        record.last_run.as_ref().and_then(|run| run.outcome),
        Some(RunOutcome::Completed)
    );
    let view = harness.command(json!({"kind": "read"})).await.unwrap().view;
    assert!(thread(&view, key).unread, "a finished run is news");

    // A run nothing will end (its session went idle while the host held no
    // run and expects no end, as after a restart) is still closed as
    // interrupted.
    let cut = "claude#cut";
    harness.git_thread(cut, &root, None, "cut off").await;
    harness.host.reconcile().await;
    let document = harness.document().await;
    assert_eq!(
        document
            .record(cut)
            .unwrap()
            .last_run
            .as_ref()
            .and_then(|run| run.outcome),
        Some(RunOutcome::Interrupted)
    );
}

#[path = "threads_references_tests.rs"]
mod references;
