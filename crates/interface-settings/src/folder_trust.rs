//! Which folder a session's trust is decided for, and what Claude Code
//! itself already records about it.
//!
//! Run headless, the Claude CLI asks no trust question: it loads a folder's
//! `.claude/settings*.json` (hooks that run commands, allow rules, MCP
//! servers) and its instructions at once. Interface therefore decides, before
//! each launch, whether the folder may contribute anything. The decision is
//! made for the folder's *trust root*: the repository a working folder
//! belongs to, so one answer covers every subfolder and every git worktree
//! of a project, and nothing else.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use interface_common::sync::lock;
use serde::Deserialize;

const MAX_ANCESTORS: usize = 64;
const MAX_POINTER_BYTES: u64 = 4 * 1024;
const MAX_CLAUDE_CONFIG_BYTES: u64 = 32 * 1024 * 1024;

/// The folder whose trust covers a session started in `cwd`.
///
/// - Inside a git repository: the repository's top folder.
/// - Inside a linked git worktree: the main repository's top folder, but
///   only when that repository's own `.git/worktrees/<name>/gitdir` names
///   this worktree back. A folder cannot claim to be a worktree of a
///   repository it was never added to: only git, run in that repository,
///   writes the entry.
/// - Anywhere else: `cwd` itself.
///
/// A repository found at or above `home` never covers a deeper folder: a
/// home folder kept under git would otherwise make everything in it one
/// project.
///
/// `None` when `cwd` is not a folder that exists.
#[must_use]
pub fn trust_root(cwd: &Path, home: &Path) -> Option<PathBuf> {
    let cwd = fs::canonicalize(cwd).ok()?;
    if !fs::metadata(&cwd).ok()?.is_dir() {
        return None;
    }
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    for folder in cwd.ancestors().take(MAX_ANCESTORS) {
        let pointer = folder.join(".git");
        let Ok(metadata) = fs::symlink_metadata(&pointer) else {
            continue;
        };
        let root = if metadata.is_file() {
            main_repository_of(folder, &pointer).unwrap_or_else(|| folder.to_path_buf())
        } else {
            // A real `.git` folder; a linked `.git` is taken at its word for
            // this folder only, never followed.
            folder.to_path_buf()
        };
        if folder != cwd && home.starts_with(folder) {
            return Some(cwd);
        }
        return Some(root);
    }
    Some(cwd)
}

/// The top folder of the repository a linked worktree at `folder` belongs
/// to, when that repository vouches for it.
fn main_repository_of(folder: &Path, pointer: &Path) -> Option<PathBuf> {
    let text = read_small(pointer)?;
    let target = text.strip_prefix("gitdir:")?.trim();
    let entry = fs::canonicalize(folder.join(target)).ok()?;
    // `<main>/.git/worktrees/<name>/gitdir` holds the path of this
    // worktree's `.git` file. The entry lives inside the main repository,
    // where only its owner can write.
    let named_back = fs::canonicalize(read_small(&entry.join("gitdir"))?.trim()).ok()?;
    if named_back != fs::canonicalize(pointer).ok()? {
        return None;
    }
    let common = fs::canonicalize(entry.join(read_small(&entry.join("commondir"))?.trim())).ok()?;
    let worktrees = entry.parent()?;
    if worktrees.file_name()? != "worktrees"
        || worktrees.parent()? != common
        || common.file_name()? != ".git"
        || !fs::symlink_metadata(&common).ok()?.is_dir()
    {
        return None;
    }
    Some(common.parent()?.to_path_buf())
}

