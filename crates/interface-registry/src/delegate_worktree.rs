//! Read-only proof that a requested directory belongs to a registered native
//! worktree of an already-approved standard Git repository. This helper never
//! creates a checkout, changes Git state, or grants a persistent workspace root.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use crate::worktree::strings;
use crate::{GitRunner, SystemGit};

const MAX_PATH_BYTES: usize = 4096;
const MAX_LIST_BYTES: usize = 256 * 1024;
const MAX_WORKTREES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Directory {
    path: PathBuf,
    device: u64,
    inode: u64,
}

/// Proves a native worktree relation for one candidate directory. The callback
/// must check existing host authority for the repository containing the common
/// `.git` directory, not infer authority from the candidate itself.
///
/// Git commands use the existing bounded, argv-only system runner. Call on a
/// blocking worker. Linked-worktree and subdirectory paths are supported; bare
/// repositories, custom common-directory layouts, and symlink aliases refuse.
///
/// # Errors
/// Refuses unapproved repositories, unsafe or drifting directory identities,
/// missing/ambiguous registration, prunable worktrees, or any failed Git probe.
pub fn related_worktree(
    candidate: &Path,
    approves_repository: impl Fn(&Path) -> bool,
) -> Result<PathBuf, String> {
    related_worktree_with(candidate, &approves_repository, &SystemGit)
}

fn related_worktree_with(
    candidate: &Path,
    approves_repository: &dyn Fn(&Path) -> bool,
    git: &dyn GitRunner,
) -> Result<PathBuf, String> {
    let candidate = directory(candidate)?;
    let top = directory(&git_path(
        git,
        &candidate.path,
        &["rev-parse", "--show-toplevel"],
    )?)?;
    if !candidate.path.starts_with(&top.path) {
        return Err("requested directory is outside its reported Git worktree".into());
    }
    let common = directory(&common_directory(git, &candidate.path)?)?;
    if common.path.file_name() != Some(std::ffi::OsStr::new(".git")) {
        return Err("only standard repositories with a common .git directory are supported".into());
    }
    let repository = directory(
        common
            .path
            .parent()
            .ok_or("Git repository is unavailable")?,
    )?;
    if !approves_repository(&repository.path) {
        return Err("this worktree's source repository is not an approved workspace".into());
    }
    if common_directory(git, &repository.path)? != common.path {
        return Err("approved repository does not own this Git common directory".into());
    }
    let common_text = common
        .path
        .to_str()
        .ok_or("Git common directory must be UTF-8")?;
    let listed = git.run(
        &strings(&[
            "--git-dir",
            common_text,
            "worktree",
            "list",
            "--porcelain",
            "-z",
        ]),
        &repository.path,
    );
    if listed.code != 0 || listed.stdout.len() > MAX_LIST_BYTES {
        return Err("Git worktree registration could not be read within its bounds".into());
    }
    let registered = registrations(&listed.stdout)?;
    if !registered.contains(&top.path) || !registered.contains(&repository.path) {
        return Err(
            "Git does not register this exact worktree under its approved repository".into(),
        );
    }
    // A .git pointer aimed at another checkout is insufficient: the exact top
    // must be registered and must independently resolve to this same common dir.
    if common_directory(git, &top.path)? != common.path
        || git_path(git, &candidate.path, &["rev-parse", "--show-toplevel"])? != top.path
    {
        return Err("the requested worktree's Git identity changed".into());
    }
    for pinned in [&candidate, &top, &common, &repository] {
        if directory(&pinned.path)? != *pinned {
            return Err("a worktree directory was replaced during authorization".into());
        }
    }
    if !approves_repository(&repository.path) {
        return Err("source repository approval changed during authorization".into());
    }
    Ok(candidate.path)
}

