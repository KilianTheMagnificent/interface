//! Removing an account keeps its sessions listed as history (spec §6
//! "Removing"): through the next listing, through a restart, and until the
//! owner deletes or archives them in Interface.
use std::{os::unix::fs::PermissionsExt, path::Path};

use interface_contracts::engine::{
    EngineInfo, EngineSessionListing, LaunchOptions, ResumeOptions, SessionSummary,
};
use interface_contracts::ipc::EngineOp;
use interface_engine::{EngineAdapter, EngineError, EngineErrorKind, SessionHandle};
use interface_runtime::SessionRuntime;
use interface_runtime::accounts::{ACCOUNT_REMOVED_MESSAGE, AccountAdapterFactory};
use serde_json::json;

use super::*;

/// One account's store: lists its sessions and deletes from them.
struct StoreAdapter {
    id: EngineId,
    name: String,
    sessions: Mutex<Vec<String>>,
    calls: Arc<Mutex<Vec<String>>>,
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
impl EngineAdapter for StoreAdapter {
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
    fn supported_ops(&self) -> Vec<EngineOp> {
        vec![EngineOp::DeleteSession]
    }
    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
        Ok(EngineSessionListing {
            sessions: lock(&self.sessions).iter().map(|id| summary(id)).collect(),
            complete: true,
            unavailable: None,
            error: None,
        })
    }
    async fn delete_session(
        &self,
        session_id: String,
        _cwd: Option<String>,
    ) -> Result<(), EngineError> {
        lock(&self.calls).push(format!("{}:delete {session_id}", self.name));
        lock(&self.sessions).retain(|kept| kept != &session_id);
        Ok(())
    }
    fn create_session(
        &self,
        _options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            "not launched in these tests",
            false,
        ))
    }
    fn resume_session(
        &self,
        session_id: String,
        _cwd: String,
        _options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        lock(&self.calls).push(format!("{}:resume {session_id}", self.name));
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Unsupported,
            "not resumed in these tests",
            false,
        ))
    }
}

