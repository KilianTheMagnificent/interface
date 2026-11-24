//! One pinned candidate, one writer, and durable progress through publication.
use crate::{artifact_receipt, gate, gate_cache, landings, pipeline_state, promote, release};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Debug)]
pub struct Request {
    pub version: String,
    pub changes: Vec<String>,
    pub revision: Option<String>,
    /// User-facing landings the owner agreed to leave on Dev.
    pub leave_out: Vec<String>,
    pub dry_run: bool,
    pub no_push: bool,
    pub worktree: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selected {
    pub sha: String,
    pub name: String,
}

/// How far a promotion has come. Each serializes as its lowercase name, the
/// spelling records have always used, which is also its `timings_ms` key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Selected,
    Prepared,
    Checked,
    Packaged,
    Published,
    Staged,
}

impl Phase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::Prepared => "prepared",
            Self::Checked => "checked",
            Self::Packaged => "packaged",
            Self::Published => "published",
            Self::Staged => "staged",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The promotion record schema this build writes. Schema 2 adds the release
/// contents; a schema-1 record still reads (with empty lists) and resumes.
const SCHEMA: u8 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub schema: u8,
    pub version: String,
    pub selectors: Vec<String>,
    pub approved: String,
    pub main_base: String,
    pub selected: Vec<Selected>,
    pub worktree: PathBuf,
    pub promoted: Option<String>,
    pub package: Option<artifact_receipt::Receipt>,
    pub phase: Phase,
    pub created_at: u64,
    pub updated_at: u64,
    pub timings_ms: BTreeMap<String, u64>,
    pub last_error: Option<String>,
    /// The `--leave-out` selectors, normalized like `selectors`.
    #[serde(default)]
    pub leave_out: Vec<String>,
    /// Every landing the release carries.
    #[serde(default)]
    pub included: Vec<landings::Entry>,
    /// Every unshipped landing on the pinned dev tip it does not carry.
    #[serde(default)]
    pub left_out: Vec<landings::Entry>,
    /// Earlier landings the included ones declare they replace.
    #[serde(default)]
    pub supersedes: Vec<landings::Replacement>,
}

pub fn lock_directory() -> Result<PathBuf> {
    let path = pipeline_state::home()?.join(".interface-pipeline");
    pipeline_state::private_directory(&path)?;
    Ok(path)
}

/// Runs `operation` holding the lock promote, package-mac and stage-update
/// share, so only one of them prepares or publishes a release at a time.
pub fn with_promotion_lock<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    release::with_pipeline_lock(&lock_directory()?, ".promotion.lock", operation)
}

pub fn run(root: &Path, request: &Request) -> Result<()> {
    release::compare_versions(&request.version, &request.version)?;
    ensure!(
        request.changes.is_empty() || request.revision.is_none(),
        "use --revision or --change, not both"
    );
    if request.dry_run {
        return execute(root, request);
    }
    with_promotion_lock(|| execute(root, request))
}

fn path(root: &Path, version: &str) -> Result<PathBuf> {
    let directory = pipeline_state::location(root)?.join("promotions");
    Ok(directory.join(format!("{version}.json")))
}

fn save(root: &Path, run: &mut Run) -> Result<()> {
    let directory = pipeline_state::directory(root)?.join("promotions");
    pipeline_state::private_directory(&directory)?;
    run.updated_at = pipeline_state::now();
    pipeline_state::write(&path(root, &run.version)?, run)?;
    pipeline_state::write(
        &pipeline_state::directory(root)?.join("promotion.json"),
        run,
    )
}

fn selectors(changes: &[String]) -> Vec<String> {
    let mut result: Vec<_> = changes.iter().map(|s| s.trim().to_owned()).collect();
    result.sort();
    result.dedup();
    result
}

