//! `cargo xtask worktree-prune`: find, and optionally remove, the linked
//! worktrees whose work already landed on `dev`.
//!
//! Parallel agents leave one worktree each, and the registrations outlive the
//! branches: a checkout nobody is in, whose branch is already an ancestor of
//! `origin/dev`, holds nothing but a `node_modules` and a `target` directory
//! that cost gigabytes. Deciding that from a shell means a `status`, a
//! `merge-base`, and an `lsof` per worktree, so it never happens.
//!
//! Removal is opt-in. A dry run is the default, it fetches nothing, and it
//! prints why every worktree it did not pick was kept — a "no candidates"
//! result has to be readable as a list of reasons, not as silence.

use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

use crate::{
    dev_restart::DEV_CHECKOUT,
    gate,
    pipeline_state::home,
    promote::{DEFAULT_RELEASE_WORKTREE, canonical_candidate},
    prove::PROVE_WORKTREE,
};

/// Where the Dev-app checkout lived before the project left iCloud Drive; an
/// old registration may still name it. The current one is `DEV_CHECKOUT`, the
/// path `dev-restart` refreshes: it is always in use, whatever `lsof` says, and
/// removing it would take the Dock app down with it.
const LEGACY_DEV_CHECKOUT: &str = "Desktop/Projects/.interface-dev";
/// Where `promote` used to keep the managed release worktree before it moved
/// out of iCloud Drive; an old one may still be registered.
const LEGACY_RELEASE_WORKTREE: &str = "Desktop/Projects/.interface-release";
/// The owner's promote driver: the detached checkout of `origin/dev` that
/// `cargo xtask promote` is run from (`docs/AGENT_RELEASE_WORKFLOW.md`). It is
/// merged, clean and idle between releases, like the Dev checkout.
const PROMOTE_DRIVER: &str = "Developer/.interface-promote";
/// Branches no cleanup removes, or removes a checkout of: the Dev app's
/// checkout has `dev` checked out, and both are the release's refs.
const LONG_LIVED_BRANCHES: [&str; 2] = ["dev", "main"];
/// How long one worktree's `git status` may take. On an iCloud-resident
/// worktree the file provider can stall a single read for the better part of
/// an hour; such a worktree is kept as unscannable, never pruned.
const STATUS_TIMEOUT: Duration = Duration::from_secs(60);
/// What a worktree may leave in `git status` and still count as clean: build
/// output and dependency trees that are ignored in some checkouts and not in
/// others, plus the untracked `.grok/` an agent session drops.
const IGNORED_DIRT: [&str; 5] = [
    "node_modules",
    "electron/node_modules",
    "target",
    "target.nosync",
    ".grok",
];

/// One registration in `git worktree list --porcelain`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Registration {
    /// The checkout directory, as git records it.
    pub path: PathBuf,
    /// The commit the worktree is on.
    pub head: Option<String>,
    /// The branch it has checked out, or `None` when it is detached.
    pub branch: Option<String>,
}

/// Everything the removal decision is allowed to depend on.
// Independent observations of one directory, not a state machine: naming each
// one is what makes [`prune_decision`] readable and its test cases obvious.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeFacts {
    /// The main checkout, which is not a linked worktree at all.
    pub is_main: bool,
    /// A managed checkout: the pinned `~/Developer/.interface-dev` Dev-app
    /// checkout, its old iCloud path, `promote`'s release worktree, or
    /// `prove`'s kept base checkout.
    pub is_dev_checkout: bool,
    /// The promote driver checkout at `~/Developer/.interface-promote`.
    pub is_promote_driver: bool,
    /// It has `dev` or `main` checked out.
    pub long_lived_branch: bool,
    /// Its branch, or its detached commit, is an ancestor of `origin/dev`.
    pub merged: bool,
    /// Uncommitted paths that are not build output.
    pub dirty_paths: Vec<String>,
    /// Some process is working inside it.
    pub in_use: bool,
    /// `git status` did not finish within [`STATUS_TIMEOUT`].
    pub unscannable: bool,
}

