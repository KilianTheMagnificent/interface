//! Release preparation helpers. The durable coordinator lives in `promotion_run.rs`.
use crate::{
    gate,
    release::{self, MANIFEST_NAME, parse_latest_mac_manifest},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
pub(crate) const DEFAULT_RELEASE_WORKTREE: &str = ".interface-release";
const GIT_ATTEMPTS: u32 = 3;
const GIT_RETRY_PAUSE: Duration = Duration::from_secs(2);
const PACKAGE_JSON: &str = gate::VERSION_FILES[0];
const PACKAGE_LOCK: &str = gate::VERSION_FILES[1];
fn bump_subject(version: &str) -> String {
    format!("chore(release): bump Interface to {version}")
}

/// The `"version"` of a `package.json` text.
pub fn version_in_package_json(text: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("package.json is not JSON")?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .context("package.json has no \"version\" string")
}

/// The landings `wanted` names, in dev order (oldest first), each once.
pub fn selected_in_dev_order<'a>(
    landings: &'a [crate::landings::Landing],
    wanted: &[String],
) -> Result<Vec<&'a crate::landings::Landing>> {
    ensure!(
        !wanted.is_empty(),
        "promote --change needs at least one landing"
    );
    let mut chosen = std::collections::BTreeSet::new();
    for name in wanted {
        match crate::landings::resolve(landings, name) {
            Ok(landing) => {
                chosen.insert(landing.sha.clone());
            }
            Err(refusal) => bail!("promote refused: {refusal}"),
        }
    }
    Ok(landings
        .iter()
        .filter(|landing| chosen.contains(&landing.sha))
        .collect())
}

pub(crate) fn check_release_worktree(root: &Path, worktree: &Path) -> Result<()> {
    if let Some(refusal) = release_worktree_refusal(root, worktree) {
        bail!(
            "{} cannot be the release worktree: {refusal}",
            worktree.display()
        );
    }
    let listing = gate::git_capture(root, &["worktree", "list", "--porcelain"])?;
    let registration = crate::worktree_prune::registered(&listing, worktree);
    // A path git still lists but that no longer exists cannot be re-added, and
    // git's own error says nothing about the fix. Refusing here keeps the bump
    // and the gate from running first.
    if !worktree.exists() && registration.is_some() {
        bail!(
            "{} is registered as a worktree but is missing on disk; run `git worktree prune` first.",
            worktree.display()
        );
    }
    // Step 4 detaches the release worktree onto the promoted commit. On a
    // worktree with a branch checked out that would move somebody else's
    // checkout off its branch — `~/Developer/.interface-dev` is on `dev` and is
    // the checkout the Dev app runs from.
    if let Some(branch) = registration.and_then(|registration| registration.branch) {
        bail!(
            "{} has the branch {branch} checked out; promote packages a detached commit and will \
             not move a branch. Detach it (`git -C {} checkout --detach`) or pass \
             --release-worktree with another path.",
            worktree.display(),
            worktree.display()
        );
    }
    Ok(())
}

/// Why a path cannot host the packaging run, or `None` when it can.
///
/// The comparison is over canonicalized paths: `package_mac` canonicalizes its
/// root, so a lexical check would pass `/tmp/x` here and refuse it only after
/// the bump, the gate and an `npm ci`.
pub fn release_worktree_refusal(root: &Path, worktree: &Path) -> Option<&'static str> {
    if worktree.is_relative() {
        return Some("it must be an absolute path");
    }
    let worktree = canonical_candidate(worktree);
    let root = canonical_candidate(root);
    if worktree.starts_with(canonical_candidate(Path::new(
        crate::package::PRIVATE_TEMP_PARENT,
    ))) {
        return Some("package-mac refuses any checkout under /private/tmp");
    }
    if worktree.starts_with(&root) || root.starts_with(&worktree) {
        return Some("it must not overlap the checkout being promoted");
    }
    None
}

