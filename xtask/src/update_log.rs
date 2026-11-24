//! Full update log (`docs/UPDATES.md`) of production releases and Dev landings.
//!
//! Maintained automatically by `cargo xtask closeout`, `cargo xtask promote`,
//! and `cargo xtask update-log`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail};
use interface_common::time::{IsoFraction, civil_from_days, unix_seconds_iso};

use crate::{
    gate,
    landings::{Kind, LandingMeta},
};

/// Repository-relative path of the update log.
pub const UPDATES_DOC: &str = "docs/UPDATES.md";

const BUMP_PREFIX: &str = "chore(release): bump Interface to ";

const DEV_LOG_FORMAT: &str = "%H%x1f%P%x1f%cI%x1f%ct%x1f%s%x1f%b\
%x1f%(trailers:key=Landed-from,valueonly)%x1f%(trailers:key=Landing-kind,valueonly)\
%x1f%(trailers:key=Landing-check,valueonly)%x1f%(trailers:key=Landing-pin,valueonly)\
%x1f%(trailers:key=Supersedes,valueonly)%x1e";

const MAIN_LOG_FORMAT: &str = "%H%x1f%cI%x1f%ct%x1f%P%x1f%s%x1f%b\
%x1f%(trailers:key=Landed-from,valueonly)%x1f%(trailers:key=Ships,valueonly)\
%x1f%(trailers:key=Leaves-out,valueonly)%x1e";

/// A landing about to be written by `closeout` before its merge commit exists.
#[derive(Clone, Debug)]
pub struct PendingLanding<'a> {
    pub sha: &'a str,
    pub completed_at: &'a str,
    pub committed: i64,
    pub name: &'a str,
    pub branch: &'a str,
    pub subjects: &'a [String],
    pub meta: &'a LandingMeta,
}

/// One Dev landing in the update log.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggedLanding {
    pub sha: String,
    pub merge_sha: String,
    pub completed_at: String,
    pub committed: i64,
    pub name: String,
    pub branch: String,
    pub kind: String,
    pub notes: String,
    pub commits: Vec<String>,
    pub pins: Vec<String>,
    pub supersedes: Vec<String>,
    pub release: Option<String>,
}

/// One item shipped inside a production release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseItem {
    pub sha: String,
    pub completed_at: String,
    pub kind: String,
    pub name: String,
    pub branch: Option<String>,
    pub notes: Option<String>,
    pub pins: Vec<String>,
}

/// One production release (`v0.3.x`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggedRelease {
    pub version: String,
    pub sha: String,
    pub completed_at: String,
    pub committed: i64,
    pub items: Vec<ReleaseItem>,
    pub left_on_dev: Vec<String>,
}

/// Complete parsed/collected update history.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateLog {
    pub releases: Vec<LoggedRelease>,
    pub landings: Vec<LoggedLanding>,
}

