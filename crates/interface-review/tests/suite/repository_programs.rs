//! A project folder can arrive with its own `.git` (an archive, a copied
//! folder) whose settings name programs. Interface's own git calls (Review,
//! thread checkpoints and restores, the Ship card's reads) run none of them.
//!
//! Each test gives the repository a program that records that it ran, then
//! makes git need it: files whose index stat data no longer matches (as after
//! unpacking an archive) are re-read through the repository's filters.

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use interface_review::read_repository_review;
use interface_review::thread_git::{self, CaptureOutcome};

use crate::{TestDir, temp_dir};

/// A repository with one committed file, `a.txt`, read through a filter
/// named `x`, and a place for the programs it names to leave their marks.
struct Project {
    root: TestDir,
    repo: PathBuf,
}

impl Project {
    fn new() -> Self {
        let root = temp_dir();
        let repo = root.path().join("project");
        fs::create_dir(&repo).expect("project folder");
        let project = Self { root, repo };
        project.git(&["init", "--quiet"]);
        fs::write(project.repo.join("a.txt"), "hello\n").expect("file");
        fs::write(project.repo.join(".gitattributes"), "* filter=x\n").expect("attributes");
        project.git(&["add", "--all"]);
        project.git(&["commit", "--quiet", "-m", "first"]);
        project
    }

    /// Runs setup git in the project, with the owner's hooks and signing off.
    fn git(&self, args: &[&str]) -> String {
        git_in(&self.repo, args)
    }

    fn config(&self, key: &str, value: &str) {
        self.git(&["config", key, value]);
    }

    /// A program that appends `label` to the marks file. A filter passes
    /// its input through; anything else fails at once.
    fn program(&self, label: &str) -> String {
        let tools = self.root.path().join("tools");
        fs::create_dir_all(&tools).expect("tools folder");
        let script = tools.join(label);
        let tail = if label.starts_with("filter") {
            "cat"
        } else {
            "exit 1"
        };
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho {label} >> '{}'\n{tail}\n",
                self.marks_file().display()
            ),
        )
        .expect("program");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("executable");
        script.display().to_string()
    }

    fn marks_file(&self) -> PathBuf {
        self.root.path().join("marks")
    }

    /// What ran since the last call.
    fn marks(&self) -> String {
        let marks = fs::read_to_string(self.marks_file()).unwrap_or_default();
        let _ = fs::remove_file(self.marks_file());
        marks.trim().replace('\n', ", ")
    }

    /// Rewrites every file with the same bytes: the index's stat data no
    /// longer matches, as after unpacking an archive, so git re-reads them.
    fn unpacked(&self) {
        for name in ["a.txt", ".gitattributes"] {
            let path = self.repo.join(name);
            let bytes = fs::read(&path).expect("read back");
            fs::write(&path, bytes).expect("rewrite");
        }
    }
}

