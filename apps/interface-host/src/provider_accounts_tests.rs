use std::{os::unix::fs::PermissionsExt, path::PathBuf};

use interface_contracts::engine::{
    EngineEvent, EngineInfo, EngineSessionListing, EngineUsage, LaunchOptions, ResumeOptions,
    SessionSummary, UsageWindow,
};
use interface_contracts::ipc::{HistoryPage, unavailable_history};
use interface_engine::{
    ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome, EngineAdapter,
    EngineError, EngineErrorKind, HistoryRequest, ReadySession, SessionHandle,
    bounded_event_stream,
};
use interface_runtime::accounts::{ACCOUNT_REMOVED_MESSAGE, AccountAdapterFactory};
use interface_runtime::{SessionOpenOutcome, SessionRuntime};
use serde_json::json;

use super::*;

/// One account's adapter, answering with its own name.
struct NamedAdapter {
    id: EngineId,
    name: String,
    sessions: Vec<String>,
    calls: Arc<Mutex<Vec<String>>>,
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
        Ok(bounded_event_stream(4).1)
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
    serde_json::from_value(json!({
        "engine": "claude", "sessionId": id, "title": id, "cwd": "/w", "status": "idle",
        "cloud": false, "lastActivity": "2026-11-15T00:00:00.000Z",
        "firstSeen": "2026-11-15T00:00:00.000Z", "archived": false,
    }))
    .expect("summary")
}

#[async_trait]
impl EngineAdapter for NamedAdapter {
    fn id(&self) -> &EngineId {
        &self.id
    }
    async fn probe(&self) -> Result<EngineInfo, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Unavailable,
            "not probed in these tests",
            false,
        ))
    }
    fn supported_ops(&self) -> Vec<interface_contracts::ipc::EngineOp> {
        vec![interface_contracts::ipc::EngineOp::ReadUsage]
    }
    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
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
                utilization: Some(62.0),
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
        lock(&self.calls).push(format!("{}:history {session_id}", self.name));
        Ok(unavailable_history(self.name.clone()))
    }
    fn create_session(
        &self,
        _options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        lock(&self.calls).push(format!("{}:create", self.name));
        Ok(Box::new(ReadyHandle {
            engine: self.id.clone(),
            id: format!("{}-new", self.name.to_lowercase()),
        }))
    }
    fn resume_session(
        &self,
        session_id: String,
        _cwd: String,
        _options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        lock(&self.calls).push(format!("{}:resume {session_id}", self.name));
        Ok(Box::new(ReadyHandle {
            engine: self.id.clone(),
            id: session_id,
        }))
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    accounts: Arc<ProviderAccounts>,
    router: Arc<AccountRoutedAdapter>,
    runtime: SessionRuntime,
    calls: Arc<Mutex<Vec<String>>>,
    host: ProviderAccountsHost,
}