/// Why a worktree is kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Keep {
    /// The main checkout.
    Main,
    /// The Dev-app checkout or the managed release worktree.
    DevCheckout,
    /// The checkout `cargo xtask promote` is run from.
    PromoteDriver,
    /// A checkout of `dev` or `main`.
    LongLivedBranch,
    /// Work that is not on `origin/dev` yet.
    Unmerged,
    /// Uncommitted changes.
    Dirty,
    /// A process is working in it.
    InUse,
    /// `git status` timed out, so its state is unknown.
    Unscannable,
}

impl Keep {
    /// The one-line reason a dry run prints.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Main => "the main checkout",
            Self::DevCheckout => "a managed checkout (Dev app or release worktree)",
            Self::PromoteDriver => "the promote driver checkout (promote runs from it)",
            Self::LongLivedBranch => "has dev or main checked out",
            Self::Unmerged => "not merged into origin/dev",
            Self::Dirty => "uncommitted changes",
            Self::InUse => "a process is working in it",
            Self::Unscannable => "git status timed out; state unknown",
        }
    }
}

/// Whether a worktree may be removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// Nothing here is unique or in use.
    Prune,
    /// Kept, for the stated reason.
    Keep(Keep),
}

/// The whole removal decision, kept free of process calls.
///
/// The order is what a reader wants first — identity, then work that has not
/// landed, then whoever is standing in the directory — and it is also cheapest
/// first. `dirty_paths` is last because it is the only fact that costs a walk
/// of the whole working tree: on a synced volume that is minutes per worktree,
/// so [`worktree_prune`] gathers it only when every cheaper fact already says
/// [`Decision::Prune`].
pub fn prune_decision(facts: &WorktreeFacts) -> Decision {
    if facts.is_main {
        return Decision::Keep(Keep::Main);
    }
    if facts.is_dev_checkout {
        return Decision::Keep(Keep::DevCheckout);
    }
    if facts.is_promote_driver {
        return Decision::Keep(Keep::PromoteDriver);
    }
    if facts.long_lived_branch {
        return Decision::Keep(Keep::LongLivedBranch);
    }
    if !facts.merged {
        return Decision::Keep(Keep::Unmerged);
    }
    if facts.in_use {
        return Decision::Keep(Keep::InUse);
    }
    if facts.unscannable {
        return Decision::Keep(Keep::Unscannable);
    }
    if !facts.dirty_paths.is_empty() {
        return Decision::Keep(Keep::Dirty);
    }
    Decision::Prune
}

/// `git status --porcelain` with a deadline. `None` means it did not finish.
///
/// Output is read only after exit, so a listing past the pipe buffer would
/// stall the child until the deadline; that also lands on "unscannable",
/// which keeps the worktree, so the failure is on the safe side.
fn bounded_status(path: &Path) -> Result<Option<String>> {
    let mut child = gate::git_command(path, &["status", "--porcelain"], gate::GitStderr::Null)
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to launch git status")?;
    let deadline = Instant::now() + STATUS_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().context("failed to wait for git status")? {
            anyhow::ensure!(status.success(), "git status failed in {}", path.display());
            let mut listing = String::new();
            if let Some(mut stdout) = child.stdout.take() {
                stdout
                    .read_to_string(&mut listing)
                    .context("failed to read git status")?;
            }
            return Ok(Some(listing));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// The `git worktree list --porcelain` blocks, in the order git prints them.
pub fn parse_worktrees(listing: &str) -> Vec<Registration> {
    let mut worktrees = Vec::new();
    for line in listing.lines() {
        let line = line.trim_end();
        if let Some(path) = line.strip_prefix("worktree ") {
            worktrees.push(Registration {
                path: PathBuf::from(path.trim()),
                head: None,
                branch: None,
            });
        } else if let Some(current) = worktrees.last_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                current.head = Some(head.trim().to_owned());
            } else if let Some(branch) = line.strip_prefix("branch ") {
                let branch = branch.trim();
                current.branch = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_owned(),
                );
            }
        }
    }
    worktrees
}

/// The registration a listing holds for `path`, or `None` when git does not
/// list it. Paths are compared the way [`canonical_candidate`] resolves them,
/// so a `/tmp` argument matches git's own `/private/tmp` listing.
pub fn registered(listing: &str, path: &Path) -> Option<Registration> {
    let wanted = canonical_candidate(path);
    parse_worktrees(listing)
        .into_iter()
        .find(|worktree| canonical_candidate(&worktree.path) == wanted)
}