/// A path with its nearest existing ancestor canonicalized, so a path that does
/// not exist yet still compares the way it will once it does. On macOS this is
/// what turns `/tmp/x` into `/private/tmp/x`.
pub fn canonical_candidate(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut ancestor = path;
    loop {
        if let Ok(resolved) = fs::canonicalize(ancestor) {
            return rest
                .iter()
                .rev()
                .fold(resolved, |path, part| path.join(part));
        }
        let Some(name) = ancestor.file_name() else {
            return path.to_owned();
        };
        rest.push(name.to_owned());
        let Some(parent) = ancestor.parent() else {
            return path.to_owned();
        };
        ancestor = parent;
    }
}

/// The version the local update feed serves, or `None` when there is no feed.
pub(crate) fn feed_version() -> Result<Option<String>> {
    manifest_version(&release::default_feed_target()?)
}

/// The version a directory's `latest-mac.yml` carries, or `None` when the
/// directory holds no manifest at all.
pub(crate) fn manifest_version(directory: &Path) -> Result<Option<String>> {
    let manifest = directory.join(MANIFEST_NAME);
    match fs::read(&manifest) {
        Ok(bytes) => parse_latest_mac_manifest(&bytes)
            .with_context(|| format!("could not read {}", manifest.display()))
            .map(|manifest| Some(manifest.version)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not read {}", manifest.display())),
    }
}

/// Prints what the remaining steps would do, having changed nothing.
pub(crate) fn bump(root: &Path, version: &str, body: &str) -> Result<()> {
    let before = gate::rev_parse(root, "HEAD")?;
    println!("promote: bump to {version}");
    crate::package::run_command(
        "npm",
        [
            "--prefix",
            "electron",
            "version",
            version,
            "--no-git-tag-version",
        ],
        root,
    )?;
    let package_version = crate::package::read_package_version(&root.join(PACKAGE_JSON))?;
    ensure!(
        package_version == version,
        "{PACKAGE_JSON} carries {package_version} after the bump, not {version}."
    );
    let lock_version = read_lock_version(&root.join(PACKAGE_LOCK))?;
    ensure!(
        lock_version == version,
        "{PACKAGE_LOCK} carries {lock_version} after the bump, not {version}."
    );
    let (_, completed_at) = crate::update_log::current_committer_time(root);
    gate::git_capture(root, &["add", "--", PACKAGE_JSON, PACKAGE_LOCK])?;
    let commit = gate::git_command(
        root,
        &[
            "commit",
            "-m",
            &bump_subject(version),
            "-m",
            body,
            "--",
            PACKAGE_JSON,
            PACKAGE_LOCK,
        ],
        gate::GitStderr::Capture,
    )
    .env("GIT_AUTHOR_DATE", &completed_at)
    .env("GIT_COMMITTER_DATE", &completed_at)
    .output()
    .context("failed to commit version bump")?;
    if !commit.status.success() {
        bail!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        );
    }
    if root.join(crate::update_log::UPDATES_DOC).exists() {
        crate::update_log::write_update_log(root)?;
        gate::git_capture(root, &["add", "--", crate::update_log::UPDATES_DOC])?;
        let amend = gate::git_command(
            root,
            &[
                "commit",
                "--amend",
                "--no-edit",
                "--",
                PACKAGE_JSON,
                PACKAGE_LOCK,
                crate::update_log::UPDATES_DOC,
            ],
            gate::GitStderr::Capture,
        )
        .env("GIT_AUTHOR_DATE", &completed_at)
        .env("GIT_COMMITTER_DATE", &completed_at)
        .output()
        .context("failed to amend version bump with update log")?;
        if !amend.status.success() {
            bail!(
                "git commit --amend failed: {}",
                String::from_utf8_lossy(&amend.stderr).trim()
            );
        }
    }
    ensure!(
        gate::version_only_diff(root, &before, &gate::rev_parse(root, "HEAD")?)?,
        "version bump changed other content; candidate is not accepted"
    );
    println!(
        "promote: committed the bump as {}",
        gate::short(&gate::rev_parse(root, "HEAD")?)
    );
    Ok(())
}