fn fixture(default_sessions: &[&str], account_sessions: &[&str]) -> Fixture {
    let root = tempfile::tempdir().expect("root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let accounts = ProviderAccounts::open(root.path()).expect("accounts");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let engine = EngineId::new("claude").unwrap();
    let default: Arc<dyn EngineAdapter> = Arc::new(NamedAdapter {
        id: engine.clone(),
        name: "Default".into(),
        sessions: default_sessions.iter().map(|id| (*id).to_owned()).collect(),
        calls: Arc::clone(&calls),
    });
    let factory_calls = Arc::clone(&calls);
    let account_sessions = account_sessions
        .iter()
        .map(|id| (*id).to_owned())
        .collect::<Vec<_>>();
    let factory: AccountAdapterFactory = Arc::new(move |profile| {
        Ok(Arc::new(NamedAdapter {
            id: engine.clone(),
            name: profile.label.clone(),
            sessions: account_sessions.clone(),
            calls: Arc::clone(&factory_calls),
        }) as Arc<dyn EngineAdapter>)
    });
    let directory: Arc<dyn AccountDirectory> = accounts.clone();
    let router = Arc::new(AccountRoutedAdapter::new(default, factory, directory));
    accounts.install_router(AccountProvider::Claude, Arc::clone(&router));
    let registry_root = root.path().join("registry");
    std::fs::create_dir(&registry_root).unwrap();
    std::fs::set_permissions(&registry_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let sessions = interface_registry::SessionRegistry::open(&registry_root).expect("registry");
    let history = interface_runtime::EngineHistoryStore::open(
        registry_root.join("history"),
        interface_runtime::HistoryLimits::default(),
    )
    .expect("history");
    let mut builder =
        SessionRuntime::builder(interface_runtime::RuntimeServices::new(sessions, history));
    builder
        .register_shared_adapter(Arc::clone(&router) as Arc<dyn EngineAdapter>)
        .expect("register");
    let runtime = builder.build().expect("runtime");
    Fixture {
        host: ProviderAccountsHost::new(Arc::clone(&accounts)),
        _root: root,
        accounts,
        router,
        runtime,
        calls,
    }
}

async fn command(fixture: &Fixture, command: Value) -> Result<ProviderAccountsView, DispatchError> {
    fixture
        .host
        .dispatch(CHANNELS.provider_accounts, command)
        .await
        .map(|value| serde_json::from_value(value).expect("view"))
}

async fn add_work(fixture: &Fixture) -> String {
    let view = command(
        fixture,
        json!({"command": "add", "provider": "claude", "label": "Work"}),
    )
    .await
    .expect("add");
    view.providers[0].accounts[1].id.clone()
}

fn history_request() -> HistoryRequest {
    HistoryRequest {
        cwd: "/w".into(),
        limit: 10,
    }
}

#[tokio::test]
async fn a_session_launched_on_an_account_is_bound_and_resumes_and_reads_history_there() {
    let fixture = fixture(&[], &[]);
    let work = add_work(&fixture).await;
    let engine = EngineId::new("claude").unwrap();
    let opened = fixture
        .runtime
        .start_session(
            &engine,
            LaunchOptions {
                cwd: "/w".into(),
                prompt: "hello".into(),
                account: Some(work.clone()),
                ..LaunchOptions::default()
            },
        )
        .await;
    let SessionOpenOutcome::Ready(session) = opened else {
        panic!("the launch did not open");
    };
    assert_eq!(
        session.key, "claude#work-new",
        "the account is not in the key"
    );
    assert_eq!(
        fixture.accounts.account_of_key(&session.key).as_deref(),
        Some(work.as_str())
    );
    fixture
        .router
        .resume_session("work-new".into(), "/w".into(), ResumeOptions::default())
        .expect("resume");
    let page = fixture
        .router
        .read_history("work-new".into(), history_request())
        .await
        .unwrap();
    assert_eq!(page.reason.as_deref(), Some("Work"));
    // A launch that names no account runs on the provider's default account.
    fixture
        .router
        .create_session(LaunchOptions::default())
        .expect("create");
    assert_eq!(
        *lock(&fixture.calls),
        [
            "Work:create",
            "Work:resume work-new",
            "Work:history work-new",
            "Default:create"
        ]
    );
}

#[tokio::test]
async fn listings_merge_both_accounts_and_usage_has_a_row_per_account() {
    let fixture = fixture(&["home-1"], &["work-1"]);
    let work = add_work(&fixture).await;
    let listing = fixture.router.list_sessions().await.unwrap();
    let mut ids = listing
        .sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["home-1", "work-1"]);
    assert_eq!(
        fixture.accounts.account_of_key("claude#work-1").as_deref(),
        Some(work.as_str()),
        "a session found in an account's store is bound to it"
    );
    assert_eq!(fixture.accounts.account_of_key("claude#home-1"), None);

    let usage = fixture.runtime.read_all_usage().await;
    assert_eq!(usage.len(), 2);
    assert_eq!(usage[0].plan.as_deref(), Some("Default"));
    assert_eq!(usage[0].account, None);
    assert_eq!(usage[1].plan.as_deref(), Some("Work"));
    assert_eq!(usage[1].account.as_deref(), Some(work.as_str()));
    assert_eq!(usage[1].account_label.as_deref(), Some("Work"));
}

#[tokio::test]
async fn removing_an_account_leaves_its_sessions_as_history_that_cannot_be_resumed() {
    let fixture = fixture(&[], &["work-1"]);
    let work = add_work(&fixture).await;
    fixture.router.list_sessions().await.unwrap();
    let directory = fixture.accounts.view(None).providers[0].accounts[1]
        .directory
        .clone()
        .expect("directory");
    let view = command(
        &fixture,
        json!({"command": "remove", "provider": "claude", "account": work}),
    )
    .await
    .expect("remove");
    assert_eq!(view.providers[0].accounts.len(), 1);
    assert!(
        view.notice
            .as_deref()
            .is_some_and(|notice| notice.contains("kept")),
        "{:?}",
        view.notice
    );
    assert!(
        Path::new(&directory).is_dir(),
        "kept unless the owner confirms"
    );
    // Still bound, so it stays listed as that account's history.
    assert_eq!(
        fixture.accounts.account_of_key("claude#work-1").as_deref(),
        Some(work.as_str())
    );
    let refused = fixture
        .router
        .resume_session("work-1".into(), "/w".into(), ResumeOptions::default())
        .err()
        .expect("refused");
    assert_eq!(refused.message, ACCOUNT_REMOVED_MESSAGE);
    assert!(refused.replay_is_safe());
    let page = fixture
        .router
        .read_history("work-1".into(), history_request())
        .await
        .unwrap();
    assert_eq!(page.reason.as_deref(), Some(ACCOUNT_REMOVED_MESSAGE));
}

#[tokio::test]
async fn account_commands_keep_the_default_account_and_refuse_bad_requests() {
    let fixture = fixture(&[], &[]);
    let view = command(&fixture, json!({"command": "read"})).await.unwrap();
    assert_eq!(view.providers.len(), 3);
    assert!(view.providers.iter().all(|group| {
        group.accounts.len() == 1
            && group.accounts[0].id == "default"
            && group.accounts[0].is_default
    }));
    let work = add_work(&fixture).await;
    let view = command(
        &fixture,
        json!({"command": "setDefault", "provider": "claude", "account": work}),
    )
    .await
    .unwrap();
    assert!(view.providers[0].accounts[1].is_default);
    assert!(!view.providers[0].accounts[0].is_default);
    let view = command(
        &fixture,
        json!({"command": "rename", "provider": "claude", "account": work, "label": "Work 2"}),
    )
    .await
    .unwrap();
    assert_eq!(view.providers[0].accounts[1].label, "Work 2");
    let refused = command(
        &fixture,
        json!({"command": "remove", "provider": "claude", "account": "default"}),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, REFUSED_CODE);
    assert!(refused.replay_safe);
    let invalid = command(
        &fixture,
        json!({"command": "add", "provider": "qwen", "label": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid.code, INVALID_CODE);
    // Removing the default account makes Default the default again.
    let view = command(
        &fixture,
        json!({"command": "remove", "provider": "claude", "account": work, "deleteDirectory": true}),
    )
    .await
    .unwrap();
    assert!(view.providers[0].accounts[0].is_default);
    assert!(
        view.notice
            .as_deref()
            .is_some_and(|notice| notice.contains("deleted its folder"))
    );
}

#[tokio::test]
async fn terminal_panes_use_the_default_account_and_a_sign_in_names_its_own() {
    let fixture = fixture(&[], &[]);
    let launch = |account: Option<&str>, session: Option<&TerminalLaunchSessionV1>| {
        fixture
            .accounts
            .launch(TerminalLaunchProfileId::Claude, account, session)
            .expect("launch")
    };
    assert_eq!(launch(None, None), None, "Default changes nothing");
    assert_eq!(
        fixture
            .accounts
            .launch(TerminalLaunchProfileId::Shell, None, None)
            .unwrap(),
        None
    );
    let work = add_work(&fixture).await;
    let directory = fixture.accounts.view(None).providers[0].accounts[1]
        .directory
        .clone()
        .unwrap();
    let signed = launch(Some(&work), None).expect("the named account");
    assert_eq!(
        signed.environment,
        [(
            OsString::from("CLAUDE_CONFIG_DIR"),
            OsString::from(&directory)
        )]
    );
    assert_eq!(launch(None, None), None, "still Default until set");
    command(
        &fixture,
        json!({"command": "setDefault", "provider": "claude", "account": work}),
    )
    .await
    .unwrap();
    assert_eq!(launch(None, None), Some(signed.clone()));
    // A reopened pane follows its chat's account, not the current default.
    let chat = "0b1c2d3e-4f50-4617-8293-a4b5c6d7e8f9";
    fixture.accounts.bind(
        &EngineId::new("claude").unwrap(),
        vec![(chat.to_owned(), work.clone())],
        true,
    );
    command(
        &fixture,
        json!({"command": "setDefault", "provider": "claude", "account": "default"}),
    )
    .await
    .unwrap();
    let reopened = TerminalLaunchSessionV1 {
        version: 1,
        id: Some(chat.into()),
        resume: true,
    };
    assert_eq!(launch(None, Some(&reopened)), Some(signed));
    assert_eq!(launch(None, None), None, "a new pane uses Default again");
    assert!(
        fixture
            .accounts
            .launch(TerminalLaunchProfileId::Claude, Some("acct-gone"), None)
            .is_err()
    );
}

#[tokio::test]
async fn a_codex_account_pane_reopens_from_that_accounts_sessions() {
    let fixture = fixture(&[], &[]);
    let view = command(
        &fixture,
        json!({"command": "add", "provider": "codex", "label": "Work"}),
    )
    .await
    .unwrap();
    let account = &view.providers[1].accounts[1];
    let applied = fixture
        .accounts
        .launch(TerminalLaunchProfileId::Codex, Some(&account.id), None)
        .unwrap()
        .unwrap();
    let directory = PathBuf::from(account.directory.clone().unwrap());
    assert_eq!(
        applied.codex_sessions_root,
        Some(directory.join("sessions"))
    );
    assert_eq!(applied.grok_sessions_root, None);
    assert_eq!(applied.environment[0].0, "CODEX_HOME");
}

#[tokio::test]
async fn a_started_thread_runs_on_its_callers_account_on_the_same_provider() {
    let fixture = fixture(&[], &["work-1"]);
    let work = add_work(&fixture).await;
    fixture.router.list_sessions().await.unwrap();
    assert_eq!(
        fixture
            .accounts
            .started_thread_account("claude#work-1", "claude")
            .as_deref(),
        Some(work.as_str())
    );
    assert_eq!(
        fixture
            .accounts
            .started_thread_account("claude#home-1", "claude")
            .as_deref(),
        Some("default")
    );
    assert_eq!(
        fixture
            .accounts
            .started_thread_account("claude#work-1", "codex"),
        None,
        "another provider's thread runs on that provider's default account"
    );
}

/// A provider switch inside a thread (`switchProvider`, a chain of sessions)
/// starts its new session with no account named, so it takes the target
/// provider's default account; switching back resumes the earlier segment,
/// which runs on the account that segment is bound to, whatever the default
/// is now.
#[tokio::test]
async fn a_switched_threads_new_session_takes_the_target_default_and_a_switch_back_its_segments_account()
 {
    let fixture = fixture(&[], &[]);
    let work = add_work(&fixture).await;
    command(
        &fixture,
        json!({"command": "setDefault", "provider": "claude", "account": work}),
    )
    .await
    .unwrap();
    // switch.rs launches the new head with `..LaunchOptions::default()`.
    let handle = fixture
        .router
        .create_session(LaunchOptions {
            cwd: "/w".into(),
            ..LaunchOptions::default()
        })
        .unwrap();
    let head = handle.ready().await.unwrap().session_id;
    assert_eq!(
        fixture
            .accounts
            .account_of_key(&format!("claude#{head}"))
            .as_deref(),
        Some(work.as_str())
    );
    // An earlier Claude segment ran on Default; switching back resumes it there.
    fixture
        .router
        .resume_session("segment-1".into(), "/w".into(), ResumeOptions::default())
        .unwrap();
    // And a segment that ran on Work goes back to Work after the default moved.
    command(
        &fixture,
        json!({"command": "setDefault", "provider": "claude", "account": "default"}),
    )
    .await
    .unwrap();
    fixture
        .router
        .resume_session(head.clone(), "/w".into(), ResumeOptions::default())
        .unwrap();
    assert_eq!(
        *lock(&fixture.calls),
        [
            "Work:create".to_owned(),
            "Default:resume segment-1".to_owned(),
            format!("Work:resume {head}"),
        ]
    );
}