fn common_directory(git: &dyn GitRunner, cwd: &Path) -> Result<PathBuf, String> {
    git_path(
        git,
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
}

fn git_path(git: &dyn GitRunner, cwd: &Path, arguments: &[&str]) -> Result<PathBuf, String> {
    let result = git.run(&strings(arguments), cwd);
    if result.code != 0 || result.stdout.len() > MAX_PATH_BYTES + 1 {
        return Err("Git directory identity could not be read within its bounds".into());
    }
    let text = result.stdout.strip_suffix('\n').unwrap_or(&result.stdout);
    literal_absolute(text).map(PathBuf::from)
}

fn literal_absolute(text: &str) -> Result<&Path, String> {
    let path = Path::new(text);
    if text.is_empty()
        || text.len() > MAX_PATH_BYTES
        || text.chars().any(char::is_control)
        || !path.is_absolute()
        || text
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || !path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err("worktree directory must be a normalized absolute literal path".into());
    }
    Ok(path)
}

fn directory(path: &Path) -> Result<Directory, String> {
    literal_absolute(path.to_str().ok_or("worktree directory must be UTF-8")?)?;
    let before = fs::symlink_metadata(path).map_err(|_| "worktree directory is unavailable")?;
    if !before.is_dir() || before.file_type().is_symlink() {
        return Err("worktree directories must be real directories without symlinks".into());
    }
    let canonical = fs::canonicalize(path).map_err(|_| "worktree directory is unavailable")?;
    let after = fs::symlink_metadata(path).map_err(|_| "worktree directory changed")?;
    if canonical != path
        || !after.is_dir()
        || before.dev() != after.dev()
        || before.ino() != after.ino()
    {
        return Err("worktree directory changed or crossed a symbolic link".into());
    }
    Ok(Directory {
        path: path.to_owned(),
        device: after.dev(),
        inode: after.ino(),
    })
}