fn lines_of(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_trailer_line(line: &str) -> bool {
    [
        "Landed-from:",
        "Landing-kind:",
        "Landing-check:",
        "Landing-pin:",
        "Supersedes:",
        "Settled-look:",
        "Promotion-run:",
        "Ships:",
        "Leaves-out:",
        "Release:",
        "(cherry picked from commit ",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

fn body_subjects(body: &str, title: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_trailer_line(line) && *line != title)
        .map(str::to_owned)
        .collect()
}

/// Infers a kind label when an older commit predates `Landing-kind:` trailers.
#[must_use]
pub fn infer_kind(branch: &str, name: &str, commits: &[String]) -> String {
    let branch_lower = branch.trim().to_ascii_lowercase();
    for (prefix, kind) in [
        ("feat/", "feature"),
        ("feature/", "feature"),
        ("fix/", "fix"),
        ("bugfix/", "fix"),
        ("hotfix/", "fix"),
        ("speed/", "speed"),
        ("perf/", "speed"),
        ("look/", "look"),
        ("style/", "look"),
        ("ui/", "look"),
        ("docs/", "record"),
        ("record/", "record"),
        ("cleanup/", "internal"),
        ("refactor/", "internal"),
        ("chore/", "internal"),
        ("land/", "internal"),
        ("test/", "internal"),
    ] {
        if branch_lower.starts_with(prefix) {
            return kind.to_owned();
        }
    }
    let sample = commits
        .first()
        .map_or(name, String::as_str)
        .trim()
        .to_ascii_lowercase();
    for (prefix, kind) in [
        ("feat", "feature"),
        ("fix", "fix"),
        ("perf", "speed"),
        ("speed", "speed"),
        ("style", "look"),
        ("docs", "record"),
        ("todo:", "record"),
        ("record ", "record"),
        ("refactor", "internal"),
        ("chore", "internal"),
        ("test", "internal"),
    ] {
        if sample.starts_with(prefix) || name.to_ascii_lowercase().starts_with(prefix) {
            return kind.to_owned();
        }
    }
    "feature".to_owned()
}

/// Parses `git log --first-parent --merges --grep=Landed-from:` output into
/// newest-first [`LoggedLanding`] records.
#[must_use]
pub fn parse_dev_landings(log: &str) -> Vec<LoggedLanding> {
    log.split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_matches('\n').splitn(11, '\x1f');
            let merge_sha = fields.next()?.trim();
            let parents = fields.next()?.trim();
            let sha = parents.split_whitespace().nth(1).unwrap_or(merge_sha);
            let completed_at = fields.next()?.trim();
            let committed = fields.next()?.trim().parse().unwrap_or(0);
            let name = fields.next()?.trim();
            let body = fields.next().unwrap_or_default();
            let branch = lines_of(fields.next()).into_iter().next()?;
            if sha.is_empty() || branch.is_empty() {
                return None;
            }
            let explicit_kind = lines_of(fields.next())
                .first()
                .and_then(|value| Kind::parse(value))
                .map(|kind| kind.as_str().to_owned());
            let check = lines_of(fields.next()).into_iter().next();
            let pins = lines_of(fields.next());
            let supersedes = lines_of(fields.next());
            let commits = body_subjects(body, name);
            let kind = explicit_kind.unwrap_or_else(|| infer_kind(&branch, name, &commits));
            let notes = check.unwrap_or_else(|| {
                if commits.is_empty() {
                    name.to_owned()
                } else {
                    commits.join("; ")
                }
            });
            Some(LoggedLanding {
                sha: sha.to_owned(),
                merge_sha: merge_sha.to_owned(),
                completed_at: completed_at.to_owned(),
                committed,
                name: name.to_owned(),
                branch,
                kind,
                notes,
                commits,
                pins,
                supersedes,
                release: None,
            })
        })
        .collect()
}

fn release_item_from_landing(landing: &LoggedLanding) -> ReleaseItem {
    let notes =
        (landing.notes != landing.name && !landing.notes.is_empty()).then(|| landing.notes.clone());
    ReleaseItem {
        sha: gate::short(&landing.sha).to_owned(),
        completed_at: landing.completed_at.clone(),
        kind: landing.kind.clone(),
        name: landing.name.clone(),
        branch: Some(landing.branch.clone()),
        notes,
        pins: landing.pins.clone(),
    }
}

fn find_landing_index(landings: &[LoggedLanding], sha_prefix: &str, title: &str) -> Option<usize> {
    let prefix = sha_prefix.trim().to_ascii_lowercase();
    if prefix.len() >= 7 && prefix.chars().all(|ch| ch.is_ascii_hexdigit()) {
        if let Some(index) = landings.iter().position(|landing| {
            landing.sha.to_ascii_lowercase().starts_with(&prefix)
                || landing.merge_sha.to_ascii_lowercase().starts_with(&prefix)
        }) {
            return Some(index);
        }
    }
    let clean = title.trim();
    (!clean.is_empty()).then(|| {
        landings
            .iter()
            .position(|landing| landing.name.eq_ignore_ascii_case(clean))
    })?
}

fn cherry_picked_sha(body: &str) -> Option<&str> {
    body.lines().find_map(|line| {
        let sha = line
            .trim()
            .strip_prefix("(cherry picked from commit ")?
            .strip_suffix(')')?
            .trim();
        (sha.len() == 40 && sha.chars().all(|ch| ch.is_ascii_hexdigit())).then_some(sha)
    })
}