fn git_in(dir: &Path, args: &[&str]) -> String {
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
            "-c",
            "user.name=Interface Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "protocol.file.allow=always",
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn review_never_runs_a_filter_the_repository_defines() {
    let project = Project::new();
    project.config("filter.x.clean", &project.program("filter-clean"));
    project.unpacked();

    let review = read_repository_review(&project.repo);

    assert_eq!(project.marks(), "", "Review ran the repository's filter");
    assert!(review.repository);
    assert!(review.error.is_none(), "{:?}", review.error);
    assert_eq!(review.clean, Some(true));
}

#[test]
fn thread_checkpoint_never_runs_a_filter_the_repository_defines() {
    let project = Project::new();
    project.config("filter.x.clean", &project.program("filter-clean"));
    project.unpacked();

    let captured = thread_git::capture_checkpoint(&project.repo, "refs/interface/probe")
        .expect("the checkpoint is taken");

    assert_eq!(
        project.marks(),
        "",
        "the checkpoint ran the repository's filter"
    );
    assert!(
        matches!(captured, CaptureOutcome::Captured { .. }),
        "{captured:?}"
    );
}

#[test]
fn a_required_filter_interface_switches_off_does_not_fail_review_or_checkpoints() {
    let project = Project::new();
    project.config("filter.x.clean", &project.program("filter-clean"));
    project.config("filter.x.smudge", &project.program("filter-smudge"));
    project.config("filter.x.required", "true");
    project.unpacked();

    let review = read_repository_review(&project.repo);
    let captured = thread_git::capture_checkpoint(&project.repo, "refs/interface/required");

    assert_eq!(project.marks(), "");
    assert!(review.error.is_none(), "{:?}", review.error);
    assert_eq!(review.clean, Some(true));
    assert!(
        matches!(captured, Ok(CaptureOutcome::Captured { .. })),
        "{captured:?}"
    );
}

#[test]
fn restoring_a_checkpoint_never_runs_the_repositorys_smudge_filter() {
    let project = Project::new();
    let CaptureOutcome::Captured { .. } =
        thread_git::capture_checkpoint(&project.repo, "refs/interface/before").unwrap()
    else {
        panic!("the first checkpoint is taken");
    };
    project.config("filter.x.clean", &project.program("filter-clean"));
    project.config("filter.x.smudge", &project.program("filter-smudge"));
    fs::write(project.repo.join("a.txt"), "changed by a turn\n").unwrap();

    let restored = thread_git::restore_to(
        &project.repo,
        "refs/interface/before",
        "refs/interface/recovery",
    );

    assert_eq!(project.marks(), "", "restore ran the repository's filters");
    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(
        fs::read_to_string(project.repo.join("a.txt")).unwrap(),
        "hello\n"
    );
}

#[test]
fn a_long_running_filter_process_the_repository_defines_never_starts() {
    let project = Project::new();
    project.config("filter.x.process", &project.program("filter-process"));
    project.config("filter.x.required", "true");
    project.unpacked();

    let review = read_repository_review(&project.repo);
    let captured = thread_git::capture_checkpoint(&project.repo, "refs/interface/process");

    assert_eq!(project.marks(), "");
    assert!(review.error.is_none(), "{:?}", review.error);
    assert!(
        matches!(captured, Ok(CaptureOutcome::Captured { .. })),
        "{captured:?}"
    );
}

/// Review and a checkpoint of `project`, whose files read through the filter
/// `name`, run nothing.
fn assert_filter_never_runs(project: &Project, name: &str) {
    fs::write(
        project.repo.join(".gitattributes"),
        format!("* filter={name}\n"),
    )
    .unwrap();
    project.unpacked();
    let review = read_repository_review(&project.repo);
    let captured = thread_git::capture_checkpoint(&project.repo, "refs/interface/probe");
    assert_eq!(project.marks(), "", "the repository's filter {name} ran");
    assert!(review.error.is_none(), "{:?}", review.error);
    assert!(
        matches!(captured, Ok(CaptureOutcome::Captured { .. })),
        "{captured:?}"
    );
}

/// A settings file outside the repository holding filter `name`.
fn settings_file(project: &Project, file: &str, name: &str) -> PathBuf {
    let path = project.root.path().join(file);
    let program = project.program(&format!("filter-{name}"));
    fs::write(&path, format!("[filter \"{name}\"]\n\tclean = {program}\n")).unwrap();
    path
}

#[test]
fn a_filter_from_a_file_the_repository_settings_include_never_runs() {
    let project = Project::new();
    settings_file(&project, "included.cfg", "included");
    // Relative to `.git/config`.
    project.config("include.path", "../../included.cfg");
    assert_filter_never_runs(&project, "included");
}

#[test]
fn a_filter_from_a_file_the_repository_settings_include_conditionally_never_runs() {
    let project = Project::new();
    let conditional = settings_file(&project, "conditional.cfg", "conditional");
    project.config(
        &format!("includeIf.gitdir:{}/.git.path", project.repo.display()),
        conditional.to_str().unwrap(),
    );
    assert_filter_never_runs(&project, "conditional");
}

#[test]
fn a_filter_from_a_worktrees_own_settings_never_runs() {
    let project = Project::new();
    project.config("extensions.worktreeConfig", "true");
    let program = project.program("filter-worktree");
    project.git(&["config", "--worktree", "filter.worktree.clean", &program]);
    assert_filter_never_runs(&project, "worktree");
}

#[test]
fn a_filter_whose_name_holds_an_equals_sign_never_runs() {
    let project = Project::new();
    // `-c filter.a=b.clean=` would set `filter.a` instead; git splits at the first `=`.
    project.config("filter.a=b.clean", &project.program("filter-equals"));
    fs::write(project.repo.join(".gitattributes"), "* filter=a=b\n").unwrap();
    project.unpacked();

    let review = read_repository_review(&project.repo);
    let captured = thread_git::capture_checkpoint(&project.repo, "refs/interface/equals");

    assert_eq!(project.marks(), "");
    assert!(review.error.is_none(), "{:?}", review.error);
    assert!(captured.is_ok(), "{captured:?}");
}

/// Git LFS set up in the repository's own settings (`git lfs install --local`,
/// or a repository that arrived that way) is not run: a file whose stat data
/// is fresh reads as unchanged, and one git has to re-read shows its real
/// content against the committed pointer.
#[test]
fn git_lfs_set_up_only_by_the_repository_never_runs_and_files_read_as_they_are() {
    let project = Project::new();
    fs::write(
        project.repo.join(".gitattributes"),
        "*.bin filter=lfs diff=lfs merge=lfs -text\n",
    )
    .unwrap();
    let pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\nsize 12\n";
    fs::write(project.repo.join("fresh.bin"), pointer).unwrap();
    fs::write(project.repo.join("model.bin"), pointer).unwrap();
    project.git(&["add", "--all"]);
    project.git(&["commit", "--quiet", "-m", "pointers"]);
    project.config("filter.lfs.clean", &project.program("filter-lfs-clean"));
    project.config("filter.lfs.smudge", &project.program("filter-lfs-smudge"));
    project.config("filter.lfs.process", &project.program("filter-lfs-process"));
    project.config("filter.lfs.required", "true");
    // The real content where the pointer was, as a smudge would have left it.
    fs::write(project.repo.join("model.bin"), b"\0real bytes\n").unwrap();

    let review = read_repository_review(&project.repo);
    let captured = thread_git::capture_checkpoint(&project.repo, "refs/interface/lfs");

    assert_eq!(project.marks(), "");
    assert!(review.error.is_none(), "{:?}", review.error);
    let paths = review
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["model.bin"], "{:?}", review.files);
    assert!(
        matches!(captured, Ok(CaptureOutcome::Captured { .. })),
        "{captured:?}"
    );
}