fn execute(root: &Path, request: &Request) -> Result<()> {
    gate::git_capture(root, &["fetch", "origin"])?;
    let file = path(root, &request.version)?;
    let run = if let Some(saved) = pipeline_state::read::<Run>(&file)? {
        ensure!(
            (saved.schema == 1 || saved.schema == SCHEMA)
                && saved.version == request.version
                && saved.selectors == selectors(&request.changes)
                && saved.leave_out == selectors(&request.leave_out),
            "this version belongs to a different promotion; use its original selection \
             (--change and --leave-out)"
        );
        if let Some(revision) = &request.revision {
            ensure!(
                gate::rev_parse(root, &format!("{revision}^{{commit}}"))? == saved.approved,
                "this release is already pinned to a different approved revision"
            );
        }
        if let Some(worktree) = &request.worktree {
            ensure!(
                promote::canonical_candidate(worktree)
                    == promote::canonical_candidate(&saved.worktree),
                "resume uses the original release worktree {}",
                saved.worktree.display()
            );
        }
        println!("promote: resuming {} at {}", saved.version, saved.phase);
        (saved, Vec::new())
    } else {
        plan(root, request)?
    };
    let (mut run, unacknowledged) = run;
    print_plan(&run);
    if !unacknowledged.is_empty() {
        bail!(
            "promote refused: {}",
            landings::unacknowledged_message(&unacknowledged.iter().collect::<Vec<_>>())
        );
    }
    if request.dry_run {
        return Ok(());
    }
    save(root, &mut run)?;
    let outcome = finish(root, request, &mut run);
    if let Err(error) = &outcome {
        run.last_error = Some(format!("{error:#}"));
        save(root, &mut run)?;
        eprintln!(
            "promote: progress saved at {}; rerun the same command to continue. Inspect cargo xtask status.",
            run.phase
        );
    }
    outcome
}

/// Pins the release: the approved revision or selected landings, the split of
/// every unshipped landing into included and left out, and the user-facing
/// left-out landings no `--leave-out` acknowledges (returned, so the summary
/// prints before the refusal).
fn plan(root: &Path, request: &Request) -> Result<(Run, Vec<landings::Landing>)> {
    let dev = gate::rev_parse(root, "origin/dev")?;
    let main = gate::rev_parse(root, "origin/main")?;
    let worktree = request
        .worktree
        .clone()
        .map_or_else(promote::default_release_worktree, Ok)?;
    promote::check_release_worktree(root, &worktree)?;
    if worktree.exists() {
        ensure!(
            gate::tree_is_clean(&worktree)?,
            "release worktree has local changes; resolve them before preparing a release"
        );
        same_repository(root, &worktree)?;
    }
    let all = landings::unshipped_at(root, &main, &dev)?;
    let (approved, selected) = if request.changes.is_empty() {
        let revision = gate::rev_parse(
            root,
            &format!(
                "{}^{{commit}}",
                request.revision.as_deref().unwrap_or("HEAD")
            ),
        )?;
        ensure!(
            gate::is_ancestor(root, &revision, &dev)?,
            "approved revision is not on dev; land it first"
        );
        ensure!(
            gate::is_ancestor(root, &main, &revision)?,
            "approved revision does not include current main"
        );
        (revision, Vec::new())
    } else {
        let selected = promote::selected_in_dev_order(&all, &request.changes)?
            .iter()
            .map(|landing| Selected {
                sha: landing.sha.clone(),
                name: landing.name.clone(),
            })
            .collect();
        (dev.clone(), selected)
    };
    let (included, left_out) = split(root, &all, &approved, &selected)?;
    let unacknowledged = landings::unacknowledged(&all, &left_out, &request.leave_out)
        .map_err(|refusal| anyhow::anyhow!("promote refused: {refusal}"))?
        .into_iter()
        .cloned()
        .collect();
    let replaced = landings::replacements(&included, &landings::known_landings(root, &dev)?);
    let included: Vec<landings::Entry> = included.into_iter().map(Into::into).collect();
    if selected.is_empty() {
        // The whole-dev candidate's tree exists already: a dry run reports a
        // lost pin before anything is prepared.
        crate::pins::verify_release(root, &approved, &main, &included)?;
    }
    let base = if selected.is_empty() {
        &approved
    } else {
        &main
    };
    let current = promote::version_in_package_json(&gate::git_capture(
        root,
        &["show", &format!("{base}:electron/package.json")],
    )?)?;
    release::assert_version_progression(&request.version, Some(&current), false)?;
    release::assert_version_progression(
        &request.version,
        promote::feed_version()?.as_deref(),
        false,
    )?;
    let run = Run {
        schema: SCHEMA,
        version: request.version.clone(),
        selectors: selectors(&request.changes),
        approved,
        main_base: main,
        selected,
        worktree,
        promoted: None,
        package: None,
        phase: Phase::Selected,
        created_at: pipeline_state::now(),
        updated_at: pipeline_state::now(),
        timings_ms: BTreeMap::new(),
        last_error: None,
        leave_out: selectors(&request.leave_out),
        included,
        left_out: left_out.into_iter().map(Into::into).collect(),
        supersedes: replaced,
    };
    Ok((run, unacknowledged))
}