/// Parses `git log --reverse <main>` and links releases with `landings` (mutating
/// `landing.release` to the earliest release that shipped each landing).
pub fn parse_releases_and_link(
    main_log: &str,
    landings: &mut [LoggedLanding],
) -> Vec<LoggedRelease> {
    let mut releases = Vec::new();
    let mut pending_landings: Vec<usize> = Vec::new();
    let mut pending_commits: Vec<ReleaseItem> = Vec::new();
    let mut commit_times: BTreeMap<String, String> = BTreeMap::new();

    for record in main_log.split('\x1e') {
        let mut fields = record.trim_matches('\n').splitn(9, '\x1f');
        let Some(sha) = fields.next().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let completed_at = fields.next().unwrap_or_default().trim().to_owned();
        let committed: i64 = fields
            .next()
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0);
        let parents = fields.next().unwrap_or_default().trim();
        let subject = fields.next().unwrap_or_default().trim();
        let body = fields.next().unwrap_or_default();
        let landed_from = lines_of(fields.next());
        let ships = lines_of(fields.next());
        let leaves_out = lines_of(fields.next());
        commit_times.insert(gate::short(sha).to_owned(), completed_at.clone());

        if let Some(version) = subject.strip_prefix(BUMP_PREFIX).map(str::trim) {
            let release_sha = parents.split_whitespace().next().unwrap_or(sha).to_owned();
            let items = build_release_items(
                version,
                &completed_at,
                &ships,
                &mut pending_landings,
                &mut pending_commits,
                landings,
                &commit_times,
            );
            releases.push(LoggedRelease {
                version: version.to_owned(),
                sha: release_sha,
                completed_at,
                committed,
                items,
                left_on_dev: leaves_out,
            });
            continue;
        }

        if let Some(source_sha) = cherry_picked_sha(body) {
            if let Some(idx) = find_landing_index(landings, source_sha, subject) {
                if !pending_landings.contains(&idx) {
                    pending_landings.push(idx);
                }
                continue;
            }
        }
        if !landed_from.is_empty() {
            if let Some(idx) = find_landing_index(landings, sha, subject) {
                if !pending_landings.contains(&idx) {
                    pending_landings.push(idx);
                }
                continue;
            }
        }
        let is_merge = parents.split_whitespace().count() > 1;
        if !is_merge
            && !subject.starts_with("Release ")
            && !subject.starts_with("Reconcile ")
            && !subject.is_empty()
        {
            pending_commits.push(ReleaseItem {
                sha: gate::short(sha).to_owned(),
                completed_at,
                kind: infer_kind("", subject, &[]),
                name: subject.to_owned(),
                branch: None,
                notes: None,
                pins: Vec::new(),
            });
        }
    }
    releases.reverse();
    releases
}

fn build_release_items(
    version: &str,
    release_completed_at: &str,
    ships: &[String],
    pending_landings: &mut Vec<usize>,
    pending_commits: &mut Vec<ReleaseItem>,
    landings: &mut [LoggedLanding],
    commit_times: &BTreeMap<String, String>,
) -> Vec<ReleaseItem> {
    let mut items = Vec::new();
    let mut seen_shas = BTreeSet::new();

    if !ships.is_empty() {
        for entry in ships.iter().rev() {
            let (short_sha, title) = entry
                .split_once(' ')
                .map_or((entry.as_str(), ""), |(s, t)| (s.trim(), t.trim()));
            if let Some(idx) = find_landing_index(landings, short_sha, title) {
                if landings[idx].release.is_none() {
                    landings[idx].release = Some(version.to_owned());
                }
                let item = release_item_from_landing(&landings[idx]);
                if seen_shas.insert(item.sha.clone()) {
                    items.push(item);
                }
            } else if !short_sha.is_empty() && seen_shas.insert(short_sha.to_owned()) {
                let completed_at = commit_times
                    .get(short_sha)
                    .cloned()
                    .unwrap_or_else(|| release_completed_at.to_owned());
                items.push(ReleaseItem {
                    sha: short_sha.to_owned(),
                    completed_at,
                    kind: infer_kind("", title, &[]),
                    name: title.to_owned(),
                    branch: None,
                    notes: None,
                    pins: Vec::new(),
                });
            }
        }
        for &idx in pending_landings.iter() {
            if landings[idx].release.is_none() {
                landings[idx].release = Some(version.to_owned());
            }
        }
    } else if !pending_landings.is_empty() {
        for &idx in pending_landings.iter().rev() {
            if landings[idx].release.is_none() {
                landings[idx].release = Some(version.to_owned());
            }
            let item = release_item_from_landing(&landings[idx]);
            if seen_shas.insert(item.sha.clone()) {
                items.push(item);
            }
        }
    } else {
        for item in pending_commits.iter().rev() {
            if seen_shas.insert(item.sha.clone()) {
                items.push(item.clone());
            }
        }
    }

    pending_landings.clear();
    pending_commits.clear();
    items
}

