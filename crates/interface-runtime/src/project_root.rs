use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use interface_contracts::engine::{EngineId, SessionSummary, session_key};
use interface_registry::{GitRunner, SessionRegistry};
use tokio::sync::Notify;

use crate::{
    RuntimeError,
    runtime::{RuntimeServices, blocking, lock, registry_error},
};

#[derive(Default)]
struct ProjectRootState {
    cache: HashMap<String, CachedRoot>,
    resolving: HashSet<String>,
    queue: VecDeque<String>,
    running: bool,
}

const ROOT_LOOKUP_CONCURRENCY: usize = 4;
const ROOT_LOOKUP_QUEUE_CAPACITY: usize = 256;
const NOT_REPOSITORY_TTL: Duration = Duration::from_secs(5 * 60);
const LOOKUP_FAILURE_TTL: Duration = Duration::from_secs(2);
/// A deleted worktree or temp cwd can only answer once it is recreated; a stat
/// finds that without spawning anything.
const MISSING_DIRECTORY_TTL: Duration = Duration::from_secs(15 * 60);

struct CachedRoot {
    root: Option<String>,
    expires: Option<Instant>,
}

impl CachedRoot {
    fn resolved(root: String) -> Self {
        Self {
            root: Some(root),
            expires: None,
        }
    }

    fn negative(ttl: Duration) -> Self {
        Self {
            root: None,
            expires: Some(Instant::now() + ttl),
        }
    }

    fn fresh(&self) -> bool {
        self.expires.is_none_or(|expires| expires > Instant::now())
    }
}

pub(crate) struct ProjectRootProjector {
    git: Arc<dyn GitRunner>,
    state: Mutex<ProjectRootState>,
    pending: AtomicUsize,
    idle: Notify,
}

impl ProjectRootProjector {
    pub(crate) fn new(git: Arc<dyn GitRunner>) -> Self {
        Self {
            git,
            state: Mutex::new(ProjectRootState::default()),
            pending: AtomicUsize::new(0),
            idle: Notify::new(),
        }
    }

    /// Seeds durable/provider roots, attaches cached roots, and returns the
    /// distinct ordinary cwd values that still need a lookup.
    pub(crate) fn prepare(
        &self,
        registry: &SessionRegistry,
        engine: &EngineId,
        summaries: &mut [SessionSummary],
    ) -> Vec<String> {
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        let mut unresolved = HashSet::new();
        for summary in summaries {
            if summary.cwd.is_empty() {
                continue;
            }
            let key = session_key(engine, &summary.session_id);
            let previous = registry.get(&key);
            let durable_root = previous
                .filter(|known| {
                    known.cwd == summary.cwd || is_worktree_location(known, &summary.cwd)
                })
                .and_then(|known| known.project_root.clone());
            if let Some(root) = summary.project_root.clone().or(durable_root) {
                summary.project_root = Some(root.clone());
                state
                    .cache
                    .insert(summary.cwd.clone(), CachedRoot::resolved(root));
                continue;
            }
            if let Some(entry) = state.cache.get(&summary.cwd).filter(|entry| entry.fresh()) {
                summary.project_root.clone_from(&entry.root);
            } else {
                let worktree = summary.worktree_home.is_some()
                    || summary.worktree_path.is_some()
                    || previous.is_some_and(|known| is_worktree_location(known, &summary.cwd));
                if !worktree {
                    unresolved.insert(summary.cwd.clone());
                }
            }
        }
        let mut unresolved = unresolved.into_iter().collect::<Vec<_>>();
        // Expired negative answers must not starve never-attempted roots when
        // the unresolved set is larger than the bounded queue.
        unresolved.sort_by(|left, right| {
            state
                .cache
                .contains_key(left)
                .cmp(&state.cache.contains_key(right))
                .then_with(|| left.cmp(right))
        });
        unresolved
    }