/// The uncommitted paths that are not build output. `git status --porcelain`
/// collapses an untracked directory to one entry, so an exact match on the
/// ignorable names is enough and never hides a real change beneath them.
pub fn significant_dirty(status: &str) -> Vec<String> {
    status
        .lines()
        .flat_map(gate::porcelain_paths)
        .map(|path| path.trim_end_matches('/').to_owned())
        .filter(|path| !IGNORED_DIRT.contains(&path.as_str()))
        .collect()
}

/// The working directories in one `lsof -F n -d cwd` pass.
pub fn parse_cwds(lsof_stdout: &str) -> Vec<PathBuf> {
    lsof_stdout
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Whether any process is standing inside `path`. `Path::starts_with` compares
/// whole components, so a sibling named `…-2` is not a match.
pub fn is_in_use(path: &Path, cwds: &[PathBuf]) -> bool {
    cwds.iter().any(|cwd| cwd.starts_with(path))
}

/// The checkouts no cleanup removes, resolved the way registrations are:
/// the Dev checkout, the release worktree, and `prove`'s kept base checkout.
pub fn managed_checkouts(home: &Path) -> Vec<PathBuf> {
    [
        DEV_CHECKOUT,
        LEGACY_DEV_CHECKOUT,
        DEFAULT_RELEASE_WORKTREE,
        LEGACY_RELEASE_WORKTREE,
        PROVE_WORKTREE,
    ]
    .iter()
    .map(|relative| canonical_candidate(&home.join(relative)))
    .collect()
}

/// The promote driver checkout, resolved the way registrations are.
pub fn promote_driver(home: &Path) -> PathBuf {
    canonical_candidate(&home.join(PROMOTE_DRIVER))
}

/// Whether a branch is one no cleanup deletes or removes a checkout of.
fn long_lived(branch: Option<&str>) -> bool {
    branch.is_some_and(|branch| LONG_LIVED_BRANCHES.contains(&branch))
}

/// Lists, and with `apply` removes, the worktrees that already landed.
pub fn worktree_prune(root: &Path, apply: bool) -> Result<()> {
    let home = home()?;
    let managed = managed_checkouts(&home);
    let driver = promote_driver(&home);
    let listing = gate::git_capture(root, &["worktree", "list", "--porcelain"])?;
    let registrations = parse_worktrees(&listing);
    let origin_dev = gate::rev_parse(root, "origin/dev")?;
    let cwds = all_cwds()?;

    println!(
        "worktree-prune: {} registrations, origin/dev {}",
        registrations.len(),
        gate::short(&origin_dev)
    );

    // Every verdict is printed as it is reached rather than collected: one
    // `git status` here can take minutes, and a run that prints nothing for
    // ten of them is indistinguishable from a hung one.
    let mut candidates = Vec::new();
    let mut missing = 0_usize;
    for (index, worktree) in registrations.iter().enumerate() {
        let shown = worktree.path.display();
        let path = canonical_candidate(&worktree.path);
        if !path.is_dir() {
            missing += 1;
            println!(
                "  gone         {shown} — the directory is gone, `git worktree prune` clears it"
            );
            continue;
        }
        // git prints the main checkout first, and only it is not linked.
        let mut facts = WorktreeFacts {
            is_main: index == 0,
            is_dev_checkout: managed.contains(&path),
            is_promote_driver: path == driver,
            long_lived_branch: long_lived(worktree.branch.as_deref()),
            merged: is_merged(root, worktree)?,
            dirty_paths: Vec::new(),
            in_use: is_in_use(&path, &cwds),
            unscannable: false,
        };
        if prune_decision(&facts) == Decision::Prune {
            match bounded_status(&path)? {
                Some(listing) => facts.dirty_paths = significant_dirty(&listing),
                None => facts.unscannable = true,
            }
        }
        match prune_decision(&facts) {
            Decision::Prune => {
                println!(
                    "  prune  {:>5}  {shown} ({})",
                    disk_usage(&path),
                    describe(worktree)
                );
                candidates.push((worktree.clone(), path));
            }
            Decision::Keep(Keep::Dirty) => println!(
                "  keep         {shown} — {} ({})",
                Keep::Dirty.reason(),
                summarize(&facts.dirty_paths)
            ),
            Decision::Keep(keep) => println!("  keep         {shown} — {}", keep.reason()),
        }
    }

    println!(
        "worktree-prune: {} candidate(s), {} kept, {missing} gone.",
        candidates.len(),
        registrations.len() - candidates.len() - missing
    );
    if candidates.is_empty() {
        return Ok(());
    }
    if !apply {
        println!("worktree-prune: dry run; rerun with --apply to remove them.");
        return Ok(());
    }
    remove(root, &candidates)
}

/// Removes each candidate, then the branches that are now unused, then the
/// stale registrations. `--force` is required because these checkouts carry
/// ignored build output and symlinked dependency trees.
fn remove(root: &Path, candidates: &[(Registration, PathBuf)]) -> Result<()> {
    let mut removed = Vec::new();
    for (worktree, path) in candidates {
        let output = gate::git(
            root,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        )?;
        if output.status.success() {
            println!("worktree-prune: removed {}", worktree.path.display());
            removed.push(worktree.clone());
        } else {
            println!(
                "worktree-prune: could not remove {} ({})",
                worktree.path.display(),
                output.status
            );
        }
    }

    // A branch another worktree still has checked out is not this command's
    // to delete, so the listing is re-read after the removals.
    let still_checked_out: BTreeSet<String> = parse_worktrees(&gate::git_capture(
        root,
        &["worktree", "list", "--porcelain"],
    )?)
    .into_iter()
    .filter_map(|worktree| worktree.branch)
    .collect();
    for worktree in &removed {
        let Some(branch) = &worktree.branch else {
            continue;
        };
        if long_lived(Some(branch)) {
            println!("worktree-prune: kept branch {branch}, it is never deleted");
            continue;
        }
        if still_checked_out.contains(branch) {
            println!("worktree-prune: kept branch {branch}, checked out elsewhere");
            continue;
        }
        let output = gate::git(root, &["branch", "-d", branch])?;
        if output.status.success() {
            println!("worktree-prune: deleted branch {branch}");
        } else {
            println!("worktree-prune: kept branch {branch} ({})", output.status);
        }
    }

    gate::git_capture(root, &["worktree", "prune"])?;
    println!("worktree-prune: pruned the stale registrations.");
    Ok(())
}

/// How many uncommitted paths one reason line names before it counts the rest.
/// A worktree mid-refactor carries hundreds, and the reason is the point.
const DIRT_SHOWN: usize = 3;

/// The first few uncommitted paths, and how many more there are.
pub fn summarize(paths: &[String]) -> String {
    let shown = paths.iter().take(DIRT_SHOWN).cloned().collect::<Vec<_>>();
    match paths.len().checked_sub(DIRT_SHOWN) {
        Some(0) | None => shown.join(", "),
        Some(rest) => format!("{}, +{rest} more", shown.join(", ")),
    }
}

/// Whether a worktree's branch — or, detached, its commit — already reached
/// `origin/dev`.
fn is_merged(root: &Path, worktree: &Registration) -> Result<bool> {
    let Some(revision) = worktree.branch.as_ref().or(worktree.head.as_ref()) else {
        return Ok(false);
    };
    let status = gate::git_command(
        root,
        &["merge-base", "--is-ancestor", revision, "origin/dev"],
        gate::GitStderr::Null,
    )
    .status()
    .context("failed to launch git merge-base")?;
    Ok(status.success())
}

/// One `lsof` pass for every process on the machine. It exits non-zero when it
/// cannot read some process it does not own, which is normal and not a reason
/// to discard the directories it did report.
fn all_cwds() -> Result<Vec<PathBuf>> {
    let output = Command::new("lsof")
        .args(["-F", "n", "-d", "cwd"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("failed to launch lsof")?;
    Ok(parse_cwds(&String::from_utf8_lossy(&output.stdout)))
}

/// What `du -sh` says a candidate costs, or `?` when it cannot say.
fn disk_usage(path: &Path) -> String {
    Command::new("du")
        .arg("-sh")
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "?".to_owned())
}

fn describe(worktree: &Registration) -> String {
    worktree.branch.clone().unwrap_or_else(|| {
        format!(
            "detached at {}",
            worktree.head.as_deref().map_or("?", gate::short)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> WorktreeFacts {
        WorktreeFacts {
            is_main: false,
            is_dev_checkout: false,
            is_promote_driver: false,
            long_lived_branch: false,
            merged: true,
            dirty_paths: Vec::new(),
            in_use: false,
            unscannable: false,
        }
    }

    #[test]
    fn an_unscannable_worktree_is_kept() {
        assert_eq!(
            prune_decision(&WorktreeFacts {
                unscannable: true,
                ..facts()
            }),
            Decision::Keep(Keep::Unscannable)
        );
    }

    #[test]
    fn prune_decision_covers_every_reason() {
        assert_eq!(prune_decision(&facts()), Decision::Prune);
        assert_eq!(
            prune_decision(&WorktreeFacts {
                is_main: true,
                ..facts()
            }),
            Decision::Keep(Keep::Main)
        );
        assert_eq!(
            prune_decision(&WorktreeFacts {
                is_dev_checkout: true,
                ..facts()
            }),
            Decision::Keep(Keep::DevCheckout)
        );
        assert_eq!(
            prune_decision(&WorktreeFacts {
                is_promote_driver: true,
                ..facts()
            }),
            Decision::Keep(Keep::PromoteDriver)
        );
        assert_eq!(
            prune_decision(&WorktreeFacts {
                long_lived_branch: true,
                ..facts()
            }),
            Decision::Keep(Keep::LongLivedBranch)
        );
        assert_eq!(
            prune_decision(&WorktreeFacts {
                merged: false,
                ..facts()
            }),
            Decision::Keep(Keep::Unmerged)
        );
        assert_eq!(
            prune_decision(&WorktreeFacts {
                dirty_paths: vec!["crates/interface-core/src/lib.rs".to_owned()],
                ..facts()
            }),
            Decision::Keep(Keep::Dirty)
        );
        assert_eq!(
            prune_decision(&WorktreeFacts {
                in_use: true,
                ..facts()
            }),
            Decision::Keep(Keep::InUse)
        );
        // Identity outranks everything: the Dev checkout is merged, clean and
        // idle between restarts, and must still never be removed.
        assert_eq!(
            prune_decision(&WorktreeFacts {
                is_dev_checkout: true,
                merged: false,
                dirty_paths: vec!["electron/package.json".to_owned()],
                in_use: true,
                ..facts()
            }),
            Decision::Keep(Keep::DevCheckout)
        );
        // `in_use` outranks `dirty_paths` so the scan can answer without the
        // working-tree walk that fills `dirty_paths` in the first place.
        assert_eq!(
            prune_decision(&WorktreeFacts {
                in_use: true,
                dirty_paths: vec!["electron/package.json".to_owned()],
                ..facts()
            }),
            Decision::Keep(Keep::InUse)
        );
    }

    #[test]
    fn the_dev_checkout_dev_restart_refreshes_is_managed() {
        // Resolved the same way as a registration, so the comparison in
        // `worktree_prune` sees one path whatever links lie on the way.
        let home = Path::new("/nonexistent-home");
        let managed = managed_checkouts(home);
        for relative in [
            DEV_CHECKOUT,
            LEGACY_DEV_CHECKOUT,
            DEFAULT_RELEASE_WORKTREE,
            LEGACY_RELEASE_WORKTREE,
            PROVE_WORKTREE,
        ] {
            assert!(
                managed.contains(&canonical_candidate(&home.join(relative))),
                "{relative} is not protected"
            );
        }
        assert!(long_lived(Some("dev")) && long_lived(Some("main")));
        assert!(!long_lived(Some("cleanup/dev")) && !long_lived(None));
    }

    #[test]
    fn the_promote_driver_is_kept_as_itself() {
        let home = Path::new("/nonexistent-home");
        let driver = promote_driver(home);
        assert_eq!(
            driver,
            canonical_candidate(Path::new("/nonexistent-home/Developer/.interface-promote"))
        );
        assert!(
            !managed_checkouts(home).contains(&driver),
            "the driver has its own keep reason"
        );
        // Detached at an ancestor of origin/dev, clean and idle between
        // releases: everything else would prune it.
        assert_eq!(
            prune_decision(&WorktreeFacts {
                is_promote_driver: true,
                ..facts()
            }),
            Decision::Keep(Keep::PromoteDriver)
        );
        assert!(Keep::PromoteDriver.reason().contains("promote driver"));
    }

    #[test]
    fn summarize_counts_the_paths_it_does_not_name() {
        assert_eq!(summarize(&[]), "");
        let three = ["a.rs", "b.rs", "c.rs"].map(str::to_owned);
        assert_eq!(summarize(&three), "a.rs, b.rs, c.rs");
        let four = ["a.rs", "b.rs", "c.rs", "d.rs"].map(str::to_owned);
        assert_eq!(summarize(&four), "a.rs, b.rs, c.rs, +1 more");
    }

    #[test]
    fn significant_dirty_keeps_only_real_changes() {
        let status = "?? node_modules/\n?? electron/node_modules/\n?? target/\n\
                      ?? target.nosync/\n?? .grok/\n";
        assert!(significant_dirty(status).is_empty());
        assert_eq!(
            significant_dirty(" M crates/interface-core/src/lib.rs\n?? target/\n"),
            vec!["crates/interface-core/src/lib.rs".to_owned()]
        );
        // A name that only starts like an ignorable one still counts.
        assert_eq!(
            significant_dirty("?? target-notes.md\n"),
            vec!["target-notes.md".to_owned()]
        );
        assert!(significant_dirty("").is_empty());
    }

    #[test]
    fn parse_worktrees_reads_branches_and_detached_heads() {
        let listing = "worktree /Users/a/Projects/Interface-Rust\n\
                       HEAD 1111111111111111111111111111111111111111\n\
                       branch refs/heads/dev\n\n\
                       worktree /private/tmp/interface-audit\n\
                       HEAD 2222222222222222222222222222222222222222\n\
                       branch refs/heads/audit/xtask\n\n\
                       worktree /Users/a/.interface-release\n\
                       HEAD 3333333333333333333333333333333333333333\n\
                       detached\n";
        let worktrees = parse_worktrees(listing);
        assert_eq!(worktrees.len(), 3);
        assert_eq!(worktrees[0].branch.as_deref(), Some("dev"));
        assert_eq!(worktrees[1].path, Path::new("/private/tmp/interface-audit"));
        assert_eq!(worktrees[1].branch.as_deref(), Some("audit/xtask"));
        assert_eq!(worktrees[2].branch, None);
        assert_eq!(
            worktrees[2].head.as_deref(),
            Some("3333333333333333333333333333333333333333")
        );
    }

    #[test]
    fn registered_finds_a_listed_checkout_and_its_branch() {
        let listing = "worktree /Users/a/Desktop/Projects/Interface-Rust\n\
                       HEAD 7ef25074deadbeefdeadbeefdeadbeefdeadbeef\n\
                       branch refs/heads/dev\n\
                       \n\
                       worktree /Users/a/Desktop/Projects/.interface-dev\n\
                       HEAD 7ef25074deadbeefdeadbeefdeadbeefdeadbeef\n\
                       branch refs/heads/dev\n\
                       \n\
                       worktree /Users/a/Desktop/Projects/.interface-release\n\
                       HEAD 0123456789abcdef0123456789abcdef01234567\n\
                       detached\n";
        let branch = |path: &str| registered(listing, Path::new(path)).map(|found| found.branch);
        assert_eq!(
            branch("/Users/a/Desktop/Projects/.interface-dev"),
            Some(Some("dev".to_owned())),
            "the Dev app's checkout is on dev and must never be detached by promote"
        );
        assert_eq!(
            branch("/Users/a/Desktop/Projects/.interface-release"),
            Some(None),
            "a detached worktree is exactly what promote wants"
        );
        assert_eq!(
            branch("/Users/a/Desktop/Projects/.interface-new"),
            None,
            "a path git does not list yet is free"
        );
        assert_eq!(registered("", Path::new("/Users/a")), None);
    }

    #[test]
    fn in_use_matches_whole_components_only() {
        let cwds =
            parse_cwds("p1\nfcwd\nn/private/tmp/interface-audit-xtask/xtask\np2\nfcwd\nn/\n");
        assert_eq!(cwds.len(), 2);
        let path = Path::new("/private/tmp/interface-audit-xtask");
        assert!(is_in_use(path, &cwds));
        assert!(!is_in_use(Path::new("/private/tmp/interface-audit"), &cwds));
        assert!(!is_in_use(
            Path::new("/private/tmp/interface-audit-x"),
            &cwds
        ));
    }
}
