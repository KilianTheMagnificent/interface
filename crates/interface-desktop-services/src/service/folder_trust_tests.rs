use std::{fs, path::Path, process::Command};

use interface_contracts::ipc::CHANNELS;
use serde_json::json;
use tempfile::TempDir;

use super::{FolderTrustState, TrustedFoldersStore};
use crate::{DesktopService, DesktopServiceConfig, DesktopServiceReviewPorts};

struct Fixture {
    temp: TempDir,
    home: std::path::PathBuf,
    state: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp");
        let home = fs::canonicalize(temp.path())
            .expect("canonical")
            .join("home");
        let state = temp.path().join("state");
        fs::create_dir_all(&home).expect("home");
        Self { temp, home, state }
    }

    fn service(&self) -> DesktopService {
        DesktopService::open(
            DesktopServiceConfig {
                appearance_file: self.state.join("appearance.json"),
                acp_profiles_file: self.state.join("acp-profiles.json"),
                terminal_workspaces_file: self.state.join("terminal-workspaces.json"),
                home_directory: self.home.clone(),
                customization_user_root: None,
                review_checkpoint_store_path: None,
            },
            DesktopServiceReviewPorts::default(),
        )
        .expect("open service")
    }

    fn folder(&self, relative: &str) -> std::path::PathBuf {
        let folder = self.home.join(relative);
        fs::create_dir_all(&folder).expect("folder");
        folder
    }

    /// The Default account's `.claude.json` and an added account's, in
    /// its own folder.
    fn configs(&self) -> (std::path::PathBuf, std::path::PathBuf) {
        let account = self.temp.path().join("accounts/work");
        fs::create_dir_all(&account).expect("account");
        (self.home.join(".claude.json"), account.join(".claude.json"))
    }
}

fn git_init(folder: &Path) {
    let output = Command::new("git")
        .current_dir(folder)
        .args(["init", "-q"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "{output:?}");
}

fn claude_record(config: &Path, trusted: &Path) {
    fs::write(
        config,
        json!({"projects": {trusted.to_str().unwrap(): {"hasTrustDialogAccepted": true}}})
            .to_string(),
    )
    .expect("record");
}

#[test]
fn a_folder_trusted_in_interface_is_trusted_for_every_folder_of_its_repository() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (default, _) = fixture.configs();
    let project = fixture.folder("code/app");
    git_init(&project);
    let deep = fixture.folder("code/app/src/deep");
    let decision = service.folder_trust_of(&deep, Some(&default));
    assert_eq!(decision.state, FolderTrustState::Unasked);
    assert_eq!(decision.root.as_deref(), Some(project.as_path()));

    assert_eq!(service.record_folder_trust(&deep, true).unwrap(), project);
    for folder in [&project, &deep] {
        assert!(service.folder_trust_of(folder, Some(&default)).trusted());
    }
    // A trust survives a restart.
    drop(service);
    let reopened = fixture.service();
    assert!(reopened.folder_trust_of(&deep, Some(&default)).trusted());
}

#[test]
fn claude_codes_own_trust_counts_only_for_the_account_whose_record_holds_it() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (default, added) = fixture.configs();
    let project = fixture.folder("code/app");
    let other = fixture.folder("code/other");
    claude_record(&added, &project);
    claude_record(&default, &other);
    assert!(service.folder_trust_of(&project, Some(&added)).trusted());
    assert!(!service.folder_trust_of(&project, Some(&default)).trusted());
    assert!(service.folder_trust_of(&other, Some(&default)).trusted());
    assert!(!service.folder_trust_of(&other, Some(&added)).trusted());
}

#[test]
fn an_agents_home_and_a_registered_interface_folder_are_trusted_as_interface_owns_them() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (default, _) = fixture.configs();
    let agents = fixture.folder("Interface Agents");
    service.set_agents_root(agents.clone());
    let home = fixture.folder("Interface Agents/Helper");
    assert!(service.folder_trust_of(&home, Some(&default)).trusted());

    let chat = fixture.temp.path().join("state/chat");
    fs::create_dir_all(&chat).unwrap();
    assert!(!service.folder_trust_of(&chat, Some(&default)).trusted());
    service.add_owned_folder(&chat);
    assert!(service.folder_trust_of(&chat, Some(&default)).trusted());
    // A folder beside them is not.
    let beside = fixture.folder("Interface Agents Copy");
    assert!(!service.folder_trust_of(&beside, Some(&default)).trusted());
}