/// Collects the complete [`UpdateLog`] from git history at `root`.
pub fn collect(
    root: &Path,
    dev_rev: &str,
    main_rev: &str,
    pending: Option<&PendingLanding<'_>>,
) -> Result<UpdateLog> {
    let dev_log = gate::git_capture(
        root,
        &[
            "log",
            "--first-parent",
            "--merges",
            "--fixed-strings",
            "--grep=Landed-from:",
            &format!("--format={DEV_LOG_FORMAT}"),
            dev_rev,
        ],
    )?;
    let mut landings = parse_dev_landings(&dev_log);
    if let Some(pending) = pending {
        let already_present = landings.first().is_some_and(|first| {
            first.sha == pending.sha
                || (first.name == pending.name && first.branch == pending.branch)
        });
        if !already_present {
            let commits: Vec<String> = pending
                .subjects
                .iter()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty() && s != pending.name)
                .collect();
            let notes = pending.meta.check.clone().unwrap_or_else(|| {
                if commits.is_empty() {
                    pending.name.to_owned()
                } else {
                    commits.join("; ")
                }
            });
            landings.insert(
                0,
                LoggedLanding {
                    sha: pending.sha.to_owned(),
                    merge_sha: pending.sha.to_owned(),
                    completed_at: pending.completed_at.to_owned(),
                    committed: pending.committed,
                    name: pending.name.to_owned(),
                    branch: pending.branch.to_owned(),
                    kind: pending.meta.kind.as_str().to_owned(),
                    notes,
                    commits,
                    pins: pending.meta.pins.clone(),
                    supersedes: pending.meta.supersedes.clone(),
                    release: None,
                },
            );
        }
    }

    let main_target = if gate::rev_parse(root, main_rev).is_ok() {
        main_rev
    } else {
        dev_rev
    };
    let main_log = gate::git_capture(
        root,
        &[
            "log",
            "--reverse",
            &format!("--format={MAIN_LOG_FORMAT}"),
            main_target,
        ],
    )?;
    let releases = parse_releases_and_link(&main_log, &mut landings);
    Ok(UpdateLog { releases, landings })
}

/// Renders the complete [`UpdateLog`] as deterministic Markdown for `docs/UPDATES.md`.
#[must_use]
pub fn render_markdown(log: &UpdateLog) -> String {
    let mut out = String::new();
    out.push_str("# Interface Update Log\n\n");
    out.push_str(
        "Complete history of production releases (`v0.3.x`) and Dev landings, including notes \
         on what each update is, how it is verified, and when it was completed. Maintained \
         automatically by `cargo xtask closeout`, `cargo xtask promote`, and \
         `cargo xtask update-log`.\n\n",
    );

    let unreleased: Vec<&LoggedLanding> = log
        .landings
        .iter()
        .filter(|landing| landing.release.is_none())
        .collect();
    let release_range = match (log.releases.last(), log.releases.first()) {
        (Some(oldest), Some(newest)) if oldest.version != newest.version => {
            format!(" (`v{}` – `v{}`)", oldest.version, newest.version)
        }
        (Some(only), _) => format!(" (`v{}`)", only.version),
        _ => String::new(),
    };
    let _ = writeln!(
        out,
        "- **Production releases:** {}{release_range}",
        log.releases.len()
    );
    let _ = writeln!(
        out,
        "- **Dev landings:** {} (`{}` unreleased on Dev)\n",
        log.landings.len(),
        unreleased.len()
    );

    out.push_str("## Unreleased on Dev\n\n");
    if unreleased.is_empty() {
        out.push_str("_None — all Dev landings have shipped in a production release._\n\n");
    } else {
        for landing in &unreleased {
            render_landing_entry(&mut out, landing);
        }
    }

    out.push_str("## Production Releases\n\n");
    if log.releases.is_empty() {
        out.push_str("_No production releases recorded._\n\n");
    } else {
        for release in &log.releases {
            render_release_entry(&mut out, release);
        }
    }

    out.push_str("## All Dev Landings\n\n");
    if log.landings.is_empty() {
        out.push_str("_No Dev landings recorded._\n");
    } else {
        for landing in &log.landings {
            render_landing_entry(&mut out, landing);
        }
    }

    out
}

