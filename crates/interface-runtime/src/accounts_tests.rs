use std::collections::BTreeMap;

use interface_contracts::engine::{SessionSummary, UsageWindow};
use interface_contracts::ipc::HistoryPage;
use interface_engine::bounded_event_stream;

use super::*;

/// One account's adapter: answers with its own name so a test can tell
/// which account a call reached.
struct NamedAdapter {
    id: EngineId,
    name: String,
    sessions: Vec<String>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl NamedAdapter {
    fn new(name: &str, sessions: &[&str], calls: &Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            id: EngineId::new("claude").unwrap(),
            name: name.to_owned(),
            sessions: sessions.iter().map(|id| (*id).to_owned()).collect(),
            calls: Arc::clone(calls),
        }
    }

    fn record(&self, call: &str) {
        lock(&self.calls).push(format!("{}:{call}", self.name));
    }
}

struct ReadyHandle {
    engine: EngineId,
    id: String,
}

#[async_trait]
impl SessionHandle for ReadyHandle {
    fn engine(&self) -> &EngineId {
        &self.engine
    }
    fn session_id(&self) -> String {
        self.id.clone()
    }
    async fn ready(&self) -> Result<ReadySession, EngineError> {
        Ok(ReadySession {
            session_id: self.id.clone(),
        })
    }
    fn take_events(&mut self) -> Result<BoundedEventStream<EngineEvent>, EngineError> {
        Ok(bounded_event_stream(1).1)
    }
    async fn send(&self, _text: String) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }
    async fn interrupt(&self) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }
    async fn close(&self) -> Result<(), EngineError> {
        Ok(())
    }
    async fn respond_to_approval(
        &self,
        _approval_id: String,
        _verdict: ApprovalVerdict,
        _response: ApprovalResponse,
    ) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }
}

fn summary(id: &str) -> SessionSummary {
    serde_json::from_value(serde_json::json!({
        "engine": "claude",
        "sessionId": id,
        "title": id,
        "cwd": "/w",
        "status": "idle",
        "cloud": false,
        "lastActivity": "2026-11-15T00:00:00.000Z",
        "firstSeen": "2026-11-15T00:00:00.000Z",
        "archived": false,
    }))
    .expect("summary")
}

#[async_trait]
impl EngineAdapter for NamedAdapter {
    fn id(&self) -> &EngineId {
        &self.id
    }
    async fn probe(&self) -> Result<EngineInfo, EngineError> {
        self.record("probe");
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Unavailable,
            self.name.clone(),
            false,
        ))
    }
    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
        self.record("list");
        Ok(EngineSessionListing {
            sessions: self.sessions.iter().map(|id| summary(id)).collect(),
            complete: true,
            unavailable: None,
            error: None,
        })
    }
    async fn read_usage(&self) -> Result<EngineUsage, EngineError> {
        Ok(EngineUsage {
            engine: self.id.clone(),
            available: true,
            plan: Some(self.name.clone()),
            windows: vec![UsageWindow {
                label: "5h".into(),
                utilization: Some(10.0),
                resets_at: None,
            }],
            cost_usd: None,
            note: None,
            account: None,
            account_label: None,
        })
    }
    async fn read_history(
        &self,
        session_id: String,
        _request: HistoryRequest,
    ) -> Result<HistoryPage, EngineError> {
        self.record(&format!("history {session_id}"));
        Ok(unavailable_history(self.name.clone()))
    }
    async fn fork_session(
        &self,
        session_id: String,
        _cwd: Option<String>,
    ) -> Result<String, EngineError> {
        self.record(&format!("fork {session_id}"));
        Ok(format!("{session_id}-fork"))
    }
    fn create_session(
        &self,
        options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        assert_eq!(
            options.account, None,
            "the account never reaches the CLI adapter"
        );
        self.record("create");
        Ok(Box::new(ReadyHandle {
            engine: self.id.clone(),
            id: format!("{}-new", self.name),
        }))
    }
    fn resume_session(
        &self,
        session_id: String,
        _cwd: String,
        _options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        self.record(&format!("resume {session_id}"));
        Ok(Box::new(ReadyHandle {
            engine: self.id.clone(),
            id: session_id,
        }))
    }
}