/// Both version fields `npm version` must have rewritten.
fn read_lock_version(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let lock: serde_json::Value =
        serde_json::from_slice(&bytes).with_context(|| format!("{} is invalid", path.display()))?;
    let version = lock
        .get("version")
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("{} has no version", path.display()))?;
    if let Some(root_package) = lock
        .pointer("/packages/")
        .and_then(|package| package.get("version"))
        .and_then(serde_json::Value::as_str)
    {
        ensure!(
            root_package == version,
            "{} disagrees with itself about the version.",
            path.display()
        );
    }
    Ok(version.to_owned())
}

pub(crate) fn git_through_icloud(root: &Path, verb: &str, arguments: &[&str]) -> Result<()> {
    let mut attempt = 1_u32;
    loop {
        let output = gate::git_command(root, arguments, gate::GitStderr::Capture)
            .output()
            .context("failed to launch git")?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        attempt += 1;
        if !(mmap_cancelled(&stderr) && attempt <= GIT_ATTEMPTS) {
            eprint!("{stderr}");
            bail!("git {} failed with {}", arguments.join(" "), output.status);
        }
        println!(
            "promote: git {verb} failed with a cancelled mmap (iCloud sync); retrying \
             ({attempt}/{GIT_ATTEMPTS})"
        );
        thread::sleep(GIT_RETRY_PAUSE);
    }
}

/// Whether a git failure is the file provider cancelling a read rather than
/// anything about the repository: `fatal: mmap failed: Operation canceled`.
pub fn mmap_cancelled(stderr: &str) -> bool {
    stderr.contains("mmap failed")
}

/// Creates the release worktree without letting git check its files out.
///
/// git reads the objects out of the promoted checkout, which lives in iCloud
/// Drive, and the file provider cancels those reads under sync load: the first
/// real promotion crawled to 26% of 542 files in twelve minutes and then died
/// with `mmap failed: Operation canceled`, and a worktree outside iCloud
/// failed the same way in seven seconds. `--no-checkout` registers the
/// worktree without reading any of them; the files git tracks are copied from
/// this checkout instead, whose tree *is* `promoted` because step 1 required a
/// clean tree at it; `git reset` then builds the index over what was copied.
/// A copy that does not reproduce the commit is refused by name rather than
/// packaged, and the registration this run made is taken back with it.
pub(crate) fn seed_worktree(root: &Path, worktree: &Path, promoted: &str) -> Result<()> {
    let started = Instant::now();
    git_through_icloud(
        root,
        "worktree add",
        &[
            "worktree",
            "add",
            "--no-checkout",
            "--detach",
            &worktree.to_string_lossy(),
            promoted,
        ],
    )?;
    // The registration exists from here on. A failure that left it in place
    // would meet the next run as a release worktree "with local changes" that
    // promote refuses to overwrite — blaming the operator for this run's own
    // half-finished copy — so the run undoes what it just made.
    let filled = fill_worktree(root, worktree, promoted);
    if filled.is_err() {
        remove_worktree(root, worktree);
    }
    let files = filled?;
    println!(
        "promote: seeded the release worktree with {files} tracked files in {:.1?}",
        started.elapsed()
    );
    Ok(())
}

/// Copies the promoting checkout's tracked files into the registered worktree
/// and proves the result is `promoted`. Returns how many files were copied.
fn fill_worktree(root: &Path, worktree: &Path, promoted: &str) -> Result<usize> {
    let listing = tracked_files(root)?;
    let list = tempfile::Builder::new()
        .prefix("interface-seed-")
        .tempfile()
        .context("could not create the seed file list")?;
    fs::write(list.path(), &listing)
        .with_context(|| format!("could not write {}", list.path().display()))?;
    crate::package::run_command_os("rsync", seed_arguments(list.path(), root, worktree), root)?;
    git_through_icloud(worktree, "reset", &["reset", "--quiet"])?;
    if let Some(difference) = seed_difference(&gate::status_porcelain(worktree)?) {
        bail!(
            "the release worktree {} does not match {} after being seeded from this checkout: \
             {difference}",
            worktree.display(),
            gate::short(promoted)
        );
    }
    Ok(listing.lines().filter(|line| !line.is_empty()).count())
}