fn render_release_entry(out: &mut String, release: &LoggedRelease) {
    let _ = writeln!(
        out,
        "### v{} — {} (`{}`)\n",
        release.version,
        release.completed_at,
        gate::short(&release.sha)
    );
    if release.items.is_empty() {
        out.push_str("- _Version bump only._\n");
    } else {
        for item in &release.items {
            let branch_suffix = item
                .branch
                .as_deref()
                .map_or_else(String::new, |branch| format!(" (`{branch}`)"));
            let _ = writeln!(
                out,
                "- **[{}]** `{}` · {} · **{}**{}",
                item.kind, item.sha, item.completed_at, item.name, branch_suffix
            );
            if let Some(notes) = &item.notes {
                let _ = writeln!(out, "  - Notes: {notes}");
            }
            if !item.pins.is_empty() {
                let pins = item
                    .pins
                    .iter()
                    .map(|pin| format!("`{pin}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "  - Pins: {pins}");
            }
        }
    }
    if !release.left_on_dev.is_empty() {
        let _ = writeln!(out, "- **Left on Dev:** {}", release.left_on_dev.join("; "));
    }
    out.push('\n');
}

fn render_landing_entry(out: &mut String, landing: &LoggedLanding) {
    let _ = writeln!(out, "### {}\n", landing.name);
    let _ = writeln!(out, "- **Completed:** {}", landing.completed_at);
    let _ = writeln!(out, "- **Kind:** {}", landing.kind);
    match &landing.release {
        Some(version) => {
            let _ = writeln!(out, "- **Release:** v{version}");
        }
        None => {
            out.push_str("- **Release:** Unreleased (on Dev)\n");
        }
    }
    let _ = writeln!(
        out,
        "- **Commit:** `{}` (`{}`)",
        gate::short(&landing.sha),
        landing.branch
    );
    let _ = writeln!(out, "- **Notes:** {}", landing.notes);
    if !landing.commits.is_empty() && landing.commits.join("; ") != landing.notes {
        let _ = writeln!(out, "- **Commits:** {}", landing.commits.join("; "));
    }
    if !landing.pins.is_empty() {
        let pins = landing
            .pins
            .iter()
            .map(|pin| format!("`{pin}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "- **Pins:** {pins}");
    }
    if !landing.supersedes.is_empty() {
        let _ = writeln!(out, "- **Supersedes:** {}", landing.supersedes.join("; "));
    }
    out.push('\n');
}

/// Formats git's `GIT_COMMITTER_IDENT` timestamp (`<unix_secs> <±HHMM>`) as an
/// ISO-8601 timestamp matching `git log --format=%cI`.
#[must_use]
pub fn format_git_ident_timestamp(ident: &str) -> Option<(i64, String)> {
    let mut parts = ident.split_whitespace().rev();
    let tz = parts.next()?;
    let seconds: i64 = parts.next()?.parse().ok()?;
    if tz.len() == 5 && (tz.starts_with('+') || tz.starts_with('-')) {
        let sign: i64 = if tz.starts_with('-') { -1 } else { 1 };
        let tz_hours: i64 = tz[1..3].parse().ok()?;
        let tz_mins: i64 = tz[3..5].parse().ok()?;
        let offset = sign * (tz_hours * 3_600 + tz_mins * 60);
        let local = seconds + offset;
        let days = local.div_euclid(86_400);
        let time = local.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let hour = time / 3_600;
        let minute = (time % 3_600) / 60;
        let second = time % 60;
        let sign_char = &tz[0..1];
        return Some((
            seconds,
            format!(
                "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}\
                 {sign_char}{tz_hours:02}:{tz_mins:02}"
            ),
        ));
    }
    Some((seconds, unix_seconds_iso(seconds, IsoFraction::None)))
}

/// Current committer timestamp `(unix_seconds, iso_8601)` from git.
pub fn current_committer_time(root: &Path) -> (i64, String) {
    gate::git_capture(root, &["var", "GIT_COMMITTER_IDENT"])
        .ok()
        .and_then(|ident| format_git_ident_timestamp(ident.trim()))
        .unwrap_or_else(|| {
            let secs = i64::try_from(crate::pipeline_state::now()).unwrap_or(0);
            (secs, unix_seconds_iso(secs, IsoFraction::None))
        })
}

/// Builds a new tree SHA identical to `base_tree` except `docs/UPDATES.md`
/// holds the rendered update log including `pending`, without touching the
/// working tree or `.git/index`.
pub fn tree_with_pending_landing(
    root: &Path,
    dev_rev: &str,
    base_tree: &str,
    pending: &PendingLanding<'_>,
) -> Result<String> {
    let main_rev = if gate::rev_parse(root, "origin/main").is_ok() {
        "origin/main"
    } else {
        "main"
    };
    let log = collect(root, dev_rev, main_rev, Some(pending))?;
    let markdown = render_markdown(&log);
    tree_with_updates_doc(root, base_tree, &markdown)
}

/// Builds a new tree SHA identical to `base_tree` with `docs/UPDATES.md`
/// replaced by `markdown`.
pub fn tree_with_updates_doc(root: &Path, base_tree: &str, markdown: &str) -> Result<String> {
    let blob_file = tempfile::Builder::new()
        .prefix("interface-updates-md-")
        .tempfile()
        .context("could not create temporary UPDATES.md")?;
    fs::write(blob_file.path(), markdown)?;
    let blob = gate::git_capture(
        root,
        &["hash-object", "-w", &blob_file.path().to_string_lossy()],
    )?;
    let blob = blob.trim();
    let index_dir = tempfile::Builder::new()
        .prefix("interface-updates-idx-")
        .tempdir()
        .context("could not create temporary git index dir")?;
    let index_path = index_dir.path().join("index");
    let run_with_index = |args: &[&str]| -> Result<String> {
        let output = gate::git_command(root, args, gate::GitStderr::Capture)
            .env("GIT_INDEX_FILE", &index_path)
            .output()
            .context("failed to run git with temporary index")?;
        if !output.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    run_with_index(&["read-tree", base_tree])?;
    run_with_index(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100644,{blob},{UPDATES_DOC}"),
    ])?;
    run_with_index(&["write-tree"])
}

/// Syncs `docs/UPDATES.md` in the working tree and index from `HEAD` when
/// `HEAD` carries `docs/UPDATES.md`.
pub fn sync_from_head(root: &Path) -> Result<()> {
    let present = gate::git_command(
        root,
        &["cat-file", "-e", &format!("HEAD:{UPDATES_DOC}")],
        gate::GitStderr::Null,
    )
    .status()
    .is_ok_and(|status| status.success());
    if present {
        gate::git_capture(root, &["checkout", "--quiet", "HEAD", "--", UPDATES_DOC])?;
    }
    Ok(())
}

fn default_revs(root: &Path) -> (&'static str, &'static str) {
    let head_ok = gate::rev_parse(root, "HEAD").is_ok();
    let head_is_bump = head_ok
        && gate::git_capture(root, &["log", "-1", "--format=%s", "HEAD"])
            .is_ok_and(|s| s.trim().starts_with(BUMP_PREFIX));
    let dev_rev = if head_is_bump && gate::rev_parse(root, "origin/dev").is_ok() {
        "origin/dev"
    } else if head_ok {
        "HEAD"
    } else {
        "origin/dev"
    };
    let main_rev = if head_is_bump {
        "HEAD"
    } else if gate::rev_parse(root, "origin/main").is_ok() {
        "origin/main"
    } else if gate::rev_parse(root, "main").is_ok() {
        "main"
    } else {
        dev_rev
    };
    (dev_rev, main_rev)
}

/// Regenerates `docs/UPDATES.md` on disk from `HEAD` and `origin/main`.
pub fn write_update_log(root: &Path) -> Result<UpdateLog> {
    let (dev_rev, main_rev) = default_revs(root);
    let log = collect(root, dev_rev, main_rev, None)?;
    let markdown = render_markdown(&log);
    let path = root.join(UPDATES_DOC);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, markdown).with_context(|| format!("could not write {}", path.display()))?;
    Ok(log)
}

/// Entrypoint for `cargo xtask update-log`.
pub fn run(root: &Path, check: bool) -> Result<()> {
    let (dev_rev, main_rev) = default_revs(root);
    let log = collect(root, dev_rev, main_rev, None)?;
    let expected = render_markdown(&log);
    let path = root.join(UPDATES_DOC);
    if check {
        let actual =
            fs::read_to_string(&path).with_context(|| format!("{} is missing", path.display()))?;
        if actual != expected {
            bail!("{UPDATES_DOC} is out of date; run `cargo xtask update-log` to refresh it");
        }
        println!(
            "update-log: {UPDATES_DOC} is up to date ({} releases, {} landings)",
            log.releases.len(),
            log.landings.len()
        );
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, expected).with_context(|| format!("could not write {}", path.display()))?;
    println!(
        "update-log: wrote {UPDATES_DOC} ({} releases, {} landings)",
        log.releases.len(),
        log.landings.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_git_ident_timestamp_matches_git_iso_format() {
        let (secs, iso) =
            format_git_ident_timestamp("Andre Romero <a@b.com> 1767225600 -0500").unwrap();
        assert_eq!(secs, 1_767_225_600);
        assert_eq!(iso, "2025-12-31T19:00:00-05:00");
    }

    #[test]
    fn parse_and_render_update_log_links_releases_and_landings() {
        let dev_log = concat!(
            "a83ebb34b794c8f283ede28c5a39359028e431fb\x1f8f113822d1b709309f7a677fe5ac9dcf0e5466df 1c2a9b5300000000000000000000000000000000\x1f2026-10-21T14:48:47-05:00\x1f1792612127\x1f",
            "Harden terminal pass-off\x1fHarden terminal pass-off\n\nLanding-kind: fix\n",
            "Landing-check: Passed-off panes launch for every profile\n",
            "Landing-pin: apps/interface-host/src/pane_mcp.rs::pass_off\n",
            "Landed-from: fix/pass-off-review\x1f",
            "fix/pass-off-review\x1ffix\x1fPassed-off panes launch for every profile\x1f",
            "apps/interface-host/src/pane_mcp.rs::pass_off\x1f\x1e",
            "8f113822d1b709309f7a677fe5ac9dcf0e5466df\x1f1111111111111111111111111111111111111111 2222222222222222222222222222222222222222\x1f2026-10-21T08:52:32-05:00\x1f1792590752\x1f",
            "Name Antigravity panes from the conversation title\x1f",
            "Name Antigravity panes from the conversation title\n\nLanding-kind: feature\n",
            "Landing-check: Antigravity pane adopts its conversation title\n",
            "Landing-pin: crates/interface-terminal/src/antigravity_names.rs::title\n",
            "Landed-from: feat/antigravity-pane-titles\x1f",
            "feat/antigravity-pane-titles\x1ffeature\x1f",
            "Antigravity pane adopts its conversation title\x1f",
            "crates/interface-terminal/src/antigravity_names.rs::title\x1f\x1e"
        );
        let mut landings = parse_dev_landings(dev_log);
        assert_eq!(landings.len(), 2);

        let main_log = concat!(
            "8f113822d1b709309f7a677fe5ac9dcf0e5466df\x1f2026-10-21T08:52:32-05:00\x1f1792590752\x1f",
            "p1 p2\x1fName Antigravity panes from the conversation title\x1f\x1f",
            "feat/antigravity-pane-titles\x1f\x1f\x1e",
            "dda13b10c8435778be0454e973e7b77596f845bf\x1f2026-10-21T09:17:39-05:00\x1f1792592259\x1f",
            "8f113822d1b709309f7a677fe5ac9dcf0e5466df\x1fchore(release): bump Interface to 0.3.166\x1f\x1f\x1f",
            "8f11382 Name Antigravity panes from the conversation title\x1f\x1e"
        );
        let releases = parse_releases_and_link(main_log, &mut landings);
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].version, "0.3.166");
        assert_eq!(releases[0].items.len(), 1);
        assert_eq!(landings[0].release, None);
        assert_eq!(landings[1].release.as_deref(), Some("0.3.166"));

        let markdown = render_markdown(&UpdateLog { releases, landings });
        assert!(markdown.contains("## Unreleased on Dev"));
        assert!(markdown.contains("### Harden terminal pass-off"));
        assert!(markdown.contains("### v0.3.166 — 2026-10-21T09:17:39-05:00 (`8f11382`)"));
        assert!(markdown.contains("- **Release:** v0.3.166"));
        assert!(markdown.contains("- **Release:** Unreleased (on Dev)"));
    }

    fn fixture_landing(
        name: &str,
        sha: &str,
        (kind, release): (&str, Option<&str>),
        completed_at: &str,
        branch: &str,
        notes: &str,
    ) -> LoggedLanding {
        LoggedLanding {
            sha: sha.repeat(40 / sha.len()),
            merge_sha: String::new(),
            completed_at: completed_at.to_owned(),
            committed: 0,
            name: name.to_owned(),
            branch: branch.to_owned(),
            kind: kind.to_owned(),
            notes: notes.to_owned(),
            commits: Vec::new(),
            pins: Vec::new(),
            supersedes: Vec::new(),
            release: release.map(str::to_owned),
        }
    }

    fn fixture_release(version: &str, sha: &str, completed_at: &str) -> LoggedRelease {
        LoggedRelease {
            version: version.to_owned(),
            sha: sha.repeat(40 / sha.len()),
            completed_at: completed_at.to_owned(),
            committed: 0,
            items: Vec::new(),
            left_on_dev: Vec::new(),
        }
    }

    /// Settings -> Updates parses what [`render_markdown`] writes, but only
    /// the Electron suite runs that parser, and `docs/UPDATES.md` is in
    /// neither suite's receipt. Both sides read this fixture instead
    /// (`update-log.test.ts` parses it), so a format change here runs both.
    #[test]
    fn render_markdown_matches_the_shared_update_log_fixture() {
        let mut pass_off = fixture_landing(
            "Harden terminal pass-off",
            "1c2a9b53",
            ("fix", None),
            "2026-10-21T14:48:47-05:00",
            "fix/pass-off-review",
            "Passed-off panes launch for every profile and custom command",
        );
        pass_off.commits = vec![
            "fix(pass-off): quote custom shell commands".to_owned(),
            "test(pass-off): cover profiles".to_owned(),
        ];
        pass_off.pins = vec![
            "apps/interface-host/src/pane_mcp.rs::pass_off".to_owned(),
            "electron/src/renderer/pass-off.test.ts::custom".to_owned(),
        ];
        let mut antigravity = fixture_landing(
            "Name Antigravity panes from the conversation title",
            "2",
            ("feature", Some("0.3.166")),
            "2026-10-21T08:52:32-05:00",
            "feat/antigravity-pane-titles",
            "Antigravity pane adopts its conversation title automatically",
        );
        // A single commit that repeats the notes gets no Commits line.
        antigravity.commits = vec![antigravity.notes.clone()];
        antigravity.pins =
            vec!["crates/interface-terminal/src/antigravity_names.rs::title".to_owned()];
        let mut font_metrics = fixture_landing(
            "Cache terminal font metrics",
            "3",
            ("speed", Some("0.3.165")),
            "2026-10-20T17:30:00-05:00",
            "speed/font-metrics",
            "Startup terminal measurement runs once per font family",
        );
        font_metrics.supersedes = vec!["`0000000` Earlier font cache".to_owned()];

        let mut latest = fixture_release("0.3.166", "8f113822", "2026-10-21T09:17:39-05:00");
        latest.items = vec![release_item_from_landing(&antigravity)];
        latest.left_on_dev = vec!["`1c2a9b5` Harden terminal pass-off".to_owned()];
        let mut previous = fixture_release("0.3.165", "1", "2026-10-20T18:00:00-05:00");
        previous.items = vec![ReleaseItem {
            branch: None,
            ..release_item_from_landing(&font_metrics)
        }];
        let bump_only = fixture_release("0.3.164", "4", "2026-10-19T12:00:00-05:00");
        let log = UpdateLog {
            releases: vec![latest, previous, bump_only],
            landings: vec![pass_off, antigravity, font_metrics],
        };
        assert_eq!(
            render_markdown(&log),
            include_str!("../../docs/fixtures/update-log-v1.md"),
            "the update log format changed: regenerate docs/fixtures/update-log-v1.md \
             and update the parser in electron/src/renderer/update-log.ts with it"
        );
    }
}