/// The host as it runs: the account stores under `root`, the session
/// registry under `root/registry`, and the Claude router in the runtime.
struct Host {
    accounts: Arc<ProviderAccounts>,
    router: Arc<AccountRoutedAdapter>,
    runtime: SessionRuntime,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Host {
    /// Opens everything at `root` the way the host's startup does, including
    /// its pruning of bindings whose session left the registry.
    fn start(root: &Path) -> Self {
        let registry_root = root.join("registry");
        if !registry_root.exists() {
            std::fs::create_dir(&registry_root).unwrap();
            std::fs::set_permissions(&registry_root, std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let sessions = interface_registry::SessionRegistry::open(&registry_root).expect("registry");
        let accounts = crate::bootstrap::open_provider_accounts(root, &sessions, true)
            .expect("account stores");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let engine = EngineId::new("claude").unwrap();
        let default: Arc<dyn EngineAdapter> = Arc::new(StoreAdapter {
            id: engine.clone(),
            name: "Default".into(),
            sessions: Mutex::new(vec!["home-1".into()]),
            calls: Arc::clone(&calls),
        });
        let factory_calls = Arc::clone(&calls);
        let factory: AccountAdapterFactory = Arc::new(move |profile| {
            Ok(Arc::new(StoreAdapter {
                id: engine.clone(),
                name: profile.label.clone(),
                sessions: Mutex::new(vec!["work-1".into()]),
                calls: Arc::clone(&factory_calls),
            }) as Arc<dyn EngineAdapter>)
        });
        let directory: Arc<dyn AccountDirectory> = accounts.clone();
        let router = Arc::new(AccountRoutedAdapter::new(default, factory, directory));
        accounts.install_router(AccountProvider::Claude, Arc::clone(&router));
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
        Self {
            accounts,
            router,
            runtime,
            calls,
        }
    }

    /// Quits: every lease is released, as when the app exits.
    async fn quit(self) {
        self.runtime.shutdown().await.expect("shutdown");
        // The accounts hold the router, which holds the accounts.
        lock(&self.accounts.routers).clear();
    }

    async fn command(&self, command: Value) -> ProviderAccountsView {
        ProviderAccountsHost::new(Arc::clone(&self.accounts))
            .dispatch(CHANNELS.provider_accounts, command)
            .await
            .map(|value| serde_json::from_value(value).expect("view"))
            .expect("command")
    }

    async fn add_work(&self) -> String {
        let view = self
            .command(json!({"command": "add", "provider": "claude", "label": "Work"}))
            .await;
        view.providers[0].accounts[1].id.clone()
    }

    async fn remove(&self, account: &str) {
        self.command(json!({"command": "remove", "provider": "claude", "account": account}))
            .await;
    }

    async fn listed(&self) -> Vec<String> {
        let mut ids = self
            .runtime
            .list_managed_sessions()
            .await
            .unwrap()
            .into_iter()
            .map(|session| session.session_id)
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }
}

fn private_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    root
}

#[tokio::test]
async fn a_removed_accounts_sessions_stay_listed_after_the_next_listing() {
    let root = private_root();
    let host = Host::start(root.path());
    let work = host.add_work().await;
    assert_eq!(host.listed().await, ["home-1", "work-1"]);
    host.remove(&work).await;
    assert_eq!(
        host.listed().await,
        ["home-1", "work-1"],
        "a removed account's session stays listed as history"
    );
    assert_eq!(
        host.accounts.account_of_key("claude#work-1").as_deref(),
        Some(work.as_str()),
        "and still reads as that (removed) account"
    );
}

#[tokio::test]
async fn a_removed_accounts_sessions_stay_listed_and_bound_after_a_restart() {
    let root = private_root();
    let host = Host::start(root.path());
    let work = host.add_work().await;
    host.listed().await;
    host.remove(&work).await;
    host.listed().await;
    host.quit().await;

    let host = Host::start(root.path());
    assert_eq!(
        host.accounts.account_of_key("claude#work-1").as_deref(),
        Some(work.as_str()),
        "startup keeps the binding of a session that is still listed"
    );
    assert_eq!(host.listed().await, ["home-1", "work-1"]);
    let refused = host
        .router
        .resume_session("work-1".into(), "/w".into(), ResumeOptions::default())
        .err()
        .expect("refused");
    assert_eq!(refused.message, ACCOUNT_REMOVED_MESSAGE);
    assert!(refused.replay_is_safe());
}

#[tokio::test]
async fn a_removed_accounts_session_can_be_deleted_and_does_not_come_back() {
    let root = private_root();
    let host = Host::start(root.path());
    let work = host.add_work().await;
    host.listed().await;
    host.remove(&work).await;
    host.listed().await;

    host.runtime
        .delete_stored_session("claude#work-1")
        .await
        .expect("the owner can delete it");
    assert_eq!(host.listed().await, ["home-1"]);
    assert_eq!(host.accounts.account_of_key("claude#work-1"), None);
    assert!(
        lock(&host.calls).is_empty(),
        "no provider store was touched: {:?}",
        lock(&host.calls)
    );
    // Not back at the next listing, nor after a restart.
    assert_eq!(host.listed().await, ["home-1"]);
    host.quit().await;
    let host = Host::start(root.path());
    assert_eq!(host.listed().await, ["home-1"]);
}

#[tokio::test]
async fn a_removed_accounts_session_can_be_archived_and_restored_in_interface() {
    let root = private_root();
    let host = Host::start(root.path());
    let work = host.add_work().await;
    host.listed().await;
    host.remove(&work).await;
    host.listed().await;

    host.runtime
        .archive_session("claude#work-1", true)
        .await
        .expect("archived");
    assert_eq!(host.listed().await, ["home-1"]);
    let archived = host.runtime.list_archived_sessions().await.unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].session_id, "work-1");
    host.runtime
        .archive_session("claude#work-1", false)
        .await
        .expect("restored");
    assert_eq!(host.listed().await, ["home-1", "work-1"]);
}