/// Every path git tracks in `root`, one per line, as rsync's `--files-from`
/// reads them.
///
/// `core.quotePath=false` keeps non-ASCII names literal. A name git still has
/// to quote — one holding a newline, a quote or a backslash — arrives quoted on
/// a single line, so rsync looks for a file under that literal name, finds
/// none, and fails the seed. Such a name is never copied wrong, and no parsing
/// here would make it copyable; this repository has none.
fn tracked_files(root: &Path) -> Result<String> {
    gate::git_capture(root, &["-c", "core.quotePath=false", "ls-files"])
}

/// Undoes a registration whose seed failed. The removal failing only prints:
/// the failure worth reporting is the seed's, and this one has a hand fix.
fn remove_worktree(root: &Path, worktree: &Path) {
    let path = worktree.to_string_lossy().into_owned();
    let removed = gate::git(root, &["worktree", "remove", "--force", &path])
        .is_ok_and(|output| output.status.success());
    if removed {
        println!("promote: removed the release worktree this run had just created");
    } else {
        println!(
            "promote: could not remove the half-seeded release worktree; remove it by hand with \
             `git worktree remove --force {path}`"
        );
    }
}

/// The `rsync` arguments that copy the files `list` names from one checkout
/// into another.
///
/// Naming every tracked file is the only reliable way to copy a checkout: an
/// exclude list has to guess, and these checkouts hold tens of gigabytes it
/// would have to guess right about — nested worktrees, a `target.nosync`
/// behind a symlink, agent caches, a root `node_modules` — none of which
/// `git status` would report afterwards, because they are all ignored.
///
/// Both paths carry a trailing slash: that is what makes rsync copy a
/// directory's contents rather than the directory itself.
pub fn seed_arguments(list: &Path, from: &Path, to: &Path) -> Vec<String> {
    vec![
        "-a".to_owned(),
        format!("--files-from={}", list.display()),
        format!("{}/", from.display()),
        format!("{}/", to.display()),
    ]
}

/// What a seeded worktree still differs from its own commit by, or `None` when
/// the copy reproduced that commit exactly. `status` is porcelain output.
pub fn seed_difference(status: &str) -> Option<String> {
    let differences: Vec<&str> = status
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    (!differences.is_empty()).then(|| differences.join("; "))
}

