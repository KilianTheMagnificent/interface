//! Managed worktrees run none of the programs a repository's own git
//! settings name: a project folder can arrive with its own `.git`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use interface_registry::{CreateWorktreeOptions, SystemGit, create_worktree, remove_worktree};
use tempfile::TempDir;

struct Folder {
    _temp: TempDir,
    root: PathBuf,
}

impl Folder {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary folder");
        let root = fs::canonicalize(temp.path()).expect("canonical folder");
        Self { _temp: temp, root }
    }

    /// A program that records that it ran; a filter passes its input through.
    fn program(&self, label: &str) -> String {
        let script = self.root.join(label);
        let tail = if label.starts_with("filter") {
            "cat"
        } else {
            "exit 1"
        };
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho {label} >> '{}'\n{tail}\n",
                self.root.join("marks").display()
            ),
        )
        .expect("program");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("executable");
        script.display().to_string()
    }

    fn marks(&self) -> String {
        fs::read_to_string(self.root.join("marks"))
            .unwrap_or_default()
            .trim()
            .replace('\n', ", ")
    }
}

fn git(dir: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(name);
        }
    }
    let output = command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be available");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository(folder: &Folder) -> PathBuf {
    let repo = folder.root.join("project");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["config", "user.name", "Interface Test"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    fs::write(repo.join("a.txt"), "hello\n").unwrap();
    fs::write(repo.join(".gitattributes"), "* filter=x\n").unwrap();
    repo
}

#[test]
fn removing_a_managed_worktree_never_runs_a_filter_the_repository_defines() {
    let folder = Folder::new();
    let repo = repository(&folder);
    git(&repo, &["add", "--all"]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let mut options = CreateWorktreeOptions::new(&repo);
    options.base = Some(folder.root.join("worktrees"));
    let worktree = create_worktree(&options, &SystemGit).expect("worktree");
    git(
        &repo,
        &["config", "filter.x.clean", &folder.program("filter-clean")],
    );
    git(&repo, &["config", "filter.x.required", "true"]);
    // Same bytes, new stat data: git re-reads the file through its filter.
    fs::write(worktree.path.join("a.txt"), "hello\n").unwrap();

    let removed = remove_worktree(&worktree, &SystemGit, false);

    assert_eq!(folder.marks(), "", "removal ran the repository's filter");
    assert!(removed.removed, "{removed:?}");
}

#[test]
fn the_first_commit_interface_makes_never_runs_the_repositorys_signing_program() {
    let folder = Folder::new();
    let repo = repository(&folder);
    git(&repo, &["config", "commit.gpgsign", "true"]);
    git(&repo, &["config", "gpg.format", "ssh"]);
    let key = folder.root.join("missing-key");
    git(&repo, &["config", "user.signingkey", key.to_str().unwrap()]);
    git(
        &repo,
        &[
            "config",
            "gpg.ssh.program",
            &folder.program("signing-program"),
        ],
    );
    let mut options = CreateWorktreeOptions::new(&repo);
    options.base = Some(folder.root.join("worktrees"));
    options.init_if_needed = true;

    // Signing with the owner's own `ssh-keygen` fails on the missing key.
    let _ = create_worktree(&options, &SystemGit);

    assert_eq!(
        folder.marks(),
        "",
        "the first commit ran the repository's signing program"
    );
}