fn read_small(path: &Path) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_POINTER_BYTES {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_POINTER_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

#[derive(Deserialize)]
struct ClaudeConfig {
    #[serde(default)]
    projects: HashMap<String, ClaudeProject>,
}

#[derive(Deserialize)]
struct ClaudeProject {
    #[serde(default, rename = "hasTrustDialogAccepted")]
    trusted: bool,
}

/// The folders a `.claude.json` records as trusted; `None` when the file is
/// unreadable, oversized or malformed.
fn trusted_roots(config: &Path) -> Option<HashSet<String>> {
    let metadata = fs::metadata(config).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_CLAUDE_CONFIG_BYTES {
        return None;
    }
    let bytes = fs::read(config).ok()?;
    let parsed = serde_json::from_slice::<ClaudeConfig>(&bytes).ok()?;
    Some(
        parsed
            .projects
            .into_iter()
            .filter_map(|(folder, project)| project.trusted.then_some(folder))
            .collect(),
    )
}

/// Whether the owner already answered Claude Code's own "Do you trust the
/// files in this folder?" for exactly `root`, in the account whose
/// `.claude.json` is at `config`.
///
/// Exactly `root`: a trusted parent says nothing about a folder inside it.
/// An unreadable, oversized or malformed file answers `false`.
#[must_use]
pub fn claude_code_trusts(config: &Path, root: &Path) -> bool {
    let Some(root) = root.to_str() else {
        return false;
    };
    trusted_roots(config).is_some_and(|trusted| trusted.contains(root))
}

/// The `.claude.json` Claude Code keeps its global settings and its trust
/// answers in, for an account whose configuration folder is `config_dir`
/// (`CLAUDE_CONFIG_DIR`; `None` for the Default account without one).
///
/// Claude Code reads a legacy `.config.json` in its configuration folder
/// (`~/.claude` by default) first when one exists; otherwise
/// `$CLAUDE_CONFIG_DIR/.claude.json`, or `~/.claude.json` when the variable
/// is unset. A wrong guess only ever means "not trusted here".
#[must_use]
pub fn claude_config_file(config_dir: Option<&Path>, home: &Path) -> PathBuf {
    let folder = config_dir.map_or_else(|| home.join(".claude"), Path::to_path_buf);
    let legacy = folder.join(".config.json");
    if fs::symlink_metadata(&legacy).is_ok_and(|metadata| metadata.is_file()) {
        return legacy;
    }
    config_dir.map_or_else(|| home.join(".claude.json"), |dir| dir.join(".claude.json"))
}

/// How many `.claude.json` files [`ClaudeTrustCache`] keeps parsed.
const CACHED_CONFIGS: usize = 16;

struct CachedConfig {
    modified: Option<SystemTime>,
    len: u64,
    trusted: Arc<HashSet<String>>,
}

/// [`claude_code_trusts`] that parses each `.claude.json` once per change:
/// a file is read again only when its modification time or size moved, so
/// a thread list asking for every thread costs one `stat` per thread, never
/// a parse of a large file.
#[derive(Default)]
pub struct ClaudeTrustCache {
    configs: Mutex<HashMap<PathBuf, CachedConfig>>,
}

impl ClaudeTrustCache {
    /// Whether `config` records Claude Code's own trust for exactly `root`.
    /// Fails closed as [`claude_code_trusts`] does.
    #[must_use]
    pub fn trusts(&self, config: &Path, root: &Path) -> bool {
        let Some(root) = root.to_str() else {
            return false;
        };
        self.trusted(config)
            .is_some_and(|trusted| trusted.contains(root))
    }

    fn trusted(&self, config: &Path) -> Option<Arc<HashSet<String>>> {
        let Ok(metadata) = fs::metadata(config) else {
            lock(&self.configs).remove(config);
            return None;
        };
        let modified = metadata.modified().ok();
        let len = metadata.len();
        if let Some(cached) = lock(&self.configs).get(config)
            && cached.modified.is_some()
            && cached.modified == modified
            && cached.len == len
        {
            return Some(Arc::clone(&cached.trusted));
        }
        let Some(trusted) = trusted_roots(config).map(Arc::new) else {
            lock(&self.configs).remove(config);
            return None;
        };
        // Kept only when the file did not change while it was read.
        let after = fs::metadata(config).ok();
        if after
            .as_ref()
            .map(|after| (after.modified().ok(), after.len()))
            == Some((modified, len))
        {
            let mut configs = lock(&self.configs);
            if configs.len() >= CACHED_CONFIGS && !configs.contains_key(config) {
                configs.clear();
            }
            configs.insert(
                config.to_path_buf(),
                CachedConfig {
                    modified,
                    len,
                    trusted: Arc::clone(&trusted),
                },
            );
        }
        Some(trusted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(folder: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .current_dir(folder)
            .args(arguments)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.test")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.test")
            .output()
            .expect("git runs");
        assert!(status.status.success(), "git {arguments:?}: {status:?}");
    }

    /// A repository with one commit and one linked worktree beside it.
    fn project(root: &Path) -> (PathBuf, PathBuf) {
        let repository = root.join("project");
        let worktree = root.join("worktree");
        fs::create_dir_all(repository.join("src/deep")).unwrap();
        git(&repository, &["init", "-q"]);
        fs::write(repository.join("src/deep/file"), "x").unwrap();
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "-q", "-m", "one"]);
        git(
            &repository,
            &[
                "worktree",
                "add",
                "-q",
                worktree.to_str().unwrap(),
                "-b",
                "side",
            ],
        );
        (
            fs::canonicalize(repository).unwrap(),
            fs::canonicalize(worktree).unwrap(),
        )
    }

    fn sandbox() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir(&home).unwrap();
        (temporary, home)
    }

    #[test]
    fn a_repository_its_subfolders_and_its_worktrees_share_one_root() {
        let (temporary, home) = sandbox();
        let (repository, worktree) = project(temporary.path());
        for inside in [
            repository.clone(),
            repository.join("src"),
            repository.join("src/deep"),
            worktree.clone(),
            worktree.join("src/deep"),
        ] {
            assert_eq!(
                trust_root(&inside, &home).as_deref(),
                Some(repository.as_path()),
                "{inside:?}"
            );
        }
    }

    #[test]
    fn a_folder_outside_any_repository_is_its_own_root() {
        let (temporary, home) = sandbox();
        let plain = temporary.path().join("unzipped/inner");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(
            trust_root(&plain, &home),
            Some(fs::canonicalize(&plain).unwrap())
        );
        assert_eq!(trust_root(&temporary.path().join("missing"), &home), None);
        let file = temporary.path().join("file");
        fs::write(&file, "x").unwrap();
        assert_eq!(trust_root(&file, &home), None);
    }

    #[test]
    fn a_folder_cannot_claim_to_be_a_worktree_of_a_repository_it_was_never_added_to() {
        let (temporary, home) = sandbox();
        let (repository, worktree) = project(temporary.path());
        let entry = repository.join(".git/worktrees/worktree");
        assert!(
            entry.is_dir(),
            "the worktree's entry in the main repository"
        );

        // Pointing at a real worktree's entry: that entry names another folder.
        let borrowing = temporary.path().join("borrowing");
        fs::create_dir(&borrowing).unwrap();
        fs::write(
            borrowing.join(".git"),
            format!("gitdir: {}\n", entry.display()),
        )
        .unwrap();
        assert_eq!(
            trust_root(&borrowing, &home),
            Some(fs::canonicalize(&borrowing).unwrap())
        );

        // Forging the whole entry inside itself: it is not in the repository.
        let forging = temporary.path().join("forging");
        let forged = forging.join("fake/worktrees/x");
        fs::create_dir_all(&forged).unwrap();
        fs::write(
            forging.join(".git"),
            format!("gitdir: {}\n", forged.display()),
        )
        .unwrap();
        fs::write(
            forged.join("gitdir"),
            format!("{}\n", forging.join(".git").display()),
        )
        .unwrap();
        fs::write(
            forged.join("commondir"),
            format!("{}\n", repository.join(".git").display()),
        )
        .unwrap();
        assert_eq!(
            trust_root(&forging, &home),
            Some(fs::canonicalize(&forging).unwrap())
        );

        // The genuine worktree is still the repository's.
        assert_eq!(trust_root(&worktree, &home), Some(repository));
    }

    #[test]
    fn a_home_folder_kept_under_git_does_not_make_everything_in_it_one_project() {
        let (temporary, home) = sandbox();
        git(&home, &["init", "-q"]);
        let downloads = home.join("Downloads/some-archive");
        fs::create_dir_all(&downloads).unwrap();
        assert_eq!(
            trust_root(&downloads, &home),
            Some(fs::canonicalize(&downloads).unwrap())
        );
        // Home itself, and a real repository inside it, are unaffected.
        let home = fs::canonicalize(&home).unwrap();
        assert_eq!(trust_root(&home, &home), Some(home.clone()));
        let nested = home.join("code/app");
        fs::create_dir_all(nested.join("src")).unwrap();
        git(&nested, &["init", "-q"]);
        assert_eq!(trust_root(&nested.join("src"), &home), Some(nested));
        drop(temporary);
    }

    #[test]
    fn claude_codes_own_answer_counts_for_exactly_the_folder_it_was_given_for() {
        let temporary = tempfile::tempdir().unwrap();
        let config = temporary.path().join(".claude.json");
        fs::write(
            &config,
            r#"{"numStartups":3,"projects":{
              "/Users/me/app":{"hasTrustDialogAccepted":true,"allowedTools":[],"lastCost":1.5},
              "/Users/me/asked-and-refused":{"hasTrustDialogAccepted":false},
              "/Users/me/never-asked":{"allowedTools":[]},
              "/Users/me":{"hasTrustDialogAccepted":true}}}"#,
        )
        .unwrap();
        assert!(claude_code_trusts(&config, Path::new("/Users/me/app")));
        for untrusted in [
            "/Users/me/asked-and-refused",
            "/Users/me/never-asked",
            "/Users/me/unknown",
            "/Users/me/app/vendor",
            "/Users/me/Downloads/cloned",
        ] {
            assert!(
                !claude_code_trusts(&config, Path::new(untrusted)),
                "{untrusted}"
            );
        }
    }

    #[test]
    fn a_missing_or_damaged_claude_record_trusts_nothing() {
        let temporary = tempfile::tempdir().unwrap();
        let config = temporary.path().join(".claude.json");
        assert!(!claude_code_trusts(&config, Path::new("/Users/me/app")));
        for damaged in [
            "",
            "{ not json",
            r#"{"projects":[]}"#,
            r#"{"projects":{"/Users/me/app":{"hasTrustDialogAccepted":"yes"}}}"#,
        ] {
            fs::write(&config, damaged).unwrap();
            assert!(
                !claude_code_trusts(&config, Path::new("/Users/me/app")),
                "{damaged}"
            );
        }
    }

    #[test]
    fn an_added_accounts_record_is_in_its_own_folder_and_a_legacy_file_wins() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let account = temporary.path().join("accounts/work");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(&account).unwrap();
        assert_eq!(claude_config_file(None, &home), home.join(".claude.json"));
        assert_eq!(
            claude_config_file(Some(&account), &home),
            account.join(".claude.json")
        );
        fs::write(account.join(".config.json"), "{}").unwrap();
        fs::write(home.join(".claude/.config.json"), "{}").unwrap();
        assert_eq!(
            claude_config_file(Some(&account), &home),
            account.join(".config.json")
        );
        assert_eq!(
            claude_config_file(None, &home),
            home.join(".claude/.config.json")
        );
    }

    #[test]
    fn the_cache_answers_as_a_fresh_read_does_after_every_change() {
        let temporary = tempfile::tempdir().unwrap();
        let config = temporary.path().join(".claude.json");
        let cache = ClaudeTrustCache::default();
        let app = Path::new("/Users/me/app");
        assert!(!cache.trusts(&config, app), "no file");
        fs::write(
            &config,
            r#"{"projects":{"/Users/me/app":{"hasTrustDialogAccepted":true}}}"#,
        )
        .unwrap();
        assert!(cache.trusts(&config, app));
        assert!(cache.trusts(&config, app), "from the cache");
        assert!(!cache.trusts(&config, Path::new("/Users/me")));
        fs::write(
            &config,
            r#"{"projects":{"/Users/me/app":{"hasTrustDialogAccepted":false},"/x":{}}}"#,
        )
        .unwrap();
        assert!(!cache.trusts(&config, app), "a changed file is read again");
        fs::write(&config, "{ damaged").unwrap();
        assert!(!cache.trusts(&config, app));
        fs::remove_file(&config).unwrap();
        assert!(!cache.trusts(&config, app));
    }
}