#[derive(Default)]
struct Directory {
    default: Mutex<String>,
    accounts: Mutex<Vec<AccountProfile>>,
    bound: Mutex<BTreeMap<String, String>>,
}

impl AccountDirectory for Directory {
    fn default_account(&self, _engine: &EngineId) -> String {
        let default = lock(&self.default).clone();
        if default.is_empty() {
            DEFAULT_ACCOUNT_ID.into()
        } else {
            default
        }
    }
    fn profile(&self, _engine: &EngineId, account: &str) -> Option<AccountProfile> {
        lock(&self.accounts)
            .iter()
            .find(|profile| profile.id == account)
            .cloned()
    }
    fn profiles(&self, _engine: &EngineId) -> Vec<AccountProfile> {
        lock(&self.accounts).clone()
    }
    fn bound(&self, _engine: &EngineId, session_id: &str) -> Option<String> {
        lock(&self.bound).get(session_id).cloned()
    }
    fn bind(&self, _engine: &EngineId, bindings: Vec<(String, String)>, replace: bool) {
        let mut bound = lock(&self.bound);
        for (session, account) in bindings {
            if replace || !bound.contains_key(&session) {
                bound.insert(session, account);
            }
        }
    }
}

fn work() -> AccountProfile {
    AccountProfile {
        id: "acct-work".into(),
        label: "Work".into(),
        directory: PathBuf::from("/data/accounts/claude/acct-work"),
    }
}

/// The router, its directory, the calls log and how many account adapters
/// were built.
type Routed = (
    AccountRoutedAdapter,
    Arc<Directory>,
    Arc<Mutex<Vec<String>>>,
    Arc<Mutex<usize>>,
);

fn router(default_sessions: &[&str], work_sessions: &[&'static str]) -> Routed {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let built = Arc::new(Mutex::new(0));
    let directory = Arc::new(Directory::default());
    let default: Arc<dyn EngineAdapter> =
        Arc::new(NamedAdapter::new("default", default_sessions, &calls));
    let factory_calls = Arc::clone(&calls);
    let factory_built = Arc::clone(&built);
    let work_sessions = work_sessions.to_vec();
    let factory: AccountAdapterFactory = Arc::new(move |profile: &AccountProfile| {
        *lock(&factory_built) += 1;
        let adapter: Arc<dyn EngineAdapter> = Arc::new(NamedAdapter::new(
            &profile.label,
            &work_sessions,
            &factory_calls,
        ));
        Ok(adapter)
    });
    let directory_port: Arc<dyn AccountDirectory> = directory.clone();
    (
        AccountRoutedAdapter::new(default, factory, directory_port),
        directory,
        calls,
        built,
    )
}

fn calls(calls: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    std::mem::take(&mut *lock(calls))
}

#[tokio::test]
async fn with_no_added_accounts_every_call_reaches_the_default_adapter_unchanged() {
    let (router, _directory, log, built) = router(&["a"], &[]);
    let handle = router
        .create_session(LaunchOptions::default())
        .expect("create");
    assert_eq!(handle.ready().await.unwrap().session_id, "default-new");
    let listing = router.list_sessions().await.unwrap();
    assert_eq!(listing.sessions.len(), 1);
    router
        .resume_session("a".into(), "/w".into(), ResumeOptions::default())
        .unwrap();
    let rows = router.read_usage_rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].as_ref().unwrap().account, None);
    assert_eq!(
        calls(&log),
        ["default:create", "default:list", "default:resume a"]
    );
    assert_eq!(*lock(&built), 0, "no account adapter is ever built");
}