/// The Ship card asks the remote for its default branch on its own (there
/// is no `refs/remotes/<remote>/HEAD`). Reads `project`'s ship state with its
/// `origin` at `url`, after `setup`, and answers what ran.
fn ship_read(project: &Project, url: &str, setup: impl FnOnce(&Project)) -> String {
    project.git(&["remote", "add", "origin", url]);
    setup(project);
    let state = thread_git::ship::ship_state(&project.repo, true);
    assert!(state.is_ok(), "{state:?}");
    project.marks()
}

#[test]
fn the_ship_cards_remote_read_never_runs_the_repositorys_upload_pack() {
    let project = Project::new();
    let other = project.root.path().join("other");
    fs::create_dir(&other).unwrap();
    git_in(&other, &["init", "--quiet"]);
    git_in(
        &other,
        &["commit", "--quiet", "--allow-empty", "-m", "remote"],
    );
    // A remote on this disk runs `remote.<name>.uploadpack` here, in a shell.
    let ran = ship_read(&project, other.to_str().unwrap(), |project| {
        let program = project.program("upload-pack");
        project.config(
            "remote.origin.uploadpack",
            &format!("{program}; git-upload-pack"),
        );
    });
    assert_eq!(ran, "");
    let state = thread_git::ship::ship_state(&project.repo, true).unwrap();
    assert!(state.default_branch.is_some(), "{state:?}");
}

#[test]
fn the_ship_cards_remote_read_never_runs_the_repositorys_ssh_command() {
    let project = Project::new();
    let ran = ship_read(&project, "ssh://127.0.0.1:1/project.git", |project| {
        project.config("core.sshCommand", &project.program("ssh-command"));
    });
    assert_eq!(ran, "");
}

#[test]
fn the_ship_cards_remote_read_never_runs_the_repositorys_git_proxy() {
    let project = Project::new();
    let ran = ship_read(&project, "git://127.0.0.1:1/project.git", |project| {
        project.config("core.gitProxy", &project.program("git-proxy"));
    });
    assert_eq!(ran, "");
}

#[test]
fn the_ship_cards_remote_read_never_runs_an_ext_transport_the_repository_allows() {
    let project = Project::new();
    let url = format!("ext::{} %S", project.program("ext-transport"));
    let ran = ship_read(&project, &url, |project| {
        project.config("protocol.ext.allow", "always");
    });
    assert_eq!(ran, "");
}

/// An HTTP remote that asks for a password reaches credential helpers (the
/// URL-specific kind too) and then the askpass program.
#[test]
fn the_ship_cards_remote_read_never_runs_the_repositorys_credential_helpers_or_askpass() {
    let project = Project::new();
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in server.incoming().flatten().take(8) {
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let url = format!("http://127.0.0.1:{port}/project.git");
    let ran = ship_read(&project, &url, |project| {
        project.config(
            &format!("credential.http://127.0.0.1:{port}.helper"),
            &format!("!{}", project.program("credential-for-url")),
        );
        project.config(
            "credential.helper",
            &format!("!{}", project.program("credential")),
        );
        project.config("core.askPass", &project.program("askpass"));
    });
    assert_eq!(ran, "");
}

/// A partial clone fetches a missing object through the repository's own
/// remote settings, an upload-pack command among them; Interface's reads
/// never fetch.
#[test]
fn a_partial_clone_never_fetches_through_the_repositorys_upload_pack() {
    let project = Project::new();
    fs::write(project.repo.join("a.txt"), "second\n").unwrap();
    project.git(&["commit", "--quiet", "--all", "-m", "second"]);
    project.config("uploadpack.allowFilter", "true");
    let url = format!("file://{}", project.repo.display());
    git_in(
        project.root.path(),
        &["clone", "--quiet", "--filter=blob:none", &url, "partial"],
    );
    let partial = project.root.path().join("partial");
    git_in(&partial, &["config", "protocol.file.allow", "always"]);
    git_in(
        &partial,
        &[
            "config",
            "remote.origin.uploadpack",
            &format!("{}; git-upload-pack", project.program("lazy-fetch")),
        ],
    );

    // The first commit's `a.txt` was never fetched.
    let _ = thread_git::numstat(&partial, "HEAD~1", Some("HEAD"));

    assert_eq!(project.marks(), "");
}