/// An agent can clone anything into its home. That repository is someone
/// else's files, not Interface's, so the home's trust does not reach it.
#[test]
fn a_repository_cloned_into_an_agents_home_is_not_trusted_through_the_home() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (default, _) = fixture.configs();
    service.set_agents_root(fixture.folder("Interface Agents"));
    let home = fixture.folder("Interface Agents/Helper");
    let cloned = fixture.folder("Interface Agents/Helper/cloned-tool");
    git_init(&cloned);
    let inside = fixture.folder("Interface Agents/Helper/cloned-tool/src");
    for folder in [&cloned, &inside] {
        let decision = service.folder_trust_of(folder, Some(&default));
        assert_eq!(decision.state, FolderTrustState::Unasked, "{folder:?}");
        assert_eq!(
            decision.root.as_deref(),
            Some(fs::canonicalize(&cloned).unwrap().as_path())
        );
    }
    assert!(service.folder_trust_of(&home, Some(&default)).trusted());
}

#[test]
fn a_folder_new_project_creates_is_trusted_from_the_start() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (default, _) = fixture.configs();
    let created = service
        .dispatch_blocking(CHANNELS.create_project, json!({"name":"fresh"}))
        .expect("create");
    let created = std::path::PathBuf::from(created.as_str().unwrap());
    assert!(service.folder_trust_of(&created, Some(&default)).trusted());

    // Inside someone's repository, a new folder is the repository's to
    // answer for: nothing is recorded.
    let repository = fixture.folder("Developer/Projects");
    git_init(&repository);
    let inside = service
        .dispatch_blocking(CHANNELS.create_project, json!({"name":"inside"}))
        .expect("create inside");
    let inside = std::path::PathBuf::from(inside.as_str().unwrap());
    assert_eq!(
        service.folder_trust_of(&inside, Some(&default)).state,
        FolderTrustState::Unasked
    );
}

#[test]
fn a_folder_never_seen_declined_or_under_a_trusted_parent_is_not_trusted() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (default, _) = fixture.configs();
    let parent = fixture.folder("code");
    let child = fixture.folder("code/cloned");
    let declined = fixture.folder("Downloads/thing");
    assert_eq!(
        service.folder_trust_of(&child, Some(&default)).state,
        FolderTrustState::Unasked
    );
    service.record_folder_trust(&parent, true).unwrap();
    claude_record(&default, &parent);
    assert!(service.folder_trust_of(&parent, Some(&default)).trusted());
    assert_eq!(
        service.folder_trust_of(&child, Some(&default)).state,
        FolderTrustState::Unasked,
        "a trusted parent says nothing about a folder inside it"
    );
    service.record_folder_trust(&declined, false).unwrap();
    assert_eq!(
        service.folder_trust_of(&declined, Some(&default)).state,
        FolderTrustState::Declined
    );
    // A missing folder has no root and is never trusted.
    let missing = service.folder_trust_of(&fixture.home.join("gone"), Some(&default));
    assert_eq!(missing.root, None);
    assert!(!missing.trusted());
    assert!(
        service
            .record_folder_trust(&fixture.home.join("gone"), true)
            .is_err()
    );
}

#[test]
fn a_subfolder_of_a_home_kept_under_git_is_not_trusted_through_home() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let (default, _) = fixture.configs();
    git_init(&fixture.home);
    service.record_folder_trust(&fixture.home, true).unwrap();
    claude_record(&default, &fixture.home);
    let cloned = fixture.folder("Downloads/cloned");
    let decision = service.folder_trust_of(&cloned, Some(&default));
    assert_eq!(decision.root.as_deref(), Some(cloned.as_path()));
    assert!(!decision.trusted());
}

#[test]
fn an_unreadable_trust_list_trusts_nothing_and_the_service_still_opens() {
    let fixture = Fixture::new();
    let (default, _) = fixture.configs();
    let project = fixture.folder("code/app");
    fs::create_dir_all(&fixture.state).unwrap();
    let file = fixture
        .state
        .join(interface_settings::TRUSTED_FOLDERS_FILE_NAME);
    fs::write(&file, b"{ damaged").unwrap();
    let service = fixture.service();
    assert!(!service.folder_trust_of(&project, Some(&default)).trusted());
    // A store held by another writer: answers are unavailable, still closed.
    drop(service);
    let held = TrustedFoldersStore::open(&file).unwrap();
    let service = fixture.service();
    assert!(!service.folder_trust_of(&project, Some(&default)).trusted());
    let refused = service.record_folder_trust(&project, true).unwrap_err();
    assert_eq!(refused.code, "folder-trust-unavailable");
    drop(held);
}