/// Every unshipped landing, split into the ones the release carries — the
/// selected ones, or for a whole-dev release those the approved revision
/// contains — and the ones it leaves on Dev.
fn split<'a>(
    root: &Path,
    all: &'a [landings::Landing],
    approved: &str,
    selected: &[Selected],
) -> Result<(Vec<&'a landings::Landing>, Vec<&'a landings::Landing>)> {
    let mut included = Vec::new();
    let mut left_out = Vec::new();
    for landing in all {
        let carried = if selected.is_empty() {
            gate::is_ancestor(root, &landing.sha, approved)?
        } else {
            selected.iter().any(|picked| picked.sha == landing.sha)
        };
        if carried {
            included.push(landing);
        } else {
            left_out.push(landing);
        }
    }
    Ok((included, left_out))
}

/// The owner-facing summary of what `run` ships, or `None` for a schema-1
/// record, which never recorded its contents.
fn summary(run: &Run) -> Option<String> {
    (run.schema >= 2).then(|| {
        landings::release_summary(&run.version, &run.included, &run.left_out, &run.supersedes)
    })
}

fn print_plan(run: &Run) {
    println!(
        "promote: Interface {} pinned to {} in {}",
        run.version,
        gate::short(&run.approved),
        run.worktree.display()
    );
    if let Some(summary) = summary(run) {
        print!("{summary}");
    } else {
        for landing in &run.selected {
            println!("  {} {}", gate::short(&landing.sha), landing.name);
        }
    }
    println!("  prepare → reuse/check → reuse/package and smoke → publish main/dev → verify feed");
    println!(
        "  the caller's checkout stays on its current branch; installed production stays running"
    );
}

fn completed(root: &Path, run: &mut Run, phase: Phase, started: Instant) -> Result<()> {
    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    *run.timings_ms.entry(phase.as_str().to_owned()).or_default() += elapsed;
    run.phase = phase;
    run.last_error = None;
    println!("promote: {phase} ({:.1?})", started.elapsed());
    save(root, run)
}

