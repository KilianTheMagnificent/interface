//! `cargo xtask closeout`: validate the integrated tree, land, then verify Dev.
//!
//! The recipe is `git fetch origin dev main`, `git merge --no-edit <the dev tip>`
//! into the task branch, then one landing commit pushed to `dev`: a merge
//! whose first parent is that dev tip, whose second parent is the branch tip,
//! whose tree is the branch's tree, whose subject is the `--change` text, and
//! which carries `Landed-from: <branch>`. The landing is written with `git
//! commit-tree`, so `dev` is never checked out, and the branch ref is then
//! moved onto it with `git update-ref`, so the agent ends at the dev tip as
//! before. Every landing is a first-parent merge on `dev`, which is what lets
//! `cargo xtask landings` list them and `promote --change` ship one with a
//! `-m 1` cherry-pick. The refusals an agent is supposed to apply by hand
//! stay: never close out a dirty tree, never push from `dev`, `main`, or a
//! detached HEAD, never land without a name. A conflicted merge is left in
//! place for the operator: this command never resolves a conflict automatically.
//!
//! Several agents close out at once, and `refs/remotes/origin/dev` is shared
//! by every worktree of this repository, so an attempt reads that ref once and
//! then works from the SHA: merging the name and later reading it again could
//! give the landing a first parent this tree never merged, and pushing that
//! would fast-forward `dev` straight over the other agent's landing. Two
//! races share two attempts and no more — the push rejected as
//! non-fast-forward, and the pinned tip no longer contained in the branch when
//! the landing is written — after which the agent is told to rerun.
//!
//! Two refusals are about content. A push that changes a SETTLED LOOK — a
//! visual decision the owner recorded in `docs/SETTLED_LOOKS.md` — must say so
//! with a `Settled-look:` trailer on one of its commits. A push that changes a
//! test a shipped landing PINS as proof of its behavior must carry
//! `Supersedes:` naming that landing (`pins.rs`). Tests are rewritten by
//! whoever changes the code; these are the steps that make a reversal
//! something an agent has to say out loud.
//!
//! Every landing also says what it is: `--kind`, `--check` and `--pin` become
//! trailers on the landing merge (`landings.rs`), and any `Supersedes:` the
//! branch's commits carry is copied there, so a `-x` cherry-pick of the
//! landing alone still carries it.

use std::{fmt::Write as _, fs, path::Path, process::Output};

use anyhow::{Context, Result, bail};

use crate::{gate, landings::LandingMeta};

/// Why a closeout cannot start, or `Ok` when the three commands may run.
///
/// Kept free of process calls so the refusals are testable: every one of them
/// describes work the operator has to do before rerunning.
pub fn closeout_guard(branch: &str, detached: bool, dirty: bool) -> Result<(), String> {
    if dirty {
        return Err(
            "the working tree has uncommitted tracked changes; commit them (or stash them) \
             before closing out"
                .to_owned(),
        );
    }
    if detached {
        return Err(
            "HEAD is detached; put the work on a task branch (`git switch -c <task-branch>`) \
             before closing out"
                .to_owned(),
        );
    }
    if branch == "dev" || branch == "main" {
        return Err(format!(
            "HEAD is on `{branch}`; closeout pushes a task branch to dev, so move the work onto \
             one first (`git switch -c <task-branch>`)"
        ));
    }
    Ok(())
}

/// The ledger `docs/SETTLED_LOOKS.md` and the trailer it asks for.
pub const SETTLED_LOOKS_LEDGER: &str = "docs/SETTLED_LOOKS.md";
pub const SETTLED_LOOK_TRAILER: &str = "Settled-look:";
/// The source the guard scans; the ledger and the vault are not source.
const SETTLED_LOOK_PATHS: [&str; 3] = ["electron/src", "apps", "crates"];

/// One entry of the ledger: its heading, and the strings that carry the look.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettledLook {
    pub name: String,
    pub markers: Vec<String>,
}