    pub(crate) fn schedule(
        self: &Arc<Self>,
        services: &RuntimeServices,
        unresolved: impl IntoIterator<Item = String>,
    ) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        for cwd in unresolved {
            if state.queue.len() >= ROOT_LOOKUP_QUEUE_CAPACITY {
                // Not marked in-flight: a later listing can admit this cwd.
                break;
            }
            if state.cache.get(&cwd).is_some_and(CachedRoot::fresh)
                || !state.resolving.insert(cwd.clone())
            {
                continue;
            }
            state.queue.push_back(cwd);
            self.pending.fetch_add(1, Ordering::AcqRel);
        }
        if state.running || state.queue.is_empty() {
            return;
        }
        state.running = true;
        drop(state);
        let projector = Arc::clone(self);
        let services = services.clone();
        tokio::spawn(async move {
            projector.drain(&services).await;
        });
    }

    async fn drain(&self, services: &RuntimeServices) {
        loop {
            let jobs = {
                let Ok(mut state) = self.state.lock() else {
                    // A poisoned state ends the projector as at every other
                    // lock site here. Nothing else will finish the queued
                    // lookups, so release the idle barrier they hold.
                    self.abandon_pending();
                    return;
                };
                if state.queue.is_empty() {
                    state.running = false;
                    return;
                }
                let count = state.queue.len().min(ROOT_LOOKUP_CONCURRENCY);
                state
                    .queue
                    .drain(..count)
                    .map(|cwd| {
                        let git = Arc::clone(&self.git);
                        let lookup_cwd = cwd.clone();
                        (
                            cwd,
                            tokio::task::spawn_blocking(move || {
                                resolve_project_root(git.as_ref(), &lookup_cwd)
                            }),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let count = jobs.len();
            let mut roots = HashMap::new();
            for (cwd, job) in jobs {
                let result = job
                    .await
                    .unwrap_or_else(|_| CachedRoot::negative(LOOKUP_FAILURE_TTL));
                if let Some(root) = self.finish_lookup(&cwd, result) {
                    roots.insert(cwd, root);
                }
            }
            if !roots.is_empty()
                && let Err(error) = persist_project_roots(services.clone(), roots).await
            {
                eprintln!("[runtime] project-root projection failed: {error}");
            }
            for _ in 0..count {
                self.complete_one();
            }
        }
    }

    pub(crate) async fn wait_idle(&self) {
        loop {
            let idle = self.idle.notified();
            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            idle.await;
        }
    }

    fn finish_lookup(&self, cwd: &str, resolved: CachedRoot) -> Option<String> {
        let Ok(mut state) = self.state.lock() else {
            return resolved.root;
        };
        state.resolving.remove(cwd);
        let cached = state
            .cache
            .entry(cwd.to_owned())
            .or_insert_with(|| CachedRoot::negative(Duration::ZERO));
        // A provider/durable answer that arrived during Git wins the race.
        if !cached.fresh() {
            *cached = resolved;
        }
        cached.root.clone()
    }

    fn complete_one(&self) {
        if self.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.idle.notify_waiters();
        }
    }

    fn abandon_pending(&self) {
        if self.pending.swap(0, Ordering::AcqRel) != 0 {
            self.idle.notify_waiters();
        }
    }
}

fn is_worktree_location(summary: &SessionSummary, cwd: &str) -> bool {
    summary.worktree_home.as_deref() == Some(cwd) || summary.worktree_path.as_deref() == Some(cwd)
}

fn resolve_project_root(git: &dyn GitRunner, cwd: &str) -> CachedRoot {
    if cwd.is_empty() {
        return CachedRoot::negative(NOT_REPOSITORY_TTL);
    }
    if !Path::new(cwd).is_dir() {
        return CachedRoot::negative(MISSING_DIRECTORY_TTL);
    }
    let result = git.run(
        &["rev-parse".into(), "--show-toplevel".into()],
        Path::new(cwd),
    );
    if result.code != 0 {
        let not_repository = result.code > 0
            && (result.stderr.contains("not a git repository")
                || result.stderr.contains("not a repository"));
        return CachedRoot::negative(if not_repository {
            NOT_REPOSITORY_TTL
        } else {
            LOOKUP_FAILURE_TTL
        });
    }
    let root = result.stdout.trim();
    if root.is_empty() || root.chars().any(char::is_control) {
        CachedRoot::negative(LOOKUP_FAILURE_TTL)
    } else {
        CachedRoot::resolved(root.to_owned())
    }
}

async fn persist_project_roots(
    services: RuntimeServices,
    roots: HashMap<String, String>,
) -> Result<(), RuntimeError> {
    let sessions = services.session_registry();
    let changed = blocking(move || {
        let mut registry = lock(&sessions, "session registry")?;
        let mut by_engine = HashMap::<EngineId, Vec<SessionSummary>>::new();
        for summary in registry.summaries() {
            let Some(root) = roots.get(&summary.cwd) else {
                continue;
            };
            if summary.project_root.is_some()
                || summary.worktree_home.is_some()
                || summary.worktree_path.is_some()
            {
                continue;
            }
            let mut summary = summary.clone();
            summary.project_root = Some(root.clone());
            by_engine
                .entry(summary.engine.clone())
                .or_default()
                .push(summary);
        }
        let changed = !by_engine.is_empty();
        for (engine, summaries) in by_engine {
            let revision = registry.revision();
            registry
                .upsert_all(revision, &engine, summaries)
                .map_err(registry_error)?;
        }
        Ok(changed)
    })
    .await?;
    if changed {
        services.schedule_registry_cache_save();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{Arc, Mutex, atomic::AtomicBool},
    };

    use interface_contracts::engine::{EngineId, SessionOrigin, SessionStatus, SessionSummary};
    use interface_registry::{GitResult, GitRunner, SessionRegistry};

    use super::*;

    #[test]
    fn negative_answers_expire_and_provider_answers_win_inflight_lookups() {
        let temporary = tempfile::tempdir().unwrap();
        let registry = SessionRegistry::open(temporary.path()).unwrap();
        let engine = EngineId::new("claude").unwrap();
        let projector = ProjectRootProjector::new(Arc::new(ScriptedGit::new([])));
        projector.finish_lookup("/new", CachedRoot::negative(NOT_REPOSITORY_TTL));
        let mut row = summary("new", "/new");
        assert!(
            projector
                .prepare(&registry, &engine, std::slice::from_mut(&mut row))
                .is_empty()
        );
        projector
            .state
            .lock()
            .unwrap()
            .cache
            .get_mut("/new")
            .unwrap()
            .expires = Some(Instant::now());
        assert_eq!(
            projector.prepare(&registry, &engine, std::slice::from_mut(&mut row)),
            ["/new"]
        );
        row.project_root = Some("/authoritative".into());
        projector.prepare(&registry, &engine, std::slice::from_mut(&mut row));
        assert_eq!(
            projector.finish_lookup("/new", CachedRoot::resolved("/late-git".into())),
            Some("/authoritative".into())
        );
        let failure = CachedRoot::negative(LOOKUP_FAILURE_TTL);
        let not_repository = CachedRoot::negative(NOT_REPOSITORY_TTL);
        assert!(failure.expires < not_repository.expires);
    }

    #[tokio::test]
    async fn resolver_limits_children_and_batches_registry_merges() {
        struct CountingGit {
            active: AtomicUsize,
            maximum: AtomicUsize,
        }
        impl GitRunner for CountingGit {
            fn run(&self, _args: &[String], cwd: &Path) -> GitResult {
                let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
                self.maximum.fetch_max(active, Ordering::AcqRel);
                std::thread::sleep(Duration::from_millis(5));
                self.active.fetch_sub(1, Ordering::AcqRel);
                GitResult {
                    code: 0,
                    stdout: cwd.to_string_lossy().into_owned(),
                    stderr: String::new(),
                }
            }
        }
        let temporary = tempfile::tempdir().unwrap();
        let mut registry = SessionRegistry::open(temporary.path()).unwrap();
        let engine = EngineId::new("claude").unwrap();
        let rows = (0..32)
            .map(|index| {
                let cwd = temporary.path().join(format!("fixture/{index}"));
                std::fs::create_dir_all(&cwd).unwrap();
                summary(&index.to_string(), &cwd.to_string_lossy())
            })
            .collect::<Vec<_>>();
        let roots = rows.iter().map(|row| row.cwd.clone()).collect::<Vec<_>>();
        registry
            .upsert_all(registry.revision(), &engine, rows)
            .unwrap();
        let before = registry.revision();
        let history = crate::EngineHistoryStore::open(
            temporary.path().join("history"),
            crate::HistoryLimits::default(),
        )
        .unwrap();
        let services = RuntimeServices::new(registry, history);
        let git = Arc::new(CountingGit {
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
        });
        let projector = Arc::new(ProjectRootProjector::new(git.clone()));
        projector.schedule(&services, roots.clone());
        projector.schedule(&services, roots);
        projector.wait_idle().await;
        assert!(git.maximum.load(Ordering::Acquire) <= ROOT_LOOKUP_CONCURRENCY);
        let sessions = services.session_registry();
        let registry = sessions.lock().unwrap();
        assert!(
            registry
                .summaries()
                .all(|row| row.project_root.as_deref() == Some(row.cwd.as_str()))
        );
        assert_eq!(registry.revision() - before, 8);
    }

    #[tokio::test]
    async fn a_poisoned_drain_releases_the_idle_barrier() {
        struct GatedGit {
            open: Arc<AtomicBool>,
        }
        impl GitRunner for GatedGit {
            fn run(&self, _args: &[String], cwd: &Path) -> GitResult {
                while !self.open.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                GitResult {
                    code: 0,
                    stdout: cwd.to_string_lossy().into_owned(),
                    stderr: String::new(),
                }
            }
        }
        let temporary = tempfile::tempdir().unwrap();
        let registry = SessionRegistry::open(temporary.path()).unwrap();
        let history = crate::EngineHistoryStore::open(
            temporary.path().join("history"),
            crate::HistoryLimits::default(),
        )
        .unwrap();
        let services = RuntimeServices::new(registry, history);
        let roots = (0..2 * ROOT_LOOKUP_CONCURRENCY)
            .map(|index| {
                let cwd = temporary.path().join(format!("fixture/{index}"));
                std::fs::create_dir_all(&cwd).unwrap();
                cwd.to_string_lossy().into_owned()
            })
            .collect::<Vec<_>>();
        let open = Arc::new(AtomicBool::new(false));
        let projector = Arc::new(ProjectRootProjector::new(Arc::new(GatedGit {
            open: Arc::clone(&open),
        })));
        projector.schedule(&services, roots);
        // Wait until the first batch is blocked in Git and the rest is queued.
        while projector.state.lock().unwrap().queue.len() != ROOT_LOOKUP_CONCURRENCY {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let poisoner = Arc::clone(&projector);
        let _ = std::thread::spawn(move || {
            let _state = poisoner.state.lock().unwrap();
            panic!("poison the project-root state");
        })
        .join();
        assert!(projector.state.is_poisoned());
        open.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(5), projector.wait_idle())
            .await
            .expect("the queued lookups no longer hold the idle barrier");
        assert_eq!(projector.pending.load(Ordering::Acquire), 0);
    }

    struct ScriptedGit {
        results: Mutex<Vec<(String, GitResult)>>,
        calls: Mutex<Vec<String>>,
    }

    impl ScriptedGit {
        fn new(results: impl IntoIterator<Item = (String, GitResult)>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl GitRunner for ScriptedGit {
        fn run(&self, args: &[String], cwd: &Path) -> GitResult {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("{}:{}", cwd.display(), args.join(" ")));
            let mut results = self.results.lock().expect("results");
            let cwd = cwd.to_string_lossy();
            if let Some(position) = results
                .iter()
                .position(|(candidate, _)| candidate == cwd.as_ref())
            {
                results.remove(position).1
            } else {
                GitResult {
                    code: 1,
                    stdout: String::new(),
                    stderr: String::new(),
                }
            }
        }
    }

    fn summary(id: &str, cwd: &str) -> SessionSummary {
        SessionSummary {
            engine: EngineId::new("claude").expect("engine"),
            session_id: id.into(),
            title: id.into(),
            cwd: cwd.into(),
            status: SessionStatus::Idle,
            cloud: false,
            model: None,
            effort: None,
            last_activity: "2026-08-27T00:00:00.000Z".into(),
            first_seen: "2026-08-27T00:00:00.000Z".into(),
            archived: false,
            size: None,
            title_from_summary: None,
            origin: Some(SessionOrigin::Store),
            project_root: None,
            worktree_home: None,
            worktree_path: None,
            setting_sources: None,
            disallowed_tools: None,
            kind: None,
            permission_mode: None,
        }
    }

    #[test]
    fn durable_provider_and_worktree_roots_seed_without_lookup() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let mut registry = SessionRegistry::open(temporary.path()).expect("registry");
        let engine = EngineId::new("claude").expect("engine");
        let mut durable = summary("durable", "/repo/nested");
        durable.project_root = Some("/repo".into());
        registry
            .upsert_all(registry.revision(), &engine, [durable])
            .expect("seed");
        let git = Arc::new(ScriptedGit::new([]));
        let projector = ProjectRootProjector::new(git.clone());
        let mut rows = vec![summary("durable", "/repo/nested")];
        assert!(projector.prepare(&registry, &engine, &mut rows).is_empty());
        assert_eq!(rows[0].project_root.as_deref(), Some("/repo"));

        let mut supplied = summary("provider", "/provider");
        supplied.project_root = Some("/provider-root".into());
        assert!(
            projector
                .prepare(&registry, &engine, std::slice::from_mut(&mut supplied))
                .is_empty()
        );
        assert_eq!(supplied.project_root.as_deref(), Some("/provider-root"));

        let mut registered_worktree = summary("registered-worktree", "/repo");
        registered_worktree.project_root = Some("/repo".into());
        registered_worktree.worktree_home = Some("/repo".into());
        registered_worktree.worktree_path = Some("/checkout".into());
        registry
            .upsert_all(registry.revision(), &engine, [registered_worktree])
            .expect("registered worktree");
        let mut switched = summary("registered-worktree", "/checkout");
        assert!(
            projector
                .prepare(&registry, &engine, std::slice::from_mut(&mut switched))
                .is_empty()
        );
        assert_eq!(switched.project_root.as_deref(), Some("/repo"));
        let mut moved = summary("registered-worktree", "/different/repository");
        assert_eq!(
            projector.prepare(&registry, &engine, std::slice::from_mut(&mut moved)),
            vec!["/different/repository"]
        );

        let mut worktree = summary("worktree", "/checkout");
        worktree.worktree_home = Some("/repo".into());
        worktree.worktree_path = Some("/checkout".into());
        assert!(
            projector
                .prepare(&registry, &engine, std::slice::from_mut(&mut worktree))
                .is_empty()
        );
        assert!(git.calls.lock().expect("calls").is_empty());
    }

    #[tokio::test]
    async fn distinct_cwd_is_cached_including_non_git_and_failure_answers() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let registry = SessionRegistry::open(temporary.path()).expect("registry");
        let history = crate::EngineHistoryStore::open(
            temporary.path().join("history"),
            crate::HistoryLimits::default(),
        )
        .expect("history");
        let services = RuntimeServices::new(registry, history);
        let engine = EngineId::new("claude").expect("engine");
        let directory = |name: &str| {
            let path = temporary.path().join(name);
            std::fs::create_dir_all(&path).expect("cwd");
            path.to_string_lossy().into_owned()
        };
        let (nested, other, failed) = (
            directory("repo/nested"),
            directory("other"),
            directory("failed"),
        );
        let git = Arc::new(ScriptedGit::new([
            (
                failed.clone(),
                GitResult {
                    code: -1,
                    stdout: String::new(),
                    stderr: "timed out".into(),
                },
            ),
            (
                other.clone(),
                GitResult {
                    code: 1,
                    stdout: String::new(),
                    stderr: "not a repository".into(),
                },
            ),
            (
                nested.clone(),
                GitResult {
                    code: 0,
                    stdout: "/repo\n".into(),
                    stderr: String::new(),
                },
            ),
        ]));
        let projector = Arc::new(ProjectRootProjector::new(git.clone()));
        let mut rows = vec![
            summary("one", &nested),
            summary("two", &nested),
            summary("other", &other),
            summary("failed", &failed),
        ];
        let unresolved = {
            let sessions = services.session_registry();
            let registry = sessions.lock().expect("registry");
            projector.prepare(&registry, &engine, &mut rows)
        };
        {
            let sessions = services.session_registry();
            let mut registry = sessions.lock().expect("registry");
            let revision = registry.revision();
            registry.upsert_all(revision, &engine, rows).expect("rows");
        }
        projector.schedule(&services, unresolved);
        projector.wait_idle().await;

        let sessions = services.session_registry();
        let registry = sessions.lock().expect("registry");
        assert_eq!(
            registry
                .get("claude#one")
                .and_then(|row| row.project_root.as_deref()),
            Some("/repo")
        );
        assert_eq!(
            registry
                .get("claude#two")
                .and_then(|row| row.project_root.as_deref()),
            Some("/repo")
        );
        assert!(
            registry
                .get("claude#other")
                .and_then(|row| row.project_root.as_deref())
                .is_none()
        );
        assert!(
            registry
                .get("claude#failed")
                .and_then(|row| row.project_root.as_deref())
                .is_none()
        );
        assert_eq!(git.calls.lock().expect("calls").len(), 3);
    }

    #[test]
    fn missing_cwd_is_answered_by_a_stat_without_running_git() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let git = ScriptedGit::new([]);
        let missing = temporary.path().join("deleted-worktree");
        let answer = resolve_project_root(&git, &missing.to_string_lossy());
        assert!(answer.root.is_none());
        let not_repository = CachedRoot::negative(NOT_REPOSITORY_TTL);
        assert!(answer.expires > not_repository.expires);
        let file = temporary.path().join("file");
        std::fs::write(&file, "").expect("file");
        assert!(
            resolve_project_root(&git, &file.to_string_lossy())
                .root
                .is_none()
        );
        assert!(git.calls.lock().expect("calls").is_empty());
        // An existing directory still asks git.
        resolve_project_root(&git, &temporary.path().to_string_lossy());
        assert_eq!(git.calls.lock().expect("calls").len(), 1);
    }
}