fn finish(root: &Path, request: &Request, run: &mut Run) -> Result<()> {
    let started = Instant::now();
    if run.promoted.is_none() {
        prepare(root, run)?;
    }
    let promoted = run.promoted.clone().context("candidate was not prepared")?;
    completed(root, run, Phase::Prepared, started)?;

    // `packaged` is timed from here, so it includes `checked` whenever the gate
    // runs below; `status` labels it that way rather than rewriting the record.
    let started = Instant::now();
    if let Some(receipt) = &run.package {
        artifact_receipt::validate(receipt, &promoted, &run.version)?;
        println!("promote: reused the verified signed package and smoke result");
    } else {
        checkout(root, &run.worktree, &promoted)?;
        let previous: Option<artifact_receipt::Receipt> =
            pipeline_state::read(&run.worktree.join("target/xtask/package-receipt.json"))?;
        let receipt = if let Some(previous) = previous
            .filter(|r| r.commit == promoted && r.version == run.version && r.smoke_runs > 0)
        {
            artifact_receipt::validate(&previous, &promoted, &run.version)?;
            println!("promote: recovered the package verification receipt");
            previous
        } else {
            crate::dependencies::ensure_installed(&run.worktree)?;
            let checked = Instant::now();
            let suites = gate_cache::ensure_full(&run.worktree)?;
            completed(root, run, Phase::Checked, checked)?;
            // An unpublished interrupted attempt may have filled release/.
            // The durable run pins its source; a staged equal version is never replaced.
            let replacement = promote::manifest_version(&run.worktree.join("release"))?.as_deref()
                == Some(&run.version);
            let gate = if suites.js {
                crate::package::GateTrust::ReceiptsAndBuild
            } else {
                crate::package::GateTrust::Receipts
            };
            crate::package::package_mac(&run.worktree, replacement, true, gate)?;
            pipeline_state::read(&run.worktree.join("target/xtask/package-receipt.json"))?
                .context("packaging left no verification receipt")?
        };
        let archive_root = lock_directory()?.join("artifacts");
        pipeline_state::private_directory(&archive_root)?;
        run.package = Some(artifact_receipt::archive(
            &receipt,
            &archive_root.join(&promoted),
        )?);
    }
    completed(root, run, Phase::Packaged, started)?;
    if request.no_push {
        println!(
            "promote: prepared and verified; no refs or feed published. Rerun without --no-push to publish these exact bytes."
        );
        return Ok(());
    }
    let started = Instant::now();
    publish(root, run)?;
    completed(root, run, Phase::Published, started)?;
    let started = Instant::now();
    let receipt = run.package.as_ref().context("missing verified artifact")?;
    let feed = release::default_feed_target()?;
    if !artifact_receipt::feed_matches(receipt, &feed)? {
        release::stage_update_feed(&receipt.directory, &feed, false)?;
    }
    ensure!(
        artifact_receipt::feed_matches(receipt, &feed)?,
        "feed readback did not match the verified package"
    );
    completed(root, run, Phase::Staged, started)?;
    release::nudge_running_production_app();
    println!(
        "promote: Interface {} on main as {}; feed verified at {}",
        run.version,
        gate::short(&promoted),
        feed.display()
    );
    if let Some(summary) = summary(run) {
        print!("{summary}");
    }
    // Left-out landings plus anything that landed while this release ran. The
    // release is already staged, so a failed read here is only a missing list.
    let still = landings::unshipped_landings(root).unwrap_or_default();
    if !still.is_empty() {
        println!("promote: still on Dev only, for a later release:");
        for landing in &still {
            println!("  {} {}", gate::short(&landing.sha), landing.name);
        }
    }
    println!("promote: installation remains an owner action");
    Ok(())
}

fn same_repository(root: &Path, worktree: &Path) -> Result<()> {
    let common = |path: &Path| -> Result<PathBuf> {
        let dir = gate::git_capture(path, &["rev-parse", "--git-common-dir"])?;
        Ok(path.join(dir.trim()).canonicalize()?)
    };
    ensure!(
        common(root)? == common(worktree)?,
        "release worktree belongs to a different repository"
    );
    Ok(())
}