pub(crate) fn default_release_worktree() -> Result<PathBuf> {
    Ok(crate::pipeline_state::home()?.join(DEFAULT_RELEASE_WORKTREE))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_candidate_resolves_the_nearest_existing_ancestor() {
        // /tmp is a symlink to /private/tmp on macOS: the lexical check this
        // replaced passed such a path and package-mac refused it much later.
        let resolved = canonical_candidate(Path::new("/tmp/interface-release/electron"));
        assert_eq!(
            resolved,
            Path::new("/private/tmp/interface-release/electron")
        );
        assert_eq!(
            canonical_candidate(Path::new("/tmp")),
            Path::new("/private/tmp")
        );
        assert_eq!(canonical_candidate(Path::new("/")), Path::new("/"));
        let missing = Path::new("/nonexistent-root-for-a-test/a/b");
        assert_eq!(canonical_candidate(missing), missing);
    }

    #[test]
    fn release_worktree_refusal_rejects_the_paths_that_cannot_package() {
        let root = Path::new("/Users/a/Desktop/Projects/Interface-Rust");
        let good = Path::new("/Users/a/.interface-release");
        assert_eq!(release_worktree_refusal(root, good), None);
        assert_eq!(
            release_worktree_refusal(root, Path::new("/private/tmp/interface-release")),
            Some("package-mac refuses any checkout under /private/tmp")
        );
        assert_eq!(
            release_worktree_refusal(root, root),
            Some("it must not overlap the checkout being promoted")
        );
        assert_eq!(
            release_worktree_refusal(root, &root.join("release-worktree")),
            Some("it must not overlap the checkout being promoted")
        );
        assert_eq!(
            release_worktree_refusal(root, Path::new("/Users/a/Desktop/Projects")),
            Some("it must not overlap the checkout being promoted"),
            "a parent of the workspace root overlaps it too"
        );
        assert_eq!(
            release_worktree_refusal(root, Path::new(".interface-release")),
            Some("it must be an absolute path")
        );
    }

    #[test]
    fn only_a_cancelled_mmap_makes_a_git_failure_worth_retrying() {
        // Exactly what runs 1 and 2 died with, the second after seven seconds.
        assert!(mmap_cancelled("fatal: mmap failed: Operation canceled\n"));
        assert!(mmap_cancelled(
            "Preparing worktree (detached HEAD c196cec)\n\
             fatal: mmap failed: Operation canceled\n"
        ));
        assert!(!mmap_cancelled(
            "fatal: '/Users/a/.interface-release' already exists\n"
        ));
        assert!(!mmap_cancelled(
            "error: Your local changes would be overwritten by checkout.\n"
        ));
        assert!(!mmap_cancelled(""));
    }

    #[test]
    fn seed_arguments_copy_only_the_files_git_lists() {
        // No --exclude anywhere: an exclude list has to guess, and these
        // checkouts carry tens of gigabytes of ignored paths it would have to
        // guess right about — and git status reports none of them afterwards.
        assert_eq!(
            seed_arguments(
                Path::new("/var/folders/x/interface-seed-abc"),
                Path::new("/Users/a/Desktop/Projects/Interface-Rust"),
                Path::new("/Users/a/.interface-release"),
            ),
            [
                "-a".to_owned(),
                "--files-from=/var/folders/x/interface-seed-abc".to_owned(),
                // rsync copies a directory's contents only when both end in /.
                "/Users/a/Desktop/Projects/Interface-Rust/".to_owned(),
                "/Users/a/.interface-release/".to_owned(),
            ]
        );
    }

    #[test]
    fn seed_difference_names_what_a_copy_failed_to_reproduce() {
        assert_eq!(seed_difference(""), None);
        assert_eq!(seed_difference("\n   \n"), None);
        assert_eq!(
            seed_difference(" M electron/package.json\n?? target 2/\n"),
            Some("M electron/package.json; ?? target 2/".to_owned())
        );
    }

    /// The seeding step itself, over a real repository: registering a worktree
    /// with `--no-checkout` and copying git's own file list into it has to
    /// leave a tree git calls clean, carry a tracked symlink across as a
    /// symlink, and bring neither the source's `.git` nor anything git does not
    /// track — an ignored path least of all, since `git status` would never
    /// report one afterwards.
    #[test]
    fn a_seeded_worktree_holds_the_tracked_files_and_nothing_else() {
        let temporary = tempfile::tempdir().expect("a temporary directory");
        let repository = temporary.path().join("repository");
        let worktree = temporary.path().join("release");
        fs::create_dir_all(repository.join("electron")).expect("the repository");
        fs::write(repository.join(".gitignore"), "/ignored/\n").expect("a tracked file");
        fs::write(repository.join("README.md"), "seed\n").expect("a tracked file");
        fs::write(
            repository.join("electron/package.json"),
            "{ \"version\": \"0.0.1\" }\n",
        )
        .expect("a nested tracked file");
        std::os::unix::fs::symlink("README.md", repository.join("readme-link"))
            .expect("a tracked symlink");
        let git = |arguments: &[&str]| {
            gate::git_capture(&repository, arguments).unwrap_or_else(|error| panic!("{error}"));
        };
        git(&["init", "--quiet", "--initial-branch=main"]);
        git(&["config", "user.email", "xtask@example.invalid"]);
        git(&["config", "user.name", "xtask"]);
        git(&["config", "commit.gpgsign", "false"]);
        git(&["add", "--all", "--", "."]);
        git(&["commit", "--quiet", "-m", "seed"]);
        let head = gate::rev_parse(&repository, "HEAD").expect("the commit");
        // Untracked build output: seeded, it would leave the worktree dirty.
        fs::create_dir_all(repository.join("target/debug")).expect("build output");
        fs::write(repository.join("target/debug/xtask"), "binary\n").expect("build output");
        // An ignored cache: seeded, nothing would ever report it, which is
        // exactly how 200 GB of somebody else's worktrees got copied.
        fs::create_dir_all(repository.join("ignored")).expect("an ignored directory");
        fs::write(repository.join("ignored/cache.bin"), "huge\n").expect("an ignored file");

        git(&[
            "worktree",
            "add",
            "--no-checkout",
            "--detach",
            &worktree.to_string_lossy(),
            &head,
        ]);
        let listing = tracked_files(&repository).expect("git lists what it tracks");
        let list = temporary.path().join("files-from");
        fs::write(&list, &listing).expect("the file list");
        crate::package::run_command_os(
            "rsync",
            seed_arguments(&list, &repository, &worktree),
            &repository,
        )
        .expect("rsync copies the tracked files");
        gate::git_capture(&worktree, &["reset", "--quiet"]).expect("the index");

        assert_eq!(
            seed_difference(&gate::status_porcelain(&worktree).expect("a status")),
            None,
            "a seeded worktree has to match the commit it was registered at"
        );
        assert_eq!(
            fs::read_to_string(worktree.join("electron/package.json")).expect("the nested file"),
            "{ \"version\": \"0.0.1\" }\n"
        );
        assert_eq!(
            fs::read_link(worktree.join("readme-link")).expect("the tracked symlink"),
            Path::new("README.md"),
            "a tracked symlink is copied as a symlink, not as what it points at"
        );
        assert!(
            !worktree.join("target").exists(),
            "untracked build output is not part of the commit"
        );
        assert!(
            !worktree.join("ignored").exists(),
            "an ignored path is not part of the commit either, and no status \
             would have reported it if it had been copied"
        );
        assert!(
            worktree.join(".git").is_file(),
            "the worktree keeps git's own gitdir pointer, not a copy of .git/"
        );
    }

    #[test]
    fn bump_subject_is_the_subject_a_resumed_run_looks_for() {
        assert_eq!(
            bump_subject("0.3.50"),
            "chore(release): bump Interface to 0.3.50"
        );
    }

    fn landing(sha: char, name: &str) -> crate::landings::Landing {
        crate::landings::Landing {
            sha: sha.to_string().repeat(40),
            date: "2026-09-06".to_owned(),
            name: name.to_owned(),
            branch: "b".to_owned(),
            committed: 0,
            kind: None,
            check: None,
            pins: vec![],
            supersedes: vec![],
        }
    }

    #[test]
    fn version_in_package_json_reads_the_version_field() {
        assert_eq!(
            version_in_package_json("{\"name\":\"interface\",\"version\":\"0.3.65\"}").unwrap(),
            "0.3.65"
        );
        assert!(version_in_package_json("{\"name\":\"interface\"}").is_err());
        assert!(version_in_package_json("not json").is_err());
    }

    #[test]
    fn selected_landings_keep_dev_order_and_drop_duplicates() {
        let landings = vec![
            landing('a', "First"),
            landing('b', "Second"),
            landing('c', "Third"),
        ];
        let picked = selected_in_dev_order(
            &landings,
            &["third".to_owned(), "aaaa".to_owned(), "First".to_owned()],
        )
        .unwrap();
        let names: Vec<&str> = picked.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["First", "Third"]);
        let refusal = selected_in_dev_order(&landings, &["nope".to_owned()]).unwrap_err();
        assert!(
            refusal.to_string().contains("cargo xtask landings"),
            "{refusal}"
        );
        assert!(selected_in_dev_order(&landings, &[]).is_err());
    }
}