#[tokio::test]
async fn a_launch_on_an_account_binds_its_session_and_later_calls_follow_the_binding() {
    let (router, directory, log, built) = router(&[], &[]);
    lock(&directory.accounts).push(work());
    let handle = router
        .create_session(LaunchOptions {
            account: Some("acct-work".into()),
            ..LaunchOptions::default()
        })
        .expect("create");
    assert_eq!(handle.ready().await.unwrap().session_id, "Work-new");
    assert_eq!(
        directory.bound(router.id(), "Work-new").as_deref(),
        Some("acct-work")
    );
    router
        .resume_session("Work-new".into(), "/w".into(), ResumeOptions::default())
        .unwrap();
    let page = router
        .read_history(
            "Work-new".into(),
            HistoryRequest {
                cwd: "/w".into(),
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.reason.as_deref(), Some("Work"));
    let forked = router.fork_session("Work-new".into(), None).await.unwrap();
    assert_eq!(
        directory.bound(router.id(), &forked).as_deref(),
        Some("acct-work"),
        "a fork stays on its account"
    );
    // An unbound session is the Default account's.
    router
        .resume_session("old".into(), "/w".into(), ResumeOptions::default())
        .unwrap();
    assert_eq!(
        calls(&log),
        [
            "Work:create",
            "Work:resume Work-new",
            "Work:history Work-new",
            "Work:fork Work-new",
            "default:resume old",
        ]
    );
    assert_eq!(*lock(&built), 1, "one adapter per account, built once");
}

#[tokio::test]
async fn a_launch_naming_no_account_uses_the_providers_default_account() {
    let (router, directory, log, _built) = router(&[], &[]);
    lock(&directory.accounts).push(work());
    *lock(&directory.default) = "acct-work".into();
    let handle = router.create_session(LaunchOptions::default()).unwrap();
    handle.ready().await.unwrap();
    let explicit = router
        .create_session(LaunchOptions {
            account: Some(DEFAULT_ACCOUNT_ID.into()),
            ..LaunchOptions::default()
        })
        .unwrap();
    explicit.ready().await.unwrap();
    assert_eq!(calls(&log), ["Work:create", "default:create"]);
    assert_eq!(directory.bound(router.id(), "default-new"), None);
    let unknown = router
        .create_session(LaunchOptions {
            account: Some("acct-gone".into()),
            ..LaunchOptions::default()
        })
        .err()
        .expect("an unknown account starts nothing");
    assert!(unknown.replay_is_safe());
}

#[tokio::test]
async fn listings_merge_every_account_and_bind_what_an_account_store_holds() {
    let (router, directory, _log, _built) = router(&["home-1"], &["work-1", "work-2"]);
    lock(&directory.accounts).push(work());
    let listing = router.list_sessions().await.unwrap();
    let mut ids = listing
        .sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["home-1", "work-1", "work-2"]);
    assert!(listing.complete);
    assert_eq!(
        directory.bound(router.id(), "work-2").as_deref(),
        Some("acct-work")
    );
    assert_eq!(directory.bound(router.id(), "home-1"), None);
}

#[tokio::test]
async fn usage_has_a_row_per_account_under_its_label() {
    let (router, directory, _log, _built) = router(&[], &[]);
    lock(&directory.accounts).push(work());
    let rows = router.read_usage_rows().await;
    assert_eq!(rows.len(), 2);
    let default = rows[0].as_ref().unwrap();
    assert_eq!(default.plan.as_deref(), Some("default"));
    assert_eq!(default.account, None);
    let account = rows[1].as_ref().unwrap();
    assert_eq!(account.plan.as_deref(), Some("Work"));
    assert_eq!(account.account.as_deref(), Some("acct-work"));
    assert_eq!(account.account_label.as_deref(), Some("Work"));
}

#[tokio::test]
async fn a_session_whose_account_was_removed_is_history_that_cannot_be_resumed() {
    let (router, directory, log, _built) = router(&[], &[]);
    lock(&directory.bound).insert("orphan".into(), "acct-gone".into());
    let refused = router
        .resume_session("orphan".into(), "/w".into(), ResumeOptions::default())
        .err()
        .expect("refused");
    assert_eq!(refused.message, ACCOUNT_REMOVED_MESSAGE);
    assert!(refused.replay_is_safe(), "nothing was started");
    let page = router
        .read_history(
            "orphan".into(),
            HistoryRequest {
                cwd: "/w".into(),
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.reason.as_deref(), Some(ACCOUNT_REMOVED_MESSAGE));
    assert!(router.fork_session("orphan".into(), None).await.is_err());
    assert!(calls(&log).is_empty(), "no account adapter was asked");
}