fn checkout(root: &Path, worktree: &Path, sha: &str) -> Result<()> {
    promote::check_release_worktree(root, worktree)?;
    if !worktree.exists() {
        // Retain the iCloud-safe seed when the caller supplies clean source.
        if gate::tree_is_clean(root)? {
            let head = gate::rev_parse(root, "HEAD")?;
            promote::seed_worktree(root, worktree, &head)?;
        } else {
            promote::git_through_icloud(
                root,
                "worktree add",
                &[
                    "worktree",
                    "add",
                    "--detach",
                    &worktree.to_string_lossy(),
                    sha,
                ],
            )?;
        }
    }
    same_repository(root, worktree)?;
    ensure!(
        gate::tree_is_clean(worktree)?,
        "release worktree has local changes; they were preserved"
    );
    promote::git_through_icloud(
        worktree,
        "checkout",
        &["checkout", "--quiet", "--detach", sha],
    )
}

fn candidate_ref(run: &Run) -> String {
    format!("refs/interface/releases/{}", run.version)
}
fn run_marker(run: &Run) -> String {
    format!(
        "Promotion-run: {} {} {}",
        run.version, run.approved, run.created_at
    )
}

/// The bump commit body: the marker resume detection matches as a whole line,
/// then `Ships:` and `Leaves-out:` trailers in the same block.
fn bump_body(run: &Run) -> String {
    let trailers = landings::release_trailers(&run.included, &run.left_out);
    format!("{}\n{trailers}", run_marker(run))
        .trim_end()
        .to_owned()
}

fn prepare(root: &Path, run: &mut Run) -> Result<()> {
    let reference = candidate_ref(run);
    let marker = run_marker(run);
    // Recover a commit made immediately before the journal write was interrupted.
    let worktree = run.worktree.clone();
    for location in [
        Some((root, reference.as_str())),
        worktree.exists().then_some((worktree.as_path(), "HEAD")),
    ]
    .into_iter()
    .flatten()
    {
        let output = gate::git(location.0, &["log", "-1", "--format=%H%n%B", location.1])?;
        if output.status.success() {
            let text = String::from_utf8(output.stdout)?;
            if text.lines().any(|line| line == marker) {
                let sha = text
                    .lines()
                    .next()
                    .context("candidate has no SHA")?
                    .to_owned();
                gate::git_capture(root, &["update-ref", &reference, &sha])?;
                run.promoted = Some(sha);
                save(root, run)?;
                return Ok(());
            }
        }
    }
    let base = if run.selected.is_empty() {
        &run.approved
    } else {
        &run.main_base
    };
    checkout(root, &run.worktree, base)?;
    for landing in &run.selected {
        let pick = gate::git(
            &run.worktree,
            &["cherry-pick", "-x", "-m", "1", &landing.sha],
        )?;
        if !pick.status.success() {
            let conflicts =
                gate::git_capture(&run.worktree, &["diff", "--name-only", "--diff-filter=U"])
                    .unwrap_or_default();
            let conflicts: Vec<&str> = conflicts
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            if conflicts.as_slice() == [crate::update_log::UPDATES_DOC] {
                gate::git_capture(
                    &run.worktree,
                    &["checkout", "--ours", "--", crate::update_log::UPDATES_DOC],
                )?;
                gate::git_capture(
                    &run.worktree,
                    &["add", "--", crate::update_log::UPDATES_DOC],
                )?;
                gate::git_capture(&run.worktree, &["commit", "--no-edit"])?;
                continue;
            }
            gate::git_capture(&run.worktree, &["cherry-pick", "--abort"])?;
            let all = landings::unshipped_at(root, &run.main_base, &run.approved)?;
            let overlaps = landings::landing_overlaps(root, &all)?;
            let other = overlaps.get(&landing.sha).cloned().unwrap_or_default();
            let item = all
                .iter()
                .find(|item| item.sha == landing.sha)
                .context("selected landing disappeared")?;
            bail!("{}", landings::conflict_message(item, &other));
        }
    }
    crate::pins::verify_release(&run.worktree, "HEAD", &run.main_base, &run.included)?;
    promote::bump(&run.worktree, &run.version, &bump_body(run))?;
    let sha = gate::rev_parse(&run.worktree, "HEAD")?;
    gate::git_capture(root, &["update-ref", &reference, &sha])?;
    run.promoted = Some(sha);
    save(root, run)
}