/// `-z` keeps path bytes literal and separates each field with NUL and each
/// worktree with an empty field. No C-style quoted pathname parsing is needed.
fn registrations(text: &str) -> Result<BTreeSet<PathBuf>, String> {
    if text.is_empty() || text.len() > MAX_LIST_BYTES || !text.ends_with("\0\0") {
        return Err("Git returned incomplete worktree registration data".into());
    }
    let mut registered = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut path: Option<PathBuf> = None;
    let mut prunable = false;
    let mut count = 0;
    for field in text.split_terminator('\0') {
        if field.is_empty() {
            let found = path
                .take()
                .ok_or("Git returned an empty worktree registration")?;
            count += 1;
            if count > MAX_WORKTREES || !seen.insert(found.clone()) {
                return Err("Git returned too many or duplicate worktree registrations".into());
            }
            if !prunable {
                registered.insert(found);
            }
            prunable = false;
        } else if let Some(value) = field.strip_prefix("worktree ") {
            if path.is_some() {
                return Err("Git returned an ambiguous worktree registration".into());
            }
            path = Some(literal_absolute(value)?.to_path_buf());
        } else if path.is_none() {
            return Err("Git returned a worktree field without its path".into());
        } else if field == "prunable" || field.starts_with("prunable ") {
            prunable = true;
        } else if !(field.starts_with("HEAD ")
            || field.starts_with("branch ")
            || field == "detached"
            || field == "locked"
            || field.starts_with("locked ")
            || field == "bare")
        {
            return Err("Git returned an unsupported worktree registration field".into());
        }
    }
    if path.is_some() {
        return Err("Git returned an unfinished worktree registration".into());
    }
    Ok(registered)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::os::unix::fs::symlink;
    use std::sync::Mutex;

    use crate::GitResult;

    use super::*;

    struct Step {
        cwd: PathBuf,
        arguments: Vec<String>,
        output: GitResult,
    }

    struct FakeGit(Mutex<VecDeque<Step>>);

    impl GitRunner for FakeGit {
        fn run(&self, arguments: &[String], cwd: &Path) -> GitResult {
            let step = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected Git command");
            assert_eq!(step.cwd, cwd);
            assert_eq!(step.arguments, arguments);
            step.output
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        repository: PathBuf,
        common: PathBuf,
        top: PathBuf,
        candidate: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let repository = root.join("approved-repository");
            let common = repository.join(".git");
            let top = root.join("native-worktree");
            let candidate = top.join("src");
            fs::create_dir_all(&common).unwrap();
            fs::create_dir_all(&candidate).unwrap();
            Self {
                _temp: temp,
                repository,
                common,
                top,
                candidate,
            }
        }

        fn listing(&self, extra: &str) -> String {
            format!(
                "worktree {}\0HEAD {}\0branch refs/heads/main\0\0worktree {}\0HEAD {}\0detached\0{extra}\0",
                self.repository.display(),
                "0".repeat(40),
                self.top.display(),
                "1".repeat(40)
            )
        }

        fn plan(&self, listing: String) -> VecDeque<Step> {
            let top = format!("{}\n", self.top.display());
            let common = format!("{}\n", self.common.display());
            [
                (
                    &self.candidate,
                    strings(&["rev-parse", "--show-toplevel"]),
                    top.clone(),
                ),
                (
                    &self.candidate,
                    strings(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
                    common.clone(),
                ),
                (
                    &self.repository,
                    strings(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
                    common.clone(),
                ),
                (
                    &self.repository,
                    strings(&[
                        "--git-dir",
                        self.common.to_str().unwrap(),
                        "worktree",
                        "list",
                        "--porcelain",
                        "-z",
                    ]),
                    listing,
                ),
                (
                    &self.top,
                    strings(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
                    common,
                ),
                (
                    &self.candidate,
                    strings(&["rev-parse", "--show-toplevel"]),
                    top,
                ),
            ]
            .into_iter()
            .map(|(cwd, arguments, stdout)| Step {
                cwd: cwd.clone(),
                arguments,
                output: GitResult {
                    code: 0,
                    stdout,
                    stderr: String::new(),
                },
            })
            .collect()
        }

        fn check(&self, steps: VecDeque<Step>) -> Result<PathBuf, String> {
            related_worktree_with(
                &self.candidate,
                &|root| root == self.repository,
                &FakeGit(Mutex::new(steps)),
            )
        }
    }

    #[test]
    fn accepts_exact_registered_worktree_subdirectory_and_locked_checkout() {
        let fixture = Fixture::new();
        for extra in ["", "locked retained for work\0"] {
            assert_eq!(
                fixture.check(fixture.plan(fixture.listing(extra))).unwrap(),
                fixture.candidate
            );
        }
    }

    #[test]
    fn refuses_unapproved_repository_before_probing_its_registration() {
        let fixture = Fixture::new();
        let mut steps = fixture.plan(fixture.listing(""));
        steps.truncate(2);
        let git = FakeGit(Mutex::new(steps));
        assert!(
            related_worktree_with(&fixture.candidate, &|_| false, &git)
                .unwrap_err()
                .contains("not an approved")
        );
        assert!(git.0.lock().unwrap().is_empty());
    }

    #[test]
    fn refuses_forged_git_mapping_prunable_unregistered_and_duplicate_paths() {
        let fixture = Fixture::new();
        for listing in [
            fixture.listing("prunable missing administrative entry\0"),
            fixture
                .listing("")
                .replace(fixture.top.to_str().unwrap(), "/unregistered"),
            fixture.listing("") + &format!("worktree {}\0detached\0\0", fixture.top.display()),
        ] {
            assert!(fixture.check(fixture.plan(listing)).is_err());
        }
    }

    #[test]
    fn refuses_common_directory_drift_and_candidate_outside_reported_top() {
        let fixture = Fixture::new();
        let mut steps = fixture.plan(fixture.listing(""));
        steps[4].output.stdout = "/foreign/.git\n".into();
        assert!(
            fixture
                .check(steps)
                .unwrap_err()
                .contains("Git identity changed")
        );
        let mut steps = fixture.plan(fixture.listing(""));
        steps[0].output.stdout = format!("{}\n", fixture.repository.display());
        assert!(fixture.check(steps).unwrap_err().contains("outside"));
    }

    #[test]
    fn refuses_custom_common_layout_failed_git_and_oversized_registration() {
        let fixture = Fixture::new();
        let custom = fixture.repository.join("custom.git");
        fs::create_dir(&custom).unwrap();
        let mut steps = fixture.plan(fixture.listing(""));
        steps[1].output.stdout = format!("{}\n", custom.display());
        assert!(fixture.check(steps).is_err());
        let mut steps = fixture.plan(fixture.listing(""));
        steps[0].output.code = -1;
        assert!(fixture.check(steps).is_err());
        assert!(
            fixture
                .check(fixture.plan("x".repeat(MAX_LIST_BYTES + 1)))
                .is_err()
        );
    }

    #[test]
    fn refuses_symlink_aliases_and_directory_replacement_during_approval() {
        let fixture = Fixture::new();
        let alias = fixture.top.parent().unwrap().join("alias");
        symlink(&fixture.candidate, &alias).unwrap();
        assert!(related_worktree_with(&alias, &|_| true, &FakeGit(Mutex::default())).is_err());
        let git = FakeGit(Mutex::new(fixture.plan(fixture.listing(""))));
        let result = related_worktree_with(
            &fixture.candidate,
            &|_| {
                let backup = fixture.top.join("prior-src");
                fs::rename(&fixture.candidate, backup).unwrap();
                fs::create_dir(&fixture.candidate).unwrap();
                true
            },
            &git,
        );
        assert!(result.unwrap_err().contains("replaced"));
    }

    #[test]
    fn nul_parser_refuses_truncation_quoted_paths_and_relative_paths() {
        for listing in [
            "worktree /repo\0HEAD 123\0",
            "worktree /repo\0\0\0",
            "worktree relative\0\0",
            "worktree \"/quoted/path\"\0\0",
            "worktree /repo\0worktree /other\0\0",
            "worktree /repo\0unexpected\0\0",
        ] {
            assert!(registrations(listing).is_err());
        }
        let parsed = registrations("worktree /path with spaces\0HEAD 123\0\0").unwrap();
        assert!(parsed.contains(Path::new("/path with spaces")));
    }

    #[test]
    fn real_git_accepts_registered_checkout_and_refuses_copied_git_pointer() {
        // This test creates and commits only inside its disposable repository.
        // Global Git configuration and hooks are disabled for fixture creation.
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let repository = root.join("repository");
        let checkout = root.join("checkout with spaces");
        let git = |cwd: &Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(cwd)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_COMMON_DIR")
                .output()
                .unwrap();
            assert!(output.status.success(), "fixture Git command failed");
            String::from_utf8(output.stdout).unwrap()
        };
        git(&root, &["init", "-q", repository.to_str().unwrap()]);
        git(
            &repository,
            &[
                "-c",
                "user.name=Interface fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        );
        git(
            &repository,
            &[
                "worktree",
                "add",
                "--detach",
                "--quiet",
                checkout.to_str().unwrap(),
                "HEAD",
            ],
        );
        let subdir = checkout.join("src");
        fs::create_dir(&subdir).unwrap();
        assert_eq!(
            related_worktree(&subdir, |path| path == repository).unwrap(),
            subdir
        );
        assert!(related_worktree(&subdir, |_| false).is_err());

        let forged = root.join("forged");
        fs::create_dir(&forged).unwrap();
        let admin = git(&checkout, &["rev-parse", "--absolute-git-dir"]);
        fs::write(forged.join(".git"), format!("gitdir: {}\n", admin.trim())).unwrap();
        assert!(related_worktree(&forged, |path| path == repository).is_err());
    }
}