/// Every `## ` entry of the ledger with the backticked strings on its
/// `- Markers:` line. An entry without markers guards nothing and is skipped.
pub fn parse_settled_looks(ledger: &str) -> Vec<SettledLook> {
    let mut looks = Vec::new();
    let mut current: Option<SettledLook> = None;
    for line in ledger.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            if let Some(look) = current.take().filter(|look| !look.markers.is_empty()) {
                looks.push(look);
            }
            current = Some(SettledLook {
                name: name.trim().to_owned(),
                markers: Vec::new(),
            });
            continue;
        }
        let Some(look) = current.as_mut() else {
            continue;
        };
        if let Some(rest) = line.trim_start().strip_prefix("- Markers:") {
            look.markers.extend(
                rest.split('`')
                    .skip(1)
                    .step_by(2)
                    .map(str::trim)
                    .filter(|marker| !marker.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    if let Some(look) = current.filter(|look| !look.markers.is_empty()) {
        looks.push(look);
    }
    looks
}

/// The entries whose markers appear on a changed line of `diff` (a unified
/// diff: only `+`/`-` content lines count, never the `+++`/`---` headers).
pub fn settled_looks_touched(looks: &[SettledLook], diff: &str) -> Vec<String> {
    let changed: Vec<&str> = diff
        .lines()
        .filter(|line| {
            (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"))
        })
        .collect();
    looks
        .iter()
        .filter(|look| {
            look.markers
                .iter()
                .any(|marker| changed.iter().any(|line| line.contains(marker.as_str())))
        })
        .map(|look| look.name.clone())
        .collect()
}

/// The `Settled-look:` trailers in a run of commit messages.
pub fn settled_look_trailers(messages: &str) -> Vec<String> {
    messages
        .lines()
        .filter_map(|line| line.trim().strip_prefix(SETTLED_LOOK_TRAILER))
        .map(|rest| rest.trim().to_owned())
        .filter(|rest| !rest.is_empty())
        .collect()
}

/// `Ok` with the settled looks this push changes (possibly none), or the
/// refusal naming the ones changed without a trailer that names them.
pub fn settled_look_guard(
    looks: &[SettledLook],
    diff: &str,
    messages: &str,
) -> Result<Vec<String>, String> {
    let touched = settled_looks_touched(looks, diff);
    let trailers = settled_look_trailers(messages);
    let unapproved: Vec<&String> = touched
        .iter()
        .filter(|name| {
            !trailers
                .iter()
                .any(|trailer| trailer.to_lowercase().contains(&name.to_lowercase()))
        })
        .collect();
    if unapproved.is_empty() {
        return Ok(touched);
    }
    let mut message = String::from(
        "this push changes a settled look without saying so. The owner settled these and \
         only the owner can change them; get the approval in your session, then add\n",
    );
    for name in &unapproved {
        let _ = writeln!(
            message,
            "    {SETTLED_LOOK_TRAILER} {name} — approved by the owner <date>"
        );
    }
    let _ = write!(
        message,
        "to a commit in the push and update the entry in {SETTLED_LOOKS_LEDGER}. If the change \
         is accidental, restore the lines the markers name."
    );
    Err(message)
}

/// Runs the settled-look guard over what this push would add to dev, measured
/// against the dev tip this attempt pinned rather than the shared `origin/dev`
/// ref another agent's fetch can move mid-run.
fn guard_settled_looks(root: &Path, dev: &str) -> Result<()> {
    let Ok(ledger) = fs::read_to_string(root.join(SETTLED_LOOKS_LEDGER)) else {
        return Ok(());
    };
    let looks = parse_settled_looks(&ledger);
    if looks.is_empty() {
        return Ok(());
    }
    let mut arguments = vec!["diff", dev, "HEAD", "--unified=0", "--"];
    arguments.extend(SETTLED_LOOK_PATHS);
    let diff = gate::git_capture(root, &arguments)?;
    let range = format!("{dev}..HEAD");
    let messages = gate::git_capture(root, &["log", &range, "--format=%B"])?;
    match settled_look_guard(&looks, &diff, &messages) {
        Ok(touched) => {
            for name in touched {
                println!(
                    "closeout: this push changes the settled look \"{name}\" (trailer present)"
                );
            }
            Ok(())
        }
        Err(refusal) => bail!("closeout refused: {refusal}"),
    }
}

/// Closeout's `--kind`, `--check` and `--pin`, as given.
#[derive(Clone, Debug, Default)]
pub struct LandingRequest {
    pub kind: Option<String>,
    pub check: Option<String>,
    pub pins: Vec<String>,
}

/// Validates what the landing says it is against the merged tree: the kind
/// rules, the new pins, and the branch's `Supersedes:` trailers.
fn landing_meta(root: &Path, dev: &str, request: &LandingRequest) -> Result<LandingMeta> {
    let docs_only = !product_changed(root, dev, "HEAD")?;
    let mut meta = crate::landings::landing_meta(
        request.kind.as_deref(),
        request.check.as_deref(),
        &request.pins,
        docs_only,
    )
    .map_err(|refusal| anyhow::anyhow!("closeout refused: {refusal}"))?;
    crate::pins::validate_new(root, "HEAD", &meta.pins)?;
    meta.supersedes = crate::pins::branch_supersedes(root, dev)?;
    Ok(meta)
}

/// Runs the closeout for the checkout at `root`, landing it under `change`.
pub fn closeout(root: &Path, change: Option<&str>, request: &LandingRequest) -> Result<()> {
    closeout_with(
        root,
        change,
        request,
        |root, base| gate::run_gate(root, true, base, false),
        |sha, product| {
            if product {
                crate::dev_restart::dev_restart(Some(sha))
            } else {
                Ok(())
            }
        },
    )
}

fn closeout_with(
    root: &Path,
    change: Option<&str>,
    request: &LandingRequest,
    validate: impl Fn(&Path, &str) -> Result<()>,
    refresh: impl Fn(&str, bool) -> Result<()>,
) -> Result<()> {
    let branch = current_branch(root)?;
    let dirty = !gate::tree_is_clean(root)?;
    if let Err(refusal) = closeout_guard(
        branch.as_deref().unwrap_or_default(),
        branch.is_none(),
        dirty,
    ) {
        bail!("closeout refused: {refusal}");
    }
    let branch = branch.unwrap_or_default();
    let initial_head = gate::rev_parse(root, "HEAD")?;

    // Two attempts, no more, shared by both races: another agent can land on
    // dev between this fetch and this push, and another worktree's fetch can
    // move the shared `origin/dev` ref between this merge and this commit-tree.
    for attempt in 1..=2_u32 {
        fetch(root)?;
        // Every step of the attempt reads this one SHA, never the `origin/dev`
        // name again: remote-tracking refs are shared by every worktree of this
        // repository, so the name can point somewhere else a moment later.
        let dev = gate::rev_parse(root, "origin/dev")?;
        if let Some((landing, product)) =
            resumed_landing(root, &initial_head, &dev, &branch, change, request)?
        {
            return refresh(&landing, product);
        }
        let base = gate::git_capture(root, &["merge-base", &initial_head, &dev])?;
        merge(root, &dev)?;
        let stale = |error| explain_stale_tooling(root, &initial_head, error);
        guard_settled_looks(root, &dev).map_err(stale)?;
        // Metadata first: a misspelled `Supersedes:` gets its own refusal
        // rather than the guard's generic one.
        let meta = landing_meta(root, &dev, request).map_err(stale)?;
        crate::pins::guard(root, &dev).map_err(stale)?;
        let checked_head = gate::rev_parse(root, "HEAD")?;
        validate(root, base.trim())?;
        if gate::rev_parse(root, "HEAD")? != checked_head || !gate::tree_is_clean(root)? {
            bail!(
                "closeout: source changed during the gate; nothing pushed, rerun on a stable tree"
            );
        }
        let Some(landed) = land(root, &branch, change, &meta, &dev)? else {
            if attempt == 1 {
                println!("closeout: dev moved under the merge; fetching and merging once more");
                continue;
            }
            bail!(
                "dev moved under the merge twice; nothing was pushed. Rerun \
                 `cargo xtask closeout`"
            );
        };
        if landed.head != checked_head {
            bail!("closeout: HEAD changed after validation; nothing pushed");
        }
        let push = git_output(root, &["push", "origin", &format!("{}:dev", landed.sha)])?;
        let stderr = String::from_utf8_lossy(&push.stderr).into_owned();
        eprint!("{stderr}");
        // A push can fail after the remote took it: a connection dropped once
        // dev moved. Only a rejection is certain, so anything else reads dev
        // back before calling the landing lost.
        let reached_dev = !push.status.success()
            && !is_non_fast_forward(&stderr)
            && dev_carries(root, &landed.sha);
        if reached_dev {
            println!(
                "closeout: the push reported {}, but origin/dev carries the landing",
                push.status
            );
        }
        if push.status.success() || reached_dev {
            // The landing's tree is the working tree, so only the ref moves;
            // naming the tip `land` merged makes git refuse if this shared
            // checkout was switched mid-run.
            gate::git_capture(
                root,
                &[
                    "update-ref",
                    &format!("refs/heads/{branch}"),
                    &landed.sha,
                    &landed.head,
                ],
            )
            .with_context(|| {
                format!(
                    "the landing is already on dev; rerun `cargo xtask closeout` to move \
                     {branch} onto it"
                )
            })?;
            crate::update_log::sync_from_head(root)?;
            println!(
                "closeout: landing \"{}\" ({})",
                landed.name,
                meta.kind.as_str()
            );
            println!(
                "closeout: landed {branch} on dev as {}..{}",
                gate::short(&dev),
                gate::short(&landed.sha)
            );
            record_landing(root, &landed.name, &landed.sha, &dev, &branch)?;
            return refresh(&landed.sha, product_changed(root, &dev, &landed.sha)?).context(
                "landing is on dev; Dev refresh failed. Run cargo xtask dev-restart to finish",
            );
        }
        if attempt == 1 && is_non_fast_forward(&stderr) {
            println!("closeout: dev moved under the push; fetching and merging once more");
            continue;
        }
        bail!(
            "`git push origin {}:dev` failed with {}",
            landed.sha,
            push.status
        );
    }
    unreachable!("the loop either pushes or bails")
}

/// What decides a closeout's guards and gate: the xtask binary running this
/// closeout is built from these as they were before its merge of dev.
const TOOLING_SOURCES: [&str; 3] = ["xtask/", "crates/interface-common/", "Cargo.lock"];

/// Passes a guard's refusal through, first saying when the merge just
/// changed the tooling that made it. This run's binary predates that merge,
/// so a guard fix already on dev did not run; a rerun builds the merged
/// xtask, and the merge commit it needs stays on the branch.
fn explain_stale_tooling(root: &Path, before_merge: &str, error: anyhow::Error) -> anyhow::Error {
    let changed = gate::changed_names(root, before_merge, "HEAD").unwrap_or_default();
    if changed.iter().any(|name| {
        TOOLING_SOURCES
            .iter()
            .any(|source| name.starts_with(source))
    }) {
        eprintln!(
            "closeout: the merge of dev changed xtask, and this run's xtask was built before it; \
             rerun `cargo xtask closeout` with the same arguments to decide with the merged code"
        );
    }
    error
}

/// The explicitly given flags a resumed landing's message disagrees with.
fn resume_mismatch(message: &str, request: &LandingRequest) -> Option<&'static str> {
    let values = |key: &str| -> Vec<String> {
        message
            .lines()
            .filter_map(|line| line.strip_prefix(key))
            .map(|value| value.trim().to_owned())
            .collect()
    };
    let kind = request
        .kind
        .as_deref()
        .map(|kind| crate::landings::Kind::parse(kind).map_or(kind.trim(), |k| k.as_str()));
    if kind.is_some_and(|kind| values(crate::landings::KIND_TRAILER) != [kind]) {
        return Some("--kind");
    }
    if request
        .check
        .as_deref()
        .is_some_and(|check| values(crate::landings::CHECK_TRAILER) != [check.trim()])
    {
        return Some("--check");
    }
    let pins: Vec<String> = request
        .pins
        .iter()
        .map(|pin| pin.trim().to_owned())
        .collect();
    if !pins.is_empty() && values(crate::landings::PIN_TRAILER) != pins {
        return Some("--pin");
    }
    None
}

/// The landing a rerun resumes, and whether it changed the product: HEAD
/// itself once the push and the branch move finished, or the first-parent
/// landing on dev that merged HEAD when the push reached dev but reported
/// failure, so the branch never moved. The branch is moved onto that one.
fn resumed_landing(
    root: &Path,
    initial_head: &str,
    dev: &str,
    branch: &str,
    change: Option<&str>,
    request: &LandingRequest,
) -> Result<Option<(String, bool)>> {
    if !gate::is_ancestor(root, initial_head, dev)? {
        return Ok(None);
    }
    let landed_from = |message: &str| {
        message
            .lines()
            .any(|line| line == format!("Landed-from: {branch}"))
    };
    let mut landing = initial_head.to_owned();
    let mut message = gate::git_capture(root, &["log", "-1", "--format=%B", initial_head])?;
    if !landed_from(&message) {
        let Some(merging) = landing_merging(root, initial_head, dev)? else {
            return Ok(None);
        };
        message = gate::git_capture(root, &["log", "-1", "--format=%B", &merging])?;
        if !landed_from(&message) {
            return Ok(None);
        }
        landing = merging;
    }
    if change.is_some_and(|name| Some(name.trim()) != message.lines().next()) {
        bail!("closeout refused: nothing to land; resume under the original landing name");
    }
    if let Some(flag) = resume_mismatch(&message, request) {
        bail!(
            "closeout refused: nothing to land; this landing was written with a different {flag}. \
             Resume with the original flags, or land a new change"
        );
    }
    let parent = gate::rev_parse(root, &format!("{landing}^1"))?;
    if landing != initial_head {
        // As after a push that reported success: the landing's tree is HEAD's,
        // so only the ref moves.
        gate::git_capture(
            root,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                &landing,
                initial_head,
            ],
        )?;
        crate::update_log::sync_from_head(root)?;
        let name = message.lines().next().unwrap_or_default().trim();
        println!(
            "closeout: dev already carries landing \"{name}\" ({}); moved {branch} onto it",
            gate::short(&landing)
        );
        record_landing(root, name, &landing, &parent, branch)?;
    }
    let product = product_changed(root, &parent, &landing)?;
    Ok(Some((landing, product)))
}

/// The first-parent commit on dev after `head` whose second parent is `head`:
/// the landing that merged it.
fn landing_merging(root: &Path, head: &str, dev: &str) -> Result<Option<String>> {
    let listing = gate::git_capture(
        root,
        &[
            "rev-list",
            "--first-parent",
            "--parents",
            &format!("{head}..{dev}"),
        ],
    )?;
    Ok(listing.lines().find_map(|line| {
        let mut shas = line.split_whitespace();
        let commit = shas.next()?;
        (shas.nth(1)? == head).then(|| commit.to_owned())
    }))
}

/// Whether the remote's dev, read back, contains `landing`.
fn dev_carries(root: &Path, landing: &str) -> bool {
    fetch(root).is_ok()
        && gate::rev_parse(root, "origin/dev")
            .and_then(|tip| gate::is_ancestor(root, landing, &tip))
            .unwrap_or(false)
}

/// Records the last landing for `status`.
fn record_landing(root: &Path, name: &str, sha: &str, parent: &str, branch: &str) -> Result<()> {
    let record = crate::pipeline_state::directory(root)?.join("landing.json");
    crate::pipeline_state::write(
        &record,
        &serde_json::json!({
            "name": name, "sha": sha, "parent": parent,
            "branch": branch, "at": crate::pipeline_state::now(),
        }),
    )
}

fn product_changed(root: &Path, from: &str, to: &str) -> Result<bool> {
    Ok(gate::changed_names(root, from, to)?
        .iter()
        .any(|name| !gate::is_documentation(name)))
}

/// The landing [`land`] wrote: the commit, the name it carries, and the branch
/// tip it merged, which is the ref value the branch is moved from.
struct Landed {
    sha: String,
    name: String,
    head: String,
}

/// Writes the landing commit for the merged branch, or `Ok(None)` when the dev
/// tip this attempt pinned is no longer contained in the branch — the merge
/// lost its race and the caller has to fetch and merge again. Nothing in the
/// working tree changes either way.
fn land(
    root: &Path,
    branch: &str,
    change: Option<&str>,
    meta: &LandingMeta,
    dev: &str,
) -> Result<Option<Landed>> {
    let range = format!("{dev}..HEAD");
    let added = gate::git_capture(root, &["rev-list", "--count", &range])?;
    if added.trim() == "0" {
        bail!("closeout refused: nothing to land; {branch} adds nothing to origin/dev");
    }
    let subjects: Vec<String> = gate::git_capture(
        root,
        &["log", "--reverse", "--no-merges", "--format=%s", &range],
    )?
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .map(str::to_owned)
    .collect();
    let name = match crate::landings::landing_name(change, &subjects) {
        Ok(name) => name,
        Err(refusal) => bail!("closeout refused: {refusal}"),
    };
    let message = crate::landings::landing_message(&name, &subjects, meta, branch);
    let file = tempfile::Builder::new()
        .prefix("interface-landing-")
        .tempfile()
        .context("could not create the landing message file")?;
    fs::write(file.path(), message)
        .with_context(|| format!("could not write {}", file.path().display()))?;
    let head = gate::rev_parse(root, "HEAD")?;
    let mut tree = gate::rev_parse(root, &format!("{head}^{{tree}}"))?;
    // The invariant every landing rests on: the first parent must be a commit
    // the branch already merged. Without this, a fetch from another worktree
    // between the merge and here would make the landing's first parent a dev
    // tip this tree never saw, and the push would fast-forward dev straight
    // over that agent's work.
    if !gate::is_ancestor(root, dev, &head)? {
        return Ok(None);
    }
    let (committed, completed_at) = crate::update_log::current_committer_time(root);
    if root.join(crate::update_log::UPDATES_DOC).exists() {
        let pending = crate::update_log::PendingLanding {
            sha: &head,
            completed_at: &completed_at,
            committed,
            name: &name,
            branch,
            subjects: &subjects,
            meta,
        };
        tree = crate::update_log::tree_with_pending_landing(root, dev, &tree, &pending)?;
    }
    let output = gate::git_command(
        root,
        &[
            "commit-tree",
            &tree,
            "-p",
            dev,
            "-p",
            &head,
            "-F",
            &file.path().to_string_lossy(),
        ],
        gate::GitStderr::Capture,
    )
    .env("GIT_AUTHOR_DATE", &completed_at)
    .env("GIT_COMMITTER_DATE", &completed_at)
    .output()
    .context("failed to launch git commit-tree")?;
    if !output.status.success() {
        bail!(
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let landing = String::from_utf8(output.stdout)?;
    Ok(Some(Landed {
        sha: landing.trim().to_owned(),
        name,
        head,
    }))
}

/// Whether a rejected push lost a race rather than failing for good.
pub fn is_non_fast_forward(stderr: &str) -> bool {
    stderr.contains("non-fast-forward")
        || stderr.contains("fetch first")
        || stderr.contains("[rejected]")
}

/// The branch HEAD is on, or `None` when HEAD is detached.
fn current_branch(root: &Path) -> Result<Option<String>> {
    let output = gate::git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = String::from_utf8(output.stdout)
        .context("git printed a non-UTF-8 branch name")?
        .trim()
        .to_owned();
    Ok((!branch.is_empty()).then_some(branch))
}

/// Fetches dev to merge, and main so the pin guard knows what has shipped.
fn fetch(root: &Path) -> Result<()> {
    let output = git_output(root, &["fetch", "origin", "dev", "main"])?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(())
    } else {
        bail!("`git fetch origin dev main` failed with {}", output.status)
    }
}

/// Merges the pinned dev tip into the task branch, naming the conflicting
/// paths and leaving the merge in the tree when it does not apply cleanly. The
/// SHA is merged, not the `origin/dev` name, so the commit the landing takes
/// as its first parent is the one this merge actually resolved.
fn merge(root: &Path, dev: &str) -> Result<()> {
    let output = git_output(root, &["merge", "--no-edit", dev])?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        return Ok(());
    }
    let conflicts =
        gate::git_capture(root, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    let conflicts: Vec<&str> = conflicts
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if conflicts.as_slice() == [crate::update_log::UPDATES_DOC] {
        gate::git_capture(
            root,
            &["checkout", "--theirs", "--", crate::update_log::UPDATES_DOC],
        )?;
        gate::git_capture(root, &["add", "--", crate::update_log::UPDATES_DOC])?;
        gate::git_capture(root, &["commit", "--no-edit"])?;
        return Ok(());
    }
    if conflicts.is_empty() {
        bail!(
            "`git merge --no-edit {}` (the origin/dev tip) failed with {} and reported no \
             conflicting paths; the merge is left in the tree",
            gate::short(dev),
            output.status
        );
    }
    eprintln!("closeout: conflicting paths:");
    for path in &conflicts {
        eprintln!("  {path}");
    }
    bail!(
        "`git merge --no-edit {}` (the origin/dev tip) left {} conflicting path(s); resolve them, \
         gate, and rerun `cargo xtask closeout`",
        gate::short(dev),
        conflicts.len()
    )
}

/// One git command, captured whether or not it succeeded: the push's stderr is
/// the only thing that distinguishes a lost race from a real failure.
pub(crate) fn git_output(root: &Path, arguments: &[&str]) -> Result<Output> {
    gate::git_command(root, arguments, gate::GitStderr::Capture)
        .output()
        .context("failed to launch git")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{git_in, init_repo};
    fn internal() -> LandingRequest {
        LandingRequest {
            kind: Some("internal".to_owned()),
            ..LandingRequest::default()
        }
    }

    fn internal_meta() -> LandingMeta {
        crate::landings::landing_meta(Some("internal"), None, &[], false).expect("internal meta")
    }

    fn closeout_fixture(root: &Path, change: Option<&str>) -> Result<()> {
        closeout_request(root, change, &internal())
    }

    fn closeout_request(root: &Path, change: Option<&str>, request: &LandingRequest) -> Result<()> {
        closeout_with(root, change, request, |_, _| Ok(()), |_, _| Ok(()))
    }

    #[test]
    fn closeout_guard_names_every_reason_it_refuses() {
        assert_eq!(closeout_guard("fix/thing", false, false), Ok(()));

        let dirty = closeout_guard("fix/thing", false, true).unwrap_err();
        assert!(dirty.contains("uncommitted"), "{dirty}");
        // A dirty tree is reported even when HEAD is also unusable.
        assert_eq!(closeout_guard("dev", true, true).unwrap_err(), dirty);

        let detached = closeout_guard("", true, false).unwrap_err();
        assert!(detached.contains("detached"), "{detached}");

        for branch in ["dev", "main"] {
            let refusal = closeout_guard(branch, false, false).unwrap_err();
            assert!(refusal.contains(branch), "{refusal}");
            assert!(refusal.contains("task branch"), "{refusal}");
        }
        // Only those two names are protected.
        assert_eq!(closeout_guard("develop", false, false), Ok(()));
        assert_eq!(closeout_guard("port/main-window", false, false), Ok(()));
    }

    const LEDGER: &str = "# Settled looks\n\nprose with `stray` backticks\n\n\
## Selected terminal pane lifts its head\n\n- Settled: 2026-09-05.\n\
- Markers: `terminal-header-bg-focused`, `.terminal-pane.is-focused .terminal-pane-head`\n\n\
## Entry without markers\n\n- Look: nothing guarded.\n\n\
## Attention tray\n\n- Markers: `<AttentionTray`\n";

    #[test]
    fn parse_settled_looks_reads_headings_and_backticked_markers() {
        let looks = parse_settled_looks(LEDGER);
        assert_eq!(
            looks,
            vec![
                SettledLook {
                    name: "Selected terminal pane lifts its head".to_owned(),
                    markers: vec![
                        "terminal-header-bg-focused".to_owned(),
                        ".terminal-pane.is-focused .terminal-pane-head".to_owned(),
                    ],
                },
                SettledLook {
                    name: "Attention tray".to_owned(),
                    markers: vec!["<AttentionTray".to_owned()],
                },
            ]
        );
    }

    #[test]
    fn settled_look_guard_refuses_a_silent_change_and_passes_a_declared_one() {
        let looks = parse_settled_looks(LEDGER);
        // A diff that removes the focused head's token: the a5752b5 shape.
        let diff = "--- a/electron/src/renderer/theme.css\n+++ b/electron/src/renderer/theme.css\n\
-  --terminal-header-bg-focused: #232323;\n+  --terminal-header-bg: #15171a;\n";
        let refusal =
            settled_look_guard(&looks, diff, "fix(renderer): quieter pane chrome\n").unwrap_err();
        assert!(
            refusal.contains("Selected terminal pane lifts its head"),
            "{refusal}"
        );
        assert!(refusal.contains("Settled-look:"), "{refusal}");
        assert!(!refusal.contains("Attention tray"), "{refusal}");
        // The same diff, declared: allowed, and the look is reported.
        let declared = "fix(renderer): flatter pane head\n\n\
Settled-look: Selected terminal pane lifts its head — approved by the owner 2026-09-06\n";
        assert_eq!(
            settled_look_guard(&looks, diff, declared),
            Ok(vec!["Selected terminal pane lifts its head".to_owned()])
        );
        // A trailer for a different look does not cover this one.
        let other = "Settled-look: Attention tray — approved by the owner 2026-09-06\n";
        assert!(settled_look_guard(&looks, diff, other).is_err());
        // Headers are not content: the file-name lines never count as changes.
        let headers_only = "--- a/x\n+++ b/electron/src/terminal-header-bg-focused.css\n";
        assert_eq!(settled_look_guard(&looks, headers_only, ""), Ok(Vec::new()));
        // An untouched push passes with nothing to report.
        assert_eq!(
            settled_look_guard(&looks, "+ let x = 1;\n", ""),
            Ok(Vec::new())
        );
    }

    #[test]
    fn is_non_fast_forward_reads_a_lost_race() {
        assert!(is_non_fast_forward(
            " ! [rejected]        HEAD -> dev (non-fast-forward)\n"
        ));
        assert!(is_non_fast_forward(
            "hint: Updates were rejected because the tip of your current branch is behind\n\
             hint: its remote counterpart. Integrate the remote changes (e.g. 'git pull ...')\n\
             error: failed to push some refs to 'origin'\nfetch first\n"
        ));
        assert!(!is_non_fast_forward(
            "remote: Permission to owner/repo.git denied\nfatal: unable to access\n"
        ));
    }

    /// A fresh work repository wired to the bare `origin`.
    fn work_repo(temp: &Path, name: &str, origin: &Path) -> std::path::PathBuf {
        let dir = temp.join(name);
        init_repo(&dir);
        git_in(
            &dir,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        dir
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn closeout_lands_a_first_parent_merge_that_cherry_picks_onto_main() {
        let temp = tempfile::tempdir().expect("temp dir");
        let origin = temp.path().join("origin.git");
        git_in(
            temp.path(),
            &["init", "--bare", "--quiet", &origin.to_string_lossy()],
        );
        let work = work_repo(temp.path(), "work", &origin);
        fs::write(work.join("README.md"), "base\n").expect("readme");
        git_in(&work, &["add", "README.md"]);
        git_in(&work, &["commit", "--quiet", "-m", "base"]);
        git_in(
            &work,
            &["push", "--quiet", "origin", "HEAD:main", "HEAD:dev"],
        );
        git_in(&work, &["fetch", "--quiet", "origin"]);
        let base = git_in(&work, &["rev-parse", "HEAD"]);

        git_in(&work, &["switch", "--quiet", "-c", "fix/thing"]);
        fs::write(work.join("thing.txt"), "one\n").expect("thing");
        git_in(&work, &["add", "thing.txt"]);
        git_in(&work, &["commit", "--quiet", "-m", "fix: add thing"]);
        fs::write(work.join("thing.txt"), "two\n").expect("thing");
        git_in(&work, &["commit", "--quiet", "-am", "fix: refine thing"]);

        // Two commits and no --change: refused, nothing pushed.
        let refusal = closeout_fixture(&work, None).unwrap_err().to_string();
        assert!(refusal.contains("--change"), "{refusal}");
        assert_eq!(git_in(&work, &["rev-parse", "origin/dev"]), base);

        let tip = git_in(&work, &["rev-parse", "HEAD"]);
        let error = closeout_with(
            &work,
            Some("Adds the thing"),
            &internal(),
            |_, _| Ok(()),
            |_, _| anyhow::bail!("injected Dev refresh failure"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("landing is on dev"));
        let published = git_in(&work, &["rev-parse", "origin/dev"]);
        closeout_fixture(&work, Some("Adds the thing")).expect("refresh resumes");
        assert_eq!(git_in(&work, &["rev-parse", "HEAD"]), published);

        let landing = git_in(&work, &["rev-parse", "HEAD"]);
        assert_eq!(git_in(&work, &["rev-parse", "origin/dev"]), landing);
        assert_eq!(
            git_in(&work, &["symbolic-ref", "--short", "HEAD"]),
            "fix/thing"
        );
        assert_eq!(git_in(&work, &["rev-parse", "HEAD^1"]), base);
        // The second parent is the branch tip the merge was written over.
        assert_eq!(git_in(&work, &["rev-parse", "HEAD^2"]), tip);
        assert_eq!(
            git_in(&work, &["log", "-1", "--format=%s"]),
            "Adds the thing"
        );
        let body = git_in(&work, &["log", "-1", "--format=%b"]);
        assert!(body.contains("fix: add thing\nfix: refine thing"), "{body}");
        assert!(body.contains("Landed-from: fix/thing"), "{body}");
        assert_eq!(
            git_in(&work, &["diff", "--name-only", "HEAD^1", "HEAD"]),
            "thing.txt"
        );
        assert_eq!(
            git_in(
                &work,
                &[
                    "log",
                    "--first-parent",
                    "--merges",
                    "--format=%s",
                    "origin/main..origin/dev"
                ]
            ),
            "Adds the thing"
        );
        assert!(git_in(&work, &["status", "--porcelain"]).is_empty());

        let listed = crate::landings::unshipped_landings(&work).expect("landings read");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].sha, landing);
        assert_eq!(listed[0].name, "Adds the thing");
        assert_eq!(listed[0].branch, "fix/thing");
        assert_eq!(
            crate::landings::landing_paths(&work, &landing).expect("paths"),
            vec!["thing.txt".to_owned()]
        );

        // The landing ships onto main as one cherry-pick that records its source.
        git_in(&work, &["checkout", "--quiet", "--detach", "origin/main"]);
        git_in(&work, &["cherry-pick", "-x", "-m", "1", &landing]);
        assert_eq!(
            fs::read_to_string(work.join("thing.txt")).expect("thing"),
            "two\n"
        );
        let main_body = git_in(&work, &["log", "-1", "--format=%b"]);
        assert!(
            crate::landings::shipped_shas(&main_body).contains(&landing),
            "{main_body}"
        );

        git_in(&work, &["push", "--quiet", "origin", "HEAD:main"]);
        git_in(&work, &["fetch", "--quiet", "origin"]);
        assert!(
            crate::landings::unshipped_landings(&work)
                .expect("landings read")
                .is_empty(),
            "a -x cherry-pick on main marks the landing shipped"
        );

        // A branch that adds nothing after the merge has nothing to land.
        git_in(&work, &["switch", "--quiet", "fix/thing"]);
        let nothing = closeout_fixture(&work, Some("Nothing"))
            .unwrap_err()
            .to_string();
        assert!(nothing.contains("nothing to land"), "{nothing}");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn closeout_finishes_a_landing_whose_push_reached_dev_but_reported_failure() {
        let temp = tempfile::tempdir().expect("temp dir");
        let origin = temp.path().join("origin.git");
        git_in(
            temp.path(),
            &["init", "--bare", "--quiet", &origin.to_string_lossy()],
        );
        let work = work_repo(temp.path(), "work", &origin);
        fs::write(work.join("README.md"), "base\n").expect("readme");
        git_in(&work, &["add", "README.md"]);
        git_in(&work, &["commit", "--quiet", "-m", "base"]);
        git_in(
            &work,
            &["push", "--quiet", "origin", "HEAD:main", "HEAD:dev"],
        );
        // The remote takes every push, then the connection drops.
        let dropping = temp.path().join("receive-then-drop");
        fs::write(
            &dropping,
            "#!/bin/sh\ngit-receive-pack \"$@\"\necho 'connection dropped' >&2\nexit 1\n",
        )
        .expect("receive-pack wrapper");
        fs::set_permissions(
            &dropping,
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .expect("mode");
        git_in(
            &work,
            &[
                "config",
                "remote.origin.receivepack",
                &dropping.to_string_lossy(),
            ],
        );
        let refreshed = std::sync::Mutex::new(Vec::new());
        let closeout = |work: &Path, change: &str| {
            closeout_with(
                work,
                Some(change),
                &internal(),
                |_, _| Ok(()),
                |sha, _| {
                    refreshed.lock().expect("refreshes").push(sha.to_owned());
                    Ok(())
                },
            )
        };

        branch_from_dev(&work, "fix/thing");
        fs::write(work.join("thing.txt"), "one\n").expect("thing");
        git_in(&work, &["add", "thing.txt"]);
        git_in(&work, &["commit", "--quiet", "-m", "fix: add thing"]);
        let tip = git_in(&work, &["rev-parse", "HEAD"]);
        closeout(&work, "Adds the thing").expect("dev read back carries the landing");
        let landing = git_in(&work, &["rev-parse", "origin/dev"]);
        assert_eq!(git_in(&work, &["rev-parse", "HEAD"]), landing);
        assert_eq!(git_in(&work, &["rev-parse", "HEAD^2"]), tip);
        assert_eq!(
            refreshed.lock().expect("refreshes").clone(),
            vec![landing.clone()]
        );
        let record = fs::read_to_string(
            crate::pipeline_state::location(&work)
                .expect("state")
                .join("landing.json"),
        )
        .expect("the landing is recorded");
        assert!(record.contains(&landing), "{record}");

        // A run that gave up on the same failure left its branch behind the
        // landing on dev; the rerun finishes it instead of finding nothing.
        branch_from_dev(&work, "fix/other");
        fs::write(work.join("other.txt"), "other\n").expect("other");
        git_in(&work, &["add", "other.txt"]);
        git_in(&work, &["commit", "--quiet", "-m", "fix: add other"]);
        let tip = git_in(&work, &["rev-parse", "HEAD"]);
        let tree = git_in(&work, &["rev-parse", "HEAD^{tree}"]);
        let written = git_in(
            &work,
            &[
                "commit-tree",
                &tree,
                "-p",
                "origin/dev",
                "-p",
                &tip,
                "-m",
                "Adds other\n\nfix: add other\n\nLanding-kind: internal\nLanded-from: fix/other",
            ],
        );
        git_in(&work, &["config", "--unset", "remote.origin.receivepack"]);
        git_in(
            &work,
            &["push", "--quiet", "origin", &format!("{written}:dev")],
        );
        refreshed.lock().expect("refreshes").clear();
        closeout(&work, "Adds other").expect("the rerun resumes the landing");
        assert_eq!(git_in(&work, &["rev-parse", "HEAD"]), written);
        assert_eq!(
            git_in(&work, &["symbolic-ref", "--short", "HEAD"]),
            "fix/other"
        );
        assert_eq!(refreshed.lock().expect("refreshes").clone(), vec![written]);
        assert!(git_in(&work, &["status", "--porcelain"]).is_empty());
        let refusal = closeout(&work, "Another name").unwrap_err().to_string();
        assert!(refusal.contains("original landing name"), "{refusal}");
    }

    fn request(kind: &str, check: Option<&str>, pins: &[&str]) -> LandingRequest {
        LandingRequest {
            kind: Some(kind.to_owned()),
            check: check.map(str::to_owned),
            pins: pins.iter().map(|pin| (*pin).to_owned()).collect(),
        }
    }

    /// A whole-dev release, as far as the pins can tell: main takes dev.
    fn ship(work: &Path) {
        git_in(work, &["push", "--quiet", "origin", "origin/dev:main"]);
        git_in(work, &["fetch", "--quiet", "origin"]);
    }

    fn branch_from_dev(work: &Path, name: &str) {
        git_in(work, &["fetch", "--quiet", "origin"]);
        git_in(work, &["switch", "--quiet", "-c", name, "origin/dev"]);
    }

    const PINNED: &str = "#[test]\nfn proves_a() {\n    assert!(true);\n}\n";

    #[test]
    #[allow(clippy::too_many_lines)]
    fn closeout_records_what_a_landing_is_and_guards_shipped_pins() {
        let temp = tempfile::tempdir().expect("temp dir");
        let origin = temp.path().join("origin.git");
        git_in(
            temp.path(),
            &["init", "--bare", "--quiet", &origin.to_string_lossy()],
        );
        let work = work_repo(temp.path(), "work", &origin);
        fs::write(work.join("README.md"), "base\n").expect("readme");
        git_in(&work, &["add", "README.md"]);
        git_in(&work, &["commit", "--quiet", "-m", "base"]);
        git_in(
            &work,
            &["push", "--quiet", "origin", "HEAD:main", "HEAD:dev"],
        );
        let base = git_in(&work, &["rev-parse", "HEAD"]);

        // A product landing must say what it is; a feature needs a real pin.
        branch_from_dev(&work, "feat/a");
        fs::write(work.join("tests.rs"), PINNED).expect("tests");
        git_in(&work, &["add", "tests.rs"]);
        git_in(&work, &["commit", "--quiet", "-m", "feat: a"]);
        let no_kind = closeout_request(&work, Some("A"), &LandingRequest::default())
            .unwrap_err()
            .to_string();
        assert!(no_kind.contains("--kind"), "{no_kind}");
        let no_pin = closeout_request(&work, Some("A"), &request("feature", Some("A works"), &[]))
            .unwrap_err()
            .to_string();
        assert!(no_pin.contains("--pin"), "{no_pin}");
        let absent = request("feature", Some("A works"), &["tests.rs::missing"]);
        let absent = closeout_request(&work, Some("A"), &absent)
            .unwrap_err()
            .to_string();
        assert!(absent.contains("does not occur"), "{absent}");
        assert_eq!(
            git_in(&work, &["rev-parse", "origin/dev"]),
            base,
            "nothing pushed"
        );
        let pinned = request("feature", Some("A works"), &["tests.rs::proves_a"]);
        closeout_request(&work, Some("A"), &pinned).expect("A lands");
        let a = git_in(&work, &["rev-parse", "HEAD"]);
        let body = git_in(&work, &["log", "-1", "--format=%b"]);
        assert!(
            body.ends_with(
                "Landing-kind: feature\nLanding-check: A works\n\
                 Landing-pin: tests.rs::proves_a\nLanded-from: feat/a"
            ),
            "{body}"
        );
        let listed = crate::landings::unshipped_landings(&work).expect("landings");
        assert_eq!(listed[0].kind, Some(crate::landings::Kind::Feature));
        assert_eq!(listed[0].pins, vec!["tests.rs::proves_a".to_owned()]);
        ship(&work);

        // A new neighbour after the pinned test is not a change to it.
        branch_from_dev(&work, "fix/b");
        fs::write(
            work.join("tests.rs"),
            format!("{PINNED}\n#[test]\nfn proves_b() {{}}\n"),
        )
        .expect("tests");
        git_in(&work, &["commit", "--quiet", "-am", "test: b"]);
        closeout_fixture(&work, Some("B")).expect("an appended test lands");
        ship(&work);

        // Moving the pinned file away is refused.
        branch_from_dev(&work, "fix/d");
        git_in(&work, &["mv", "tests.rs", "moved.rs"]);
        git_in(&work, &["commit", "--quiet", "-m", "move tests"]);
        let moved = closeout_fixture(&work, Some("D")).unwrap_err().to_string();
        assert!(moved.contains("its file is gone"), "{moved}");

        // Changing a line inside it is refused until a commit supersedes A.
        branch_from_dev(&work, "fix/c");
        let before = git_in(&work, &["rev-parse", "origin/dev"]);
        let edited = fs::read_to_string(work.join("tests.rs"))
            .expect("tests")
            .replace("assert!(true);", "assert!(1 == 1);");
        fs::write(work.join("tests.rs"), edited).expect("tests");
        git_in(&work, &["commit", "--quiet", "-am", "test: reword a"]);
        let refusal = closeout_fixture(&work, Some("C")).unwrap_err().to_string();
        assert!(
            refusal.contains("tests.rs::proves_a — the test changed"),
            "{refusal}"
        );
        assert!(
            refusal.contains(&format!("Supersedes: {}", gate::short(&a))),
            "{refusal}"
        );
        assert_eq!(
            git_in(&work, &["rev-parse", "origin/dev"]),
            before,
            "nothing pushed"
        );
        let typo = "chore: supersede\n\nSupersedes: Nothing like it — reworded";
        git_in(&work, &["commit", "--quiet", "--allow-empty", "-m", typo]);
        let unknown = closeout_fixture(&work, Some("C")).unwrap_err().to_string();
        assert!(unknown.contains("names no landing"), "{unknown}");
        git_in(&work, &["reset", "--quiet", "--hard", "HEAD~1"]);
        let declared = format!(
            "chore: supersede\n\nSupersedes: {} — the assertion is reworded",
            gate::short(&a)
        );
        git_in(
            &work,
            &["commit", "--quiet", "--allow-empty", "-m", &declared],
        );
        closeout_fixture(&work, Some("C")).expect("a declared change lands");
        let body = git_in(&work, &["log", "-1", "--format=%b"]);
        assert!(
            body.contains(&format!(
                "Supersedes: {} — the assertion is reworded",
                gate::short(&a)
            )),
            "the landing carries the declaration for a cherry-pick: {body}"
        );

        // A docs-only landing needs no flags and is recorded as `record`.
        branch_from_dev(&work, "docs/x");
        fs::create_dir_all(work.join("docs")).expect("docs");
        fs::write(work.join("docs/note.md"), "note\n").expect("note");
        git_in(&work, &["add", "docs/note.md"]);
        git_in(&work, &["commit", "--quiet", "-m", "docs: note"]);
        closeout_request(&work, Some("Note"), &LandingRequest::default()).expect("docs land");
        assert!(
            git_in(&work, &["log", "-1", "--format=%b"]).contains("Landing-kind: record"),
            "docs-only is record"
        );
        // Resuming under a different kind is refused.
        let resumed = closeout_request(&work, Some("Note"), &request("fix", None, &[]))
            .unwrap_err()
            .to_string();
        assert!(resumed.contains("different --kind"), "{resumed}");
    }

    /// The race the pinned dev tip exists for: `refs/remotes/origin/dev` is
    /// shared by every worktree of a repository, so another agent's fetch can
    /// move it between this branch's merge and this branch's landing. A
    /// landing written on a tip the branch never merged carries the branch's
    /// stale tree, pushes as a fast-forward, and silently reverts the peer.
    #[test]
    fn closeout_never_lands_on_a_dev_tip_the_branch_did_not_merge() {
        let temp = tempfile::tempdir().expect("temp dir");
        let origin = temp.path().join("origin.git");
        git_in(
            temp.path(),
            &["init", "--bare", "--quiet", &origin.to_string_lossy()],
        );
        let x = work_repo(temp.path(), "x", &origin);
        fs::write(x.join("README.md"), "base\n").expect("readme");
        git_in(&x, &["add", "README.md"]);
        git_in(&x, &["commit", "--quiet", "-m", "base"]);
        git_in(&x, &["push", "--quiet", "origin", "HEAD:main", "HEAD:dev"]);
        git_in(&x, &["fetch", "--quiet", "origin"]);
        let base = git_in(&x, &["rev-parse", "HEAD"]);

        // Y's branch is cut from that same dev tip, and never merges what X
        // lands next.
        let y = work_repo(temp.path(), "y", &origin);
        git_in(&y, &["fetch", "--quiet", "origin"]);
        git_in(&y, &["checkout", "--quiet", "-B", "fix/b", "origin/dev"]);
        fs::write(y.join("b.txt"), "b\n").expect("b");
        git_in(&y, &["add", "b.txt"]);
        git_in(&y, &["commit", "--quiet", "-m", "fix: add b"]);
        let tip = git_in(&y, &["rev-parse", "HEAD"]);

        git_in(&x, &["switch", "--quiet", "-c", "fix/a"]);
        fs::write(x.join("a.txt"), "a\n").expect("a");
        git_in(&x, &["add", "a.txt"]);
        git_in(&x, &["commit", "--quiet", "-m", "fix: add a"]);
        closeout_fixture(&x, Some("X lands a")).expect("X lands");
        let x_landing = git_in(&x, &["rev-parse", "HEAD"]);

        // Y now holds X's landing as its remote-tracking tip while `fix/b`
        // still has not merged it: exactly what a fetch mid-run leaves behind.
        git_in(&y, &["fetch", "--quiet", "origin"]);
        assert_eq!(git_in(&y, &["rev-parse", "origin/dev"]), x_landing);

        // The tip the branch did merge lands. The one it did not is refused
        // before `commit-tree`, so no landing exists to push and no ref moves.
        let merged = land(&y, "fix/b", Some("Y lands b"), &internal_meta(), &base)
            .expect("land runs")
            .expect("a merged dev tip lands");
        assert_eq!(merged.head, tip);
        assert_eq!(
            git_in(&y, &["rev-parse", &format!("{}^1", merged.sha)]),
            base
        );
        assert!(
            land(&y, "fix/b", Some("Y lands b"), &internal_meta(), &x_landing)
                .expect("land runs")
                .is_none(),
            "a dev tip fix/b never merged must not become a landing's first parent"
        );
        assert_eq!(git_in(&y, &["rev-parse", "refs/heads/fix/b"]), tip);
        assert_eq!(git_in(&y, &["rev-parse", "HEAD"]), tip);
        assert_eq!(git_in(&y, &["rev-parse", "origin/dev"]), x_landing);

        // And the whole flow, run normally: Y merges X's landing and lands on
        // top of it, so dev keeps both changes.
        let failure = closeout_with(
            &y,
            Some("Y lands b"),
            &internal(),
            |root, _| {
                assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a\n");
                anyhow::bail!("injected integrated-tree gate failure")
            },
            |_, _| panic!("a failed gate must not refresh Dev"),
        );
        assert!(failure.is_err());
        assert_eq!(git_in(&y, &["rev-parse", "origin/dev"]), x_landing);
        closeout_fixture(&y, Some("Y lands b")).expect("Y lands");
        assert_eq!(git_in(&y, &["show", "origin/dev:a.txt"]), "a");
        assert_eq!(
            git_in(&y, &["diff", "--name-only", "HEAD^1", "HEAD"]),
            "b.txt"
        );
        assert_eq!(git_in(&y, &["rev-parse", "HEAD^1"]), x_landing);
        assert_eq!(
            git_in(&y, &["rev-parse", "origin/dev"]),
            git_in(&y, &["rev-parse", "HEAD"])
        );
        assert!(git_in(&y, &["status", "--porcelain"]).is_empty());
    }

    #[test]
    fn closeout_keeps_docs_updates_md_in_sync_across_sequential_and_concurrent_landings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let origin = temp.path().join("origin.git");
        git_in(
            temp.path(),
            &["init", "--bare", "--quiet", &origin.to_string_lossy()],
        );
        let root = work_repo(temp.path(), "work", &origin);
        fs::write(root.join("README.md"), "init\n").expect("readme");
        git_in(&root, &["add", "README.md"]);
        git_in(&root, &["commit", "--quiet", "-m", "initial"]);
        fs::create_dir_all(root.join("docs")).expect("docs");
        crate::update_log::write_update_log(&root).expect("initial update log");
        git_in(&root, &["add", "docs/UPDATES.md"]);
        git_in(&root, &["commit", "--quiet", "--amend", "-m", "initial"]);
        git_in(
            &root,
            &["push", "--quiet", "origin", "HEAD:main", "HEAD:dev"],
        );

        git_in(&root, &["switch", "--quiet", "-c", "feat/first"]);
        fs::write(root.join("first.txt"), "first\n").expect("first");
        git_in(&root, &["add", "first.txt"]);
        git_in(&root, &["commit", "--quiet", "-m", "feat: first"]);
        closeout_fixture(&root, Some("First update")).expect("first closeout");
        crate::update_log::run(&root, true).expect("update-log --check after first landing");
        let updates = fs::read_to_string(root.join("docs/UPDATES.md")).expect("read updates");
        assert!(updates.contains("### First update"), "{updates}");

        git_in(&root, &["switch", "--quiet", "-c", "fix/second"]);
        fs::write(root.join("second.txt"), "second\n").expect("second");
        git_in(&root, &["add", "second.txt"]);
        git_in(&root, &["commit", "--quiet", "-m", "fix: second"]);
        closeout_fixture(&root, Some("Second update")).expect("second closeout");
        crate::update_log::run(&root, true).expect("update-log --check after second landing");
        let updates = fs::read_to_string(root.join("docs/UPDATES.md")).expect("read updates");
        assert!(updates.contains("### Second update"), "{updates}");
        assert!(updates.contains("### First update"), "{updates}");
    }
}