/// Construct the merge without dirtying any checkout or leaving a conflict to abort.
pub fn integration_commit(root: &Path, dev: &str, promoted: &str, version: &str) -> Result<String> {
    if gate::is_ancestor(root, promoted, dev)? {
        return Ok(dev.to_owned());
    }
    let output = gate::git(
        root,
        &["merge-tree", "--write-tree", "--name-only", dev, promoted],
    )?;
    let stdout = String::from_utf8(output.stdout)?;
    if !output.status.success() {
        let conflicts: Vec<&str> = stdout
            .lines()
            .skip(1)
            .take_while(|line| !line.trim().is_empty())
            .map(str::trim)
            .collect();
        ensure!(
            conflicts.as_slice() == [crate::update_log::UPDATES_DOC],
            "release does not merge into dev cleanly; candidate and artifacts are retained"
        );
    }
    let mut tree = stdout
        .lines()
        .next()
        .context("merge-tree returned no tree")?
        .trim()
        .to_owned();
    let has_updates_doc = gate::git_command(
        root,
        &[
            "cat-file",
            "-e",
            &format!("{promoted}:{}", crate::update_log::UPDATES_DOC),
        ],
        gate::GitStderr::Null,
    )
    .status()
    .is_ok_and(|status| status.success());
    if has_updates_doc {
        let log = crate::update_log::collect(root, dev, promoted, None)?;
        let markdown = crate::update_log::render_markdown(&log);
        tree = crate::update_log::tree_with_updates_doc(root, &tree, &markdown)?;
    }
    let message = tempfile::NamedTempFile::new()?;
    fs::write(
        message.path(),
        format!(
            "Release {version} merged back to dev\n\n{} {version}\n",
            landings::RELEASE_TRAILER
        ),
    )?;
    gate::git_capture(
        root,
        &[
            "commit-tree",
            &tree,
            "-p",
            dev,
            "-p",
            promoted,
            "-F",
            &message.path().to_string_lossy(),
        ],
    )
    .map(|sha| sha.trim().to_owned())
}

fn publish(root: &Path, run: &Run) -> Result<()> {
    publish_with(root, run, |integrated| {
        checkout(root, &run.worktree, integrated)?;
        crate::dependencies::ensure_installed(&run.worktree)?;
        gate_cache::ensure_full(&run.worktree)?;
        ensure!(
            gate::rev_parse(&run.worktree, "HEAD")? == integrated
                && gate::tree_is_clean(&run.worktree)?,
            "dev integration changed while it was being verified"
        );
        Ok(())
    })
}

fn publish_with(root: &Path, run: &Run, verify: impl Fn(&str) -> Result<()>) -> Result<()> {
    let promoted = run.promoted.as_deref().context("missing candidate")?;
    for attempt in 1..=3 {
        gate::git_capture(root, &["fetch", "origin"])?;
        let main = gate::rev_parse(root, "origin/main")?;
        let dev = gate::rev_parse(root, "origin/dev")?;
        if gate::is_ancestor(root, promoted, &main)? && gate::is_ancestor(root, promoted, &dev)? {
            ensure!(
                main == promoted,
                "a newer release is already on main; this candidate has been superseded"
            );
            println!("promote: main and dev already contain this release");
            return Ok(());
        }
        ensure!(
            gate::is_ancestor(root, &main, promoted)?,
            "main moved outside this approved candidate; no new refs published"
        );
        let integrated = integration_commit(root, &dev, promoted, &run.version)?;
        verify(&integrated)?;
        let push = gate::git_capture(
            root,
            &[
                "push",
                "--atomic",
                "origin",
                &format!("{promoted}:main"),
                &format!("{integrated}:dev"),
            ],
        );
        // Read actual refs even after an ambiguous network failure. Repeating a
        // ref update is safe; changing or rebuilding the candidate is unnecessary.
        gate::git_capture(root, &["fetch", "origin"])?;
        let published_main = gate::rev_parse(root, "origin/main")?;
        if gate::is_ancestor(root, promoted, &published_main)?
            && gate::is_ancestor(root, promoted, "origin/dev")?
        {
            ensure!(
                published_main == promoted,
                "a newer release is already on main; this candidate has been superseded"
            );
            println!(
                "promote: verified main {}, dev includes {}",
                gate::short(promoted),
                gate::short(&integrated)
            );
            return Ok(());
        }
        if attempt == 3 {
            push?;
            bail!("refs moved during publication; rerun to reconcile the same candidate");
        }
        println!(
            "promote: refs moved; retrying integration and publication with the same verified artifact"
        );
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{git_in as git, init_repo};
    use std::cell::Cell;

    fn fixture() -> (tempfile::TempDir, Run) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_repo(root);
        git(root, &["init", "--bare", "-q", "remote.git"]);
        git(root, &["remote", "add", "origin", "remote.git"]);
        fs::write(root.join("base.txt"), "base").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-qm", "base"]);
        let base = git(root, &["rev-parse", "HEAD"]);
        git(root, &["push", "-q", "origin", "HEAD:main", "HEAD:dev"]);
        fs::write(root.join("release.txt"), "approved").unwrap();
        git(root, &["add", "release.txt"]);
        git(root, &["commit", "-qm", "candidate"]);
        let candidate = git(root, &["rev-parse", "HEAD"]);
        let run = Run {
            schema: 1,
            version: "1.1.0".into(),
            selectors: vec![],
            approved: base.clone(),
            main_base: base,
            selected: vec![],
            worktree: root.join("unused"),
            promoted: Some(candidate),
            package: None,
            phase: Phase::Packaged,
            created_at: 1,
            updated_at: 1,
            timings_ms: BTreeMap::new(),
            last_error: None,
            leave_out: vec![],
            included: vec![],
            left_out: vec![],
            supersedes: vec![],
        };
        (temp, run)
    }

    #[test]
    fn a_concurrent_dev_landing_retries_without_changing_the_release_candidate() {
        let (temp, run) = fixture();
        let root = temp.path();
        let original = git(root, &["rev-parse", "HEAD"]);
        let branch = git(root, &["symbolic-ref", "HEAD"]);
        let validations = Cell::new(0);
        publish_with(root, &run, |_| {
            validations.set(validations.get() + 1);
            if validations.get() == 1 {
                // Another writer advances dev after verification and before push.
                let tree = git(root, &["rev-parse", &format!("{}^{{tree}}", run.main_base)]);
                let peer = git(
                    root,
                    &[
                        "commit-tree",
                        &tree,
                        "-p",
                        &run.main_base,
                        "-m",
                        "concurrent landing",
                    ],
                );
                git(root, &["push", "-q", "origin", &format!("{peer}:dev")]);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(validations.get(), 2);
        assert_eq!(git(root, &["rev-parse", "origin/main"]), original);
        assert!(gate::is_ancestor(root, &original, "origin/dev").unwrap());
        assert_eq!(git(root, &["rev-parse", "HEAD"]), original);
        assert_eq!(git(root, &["symbolic-ref", "HEAD"]), branch);
        // A retry after refs were published requires neither a gate nor a new commit.
        publish_with(root, &run, |_| {
            panic!("published refs must be reconciled without rebuilding")
        })
        .unwrap();
    }

    #[test]
    fn failed_integration_validation_publishes_neither_ref() {
        let (temp, run) = fixture();
        let root = temp.path();
        assert!(publish_with(root, &run, |_| bail!("injected gate failure")).is_err());
        git(root, &["fetch", "-q", "origin"]);
        assert_eq!(git(root, &["rev-parse", "origin/main"]), run.main_base);
        assert_eq!(git(root, &["rev-parse", "origin/dev"]), run.main_base);
    }

    #[test]
    fn a_newer_release_during_publication_prevents_staging_the_old_candidate() {
        let (temp, run) = fixture();
        let root = temp.path();
        let candidate = run.promoted.as_deref().unwrap();
        let tree = git(root, &["rev-parse", &format!("{candidate}^{{tree}}")]);
        let successor = git(
            root,
            &["commit-tree", &tree, "-p", candidate, "-m", "newer release"],
        );
        let error = publish_with(root, &run, |_| {
            git(
                root,
                &[
                    "push",
                    "-q",
                    "--atomic",
                    "origin",
                    &format!("{successor}:main"),
                    &format!("{successor}:dev"),
                ],
            );
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("superseded"));
        assert_eq!(git(root, &["rev-parse", "origin/main"]), successor);
        assert_eq!(git(root, &["rev-parse", "HEAD"]), candidate);
    }
    #[test]
    fn retry_selection_is_order_independent_and_never_broadens() {
        assert_eq!(
            selectors(&[" B ".into(), "A".into(), "A".into()]),
            vec!["A", "B"]
        );
        assert_ne!(selectors(&["A".into()]), selectors(&[]));
    }

    /// Records written while the phase was a string still read, and new ones
    /// are spelled the same way.
    #[test]
    fn phases_keep_the_spelling_records_have_always_used() {
        for (phase, spelling) in [
            (Phase::Selected, "selected"),
            (Phase::Prepared, "prepared"),
            (Phase::Checked, "checked"),
            (Phase::Packaged, "packaged"),
            (Phase::Published, "published"),
            (Phase::Staged, "staged"),
        ] {
            assert_eq!(phase.as_str(), spelling);
            assert_eq!(phase.to_string(), spelling);
            assert_eq!(serde_json::to_value(phase).unwrap(), spelling);
            assert_eq!(
                serde_json::from_value::<Phase>(spelling.into()).unwrap(),
                phase
            );
        }
        assert!(serde_json::from_value::<Phase>("Staged".into()).is_err());
    }

    #[test]
    fn a_schema_one_record_still_reads_and_prints_no_summary() {
        let (_temp, run) = fixture();
        let mut value = serde_json::to_value(&run).unwrap();
        let object = value.as_object_mut().unwrap();
        for field in ["leave_out", "included", "left_out", "supersedes"] {
            object.remove(field);
        }
        let old: Run = serde_json::from_value(value).expect("schema 1 reads");
        assert!(old.included.is_empty() && old.leave_out.is_empty());
        assert_eq!(summary(&old), None);
    }

    #[test]
    fn the_bump_body_keeps_the_marker_line_and_lists_the_release() {
        let (_temp, mut run) = fixture();
        run.schema = SCHEMA;
        let entry = |sha: &str, name: &str, kind| landings::Entry {
            sha: sha.repeat(40),
            name: name.to_owned(),
            kind,
            check: None,
            pins: vec![],
        };
        run.included = vec![entry("a", "Swap preview", Some(landings::Kind::Feature))];
        run.left_out = vec![
            entry("b", "Slide", Some(landings::Kind::Look)),
            entry("c", "Notes", Some(landings::Kind::Record)),
        ];
        let body = bump_body(&run);
        assert!(body.lines().any(|line| line == run_marker(&run)), "{body}");
        assert!(
            body.ends_with("\nShips: aaaaaaa Swap preview\nLeaves-out: bbbbbbb Slide"),
            "{body}"
        );
        let summary = summary(&run).unwrap();
        assert!(
            summary.starts_with("Interface 1.1.0 — ships 1 landing\nFeatures\n"),
            "{summary}"
        );
        run.included.clear();
        run.left_out.clear();
        assert_eq!(bump_body(&run), run_marker(&run));
    }
}
