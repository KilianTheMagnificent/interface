//! Named landings on `dev`, and the release menu built from them.
//!
//! A landing is the merge commit `closeout` writes for one task branch: first
//! parent the previous `dev` tip, second parent the branch tip, subject the
//! owner-readable description, trailer `Landed-from: <branch>`. Because every
//! landing is a first-parent merge, `git log --first-parent --merges
//! origin/main..origin/dev` is the list of what dev has that main does not,
//! and `promote --change` cherry-picks a landing with `-m 1` to ship exactly
//! that change. The pure functions here take strings so they are unit tested
//! without a repository; the wrappers at the bottom run git.
//!
//! A landing also says what it is, in trailers `closeout` writes before
//! `Landed-from:`: its KIND, one CHECK line telling the owner what to look for,
//! the PIN tests that prove it, and any earlier landing it SUPERSEDES. Promote
//! reads them back to print what a release ships and what it leaves on Dev.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::gate;

/// The trailer every landing carries; commits without it are not landings.
pub const LANDED_FROM_TRAILER: &str = "Landed-from:";
/// The trailer a release merge-back carries instead, so the menu skips it.
pub const RELEASE_TRAILER: &str = "Release:";
/// The longest `--change` text closeout accepts.
pub const MAX_CHANGE_CHARS: usize = 100;
/// What a landing is: one of [`Kind`].
pub const KIND_TRAILER: &str = "Landing-kind:";
/// What the owner should see when the landing works.
pub const CHECK_TRAILER: &str = "Landing-check:";
/// A test that proves the landing: `<path>::<test name>`, repeatable.
pub const PIN_TRAILER: &str = "Landing-pin:";
/// An earlier landing whose behavior this one deliberately replaces.
pub const SUPERSEDES_TRAILER: &str = "Supersedes:";
/// On a release's bump commit: one landing the release carries.
pub const SHIPS_TRAILER: &str = "Ships:";
/// On a release's bump commit: one user-facing landing it leaves on Dev.
pub const LEAVES_OUT_TRAILER: &str = "Leaves-out:";
/// The longest `--check` text closeout accepts.
pub const MAX_CHECK_CHARS: usize = 160;

/// What a landing is. A landing written before kinds existed has none and is
/// treated as user-facing, so it is never hidden.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Feature,
    Fix,
    Look,
    Speed,
    Internal,
    Record,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Feature,
        Kind::Fix,
        Kind::Look,
        Kind::Speed,
        Kind::Internal,
        Kind::Record,
    ];

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_lowercase();
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Feature => "feature",
            Kind::Fix => "fix",
            Kind::Look => "look",
            Kind::Speed => "speed",
            Kind::Internal => "internal",
            Kind::Record => "record",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Kind::Feature => "Features",
            Kind::Fix => "Fixes",
            Kind::Look => "Looks",
            Kind::Speed => "Speed",
            Kind::Internal => "Internal",
            Kind::Record => "Record",
        }
    }

    fn needs_check(self) -> bool {
        matches!(self, Kind::Feature | Kind::Fix | Kind::Look | Kind::Speed)
    }

    fn needs_pin(self) -> bool {
        matches!(self, Kind::Feature | Kind::Fix | Kind::Look)
    }
}

/// Whether the owner has to hear about this landing: everything but
/// `internal` and `record`, including landings without a kind.
pub fn user_facing(kind: Option<Kind>) -> bool {
    !matches!(kind, Some(Kind::Internal | Kind::Record))
}

fn kinds_list() -> String {
    Kind::ALL.map(Kind::as_str).join(", ")
}

/// The trailers a landing carries besides its name and branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LandingMeta {
    pub kind: Kind,
    pub check: Option<String>,
    pub pins: Vec<String>,
    pub supersedes: Vec<String>,
}

/// Validates closeout's `--kind`, `--check` and `--pin` against the kind
/// rules. A docs-only landing without `--kind` is `record`; any other landing
/// must say what it is. Pins are checked for shape here and against the tree
/// by `pins::validate_new`.
pub fn landing_meta(
    kind: Option<&str>,
    check: Option<&str>,
    pins: &[String],
    docs_only: bool,
) -> Result<LandingMeta, String> {
    let kind = match kind {
        Some(text) => Kind::parse(text).ok_or_else(|| {
            format!(
                "--kind \"{}\" is not a kind; use one of {}",
                text.trim(),
                kinds_list()
            )
        })?,
        None if docs_only => Kind::Record,
        None => {
            return Err(format!(
                "closeout needs --kind, one of {}: what this landing is, so a release can say \
                 what it ships (a docs-only landing is `record` without it)",
                kinds_list()
            ));
        }
    };
    let check = match check {
        Some(text) => {
            let text = text.trim();
            if text.is_empty() {
                return Err("--check is empty; say what the owner should see".to_owned());
            }
            if text.contains('\n') || text.contains('\r') {
                return Err("--check must be one line".to_owned());
            }
            let count = text.chars().count();
            if count > MAX_CHECK_CHARS {
                return Err(format!(
                    "--check is {count} characters; keep it to {MAX_CHECK_CHARS}"
                ));
            }
            Some(text.to_owned())
        }
        None if kind.needs_check() => {
            return Err(format!(
                "a {} landing needs --check \"<what the owner should see when it works>\"",
                kind.as_str()
            ));
        }
        None => None,
    };
    let mut unique: Vec<String> = Vec::new();
    for pin in pins {
        let pin = pin.trim();
        crate::pins::parse_pin(pin)?;
        if !unique.iter().any(|seen| seen == pin) {
            unique.push(pin.to_owned());
        }
    }
    if unique.is_empty() && kind.needs_pin() {
        return Err(format!(
            "a {} landing needs at least one --pin <path>::<test name>: the test that proves it, \
             so a later change cannot undo it silently",
            kind.as_str()
        ));
    }
    Ok(LandingMeta {
        kind,
        check,
        pins: unique,
        supersedes: Vec::new(),
    })
}

/// The landing's subject: the `--change` text, or the single commit subject
/// when the branch adds exactly one non-merge commit and no text was given.
pub fn landing_name(change: Option<&str>, subjects: &[String]) -> Result<String, String> {
    if let Some(change) = change {
        let name = change.trim();
        if name.is_empty() {
            return Err("--change must say what this lands; it is empty".to_owned());
        }
        if name.contains('\n') || name.contains('\r') {
            return Err("--change must be one line".to_owned());
        }
        let count = name.chars().count();
        if count > MAX_CHANGE_CHARS {
            return Err(format!(
                "--change is {count} characters; keep it to {MAX_CHANGE_CHARS}"
            ));
        }
        return Ok(name.to_owned());
    }
    match subjects {
        [only] => Ok(only.trim().to_owned()),
        _ => Err(format!(
            "closeout needs --change \"one line saying what this lands\": the branch adds {} \
             commits, so no single subject can name the landing",
            subjects.len()
        )),
    }
}

/// The landing commit message: the name, the subjects landed, then one
/// trailer block — kind, check, pins, supersedes, and `Landed-from:` last.
pub fn landing_message(
    name: &str,
    subjects: &[String],
    meta: &LandingMeta,
    branch: &str,
) -> String {
    let mut message = format!("{name}\n\n");
    if !subjects.is_empty() {
        for subject in subjects {
            message.push_str(subject.trim());
            message.push('\n');
        }
        message.push('\n');
    }
    let _ = writeln!(message, "{KIND_TRAILER} {}", meta.kind.as_str());
    if let Some(check) = &meta.check {
        let _ = writeln!(message, "{CHECK_TRAILER} {check}");
    }
    for pin in &meta.pins {
        let _ = writeln!(message, "{PIN_TRAILER} {pin}");
    }
    for supersede in &meta.supersedes {
        let _ = writeln!(message, "{SUPERSEDES_TRAILER} {supersede}");
    }
    let _ = writeln!(message, "{LANDED_FROM_TRAILER} {branch}");
    message
}

/// One landing on `dev`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Landing {
    /// Full object name.
    pub sha: String,
    /// Author date, `YYYY-MM-DD`.
    pub date: String,
    /// The subject: the owner-readable description.
    pub name: String,
    /// The `Landed-from:` trailer value.
    pub branch: String,
    /// Committer time, Unix seconds.
    pub committed: i64,
    /// `None` for a landing written before kinds existed.
    pub kind: Option<Kind>,
    pub check: Option<String>,
    pub pins: Vec<String>,
    pub supersedes: Vec<String>,
}

/// The `git log --format` that [`parse_landings`] reads: sha, date, committer
/// time, subject, then the `Landed-from`, kind, check, pin and supersedes
/// trailer values, separated by U+001F, one record per U+001E. Control
/// separators cannot appear in a subject, a branch name or a trailer.
pub const LANDING_LOG_FORMAT: &str = "%H%x1f%as%x1f%ct%x1f%s\
%x1f%(trailers:key=Landed-from,valueonly)%x1f%(trailers:key=Landing-kind,valueonly)\
%x1f%(trailers:key=Landing-check,valueonly)%x1f%(trailers:key=Landing-pin,valueonly)\
%x1f%(trailers:key=Supersedes,valueonly)%x1e";

fn trailer_lines(values: Option<&str>) -> Vec<String> {
    values
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The landings in a `git log --first-parent --merges` run over
/// `origin/main..origin/dev`, oldest first. A merge without the trailer (a
/// release merge-back, a hand merge) is not a landing and is dropped.
pub fn parse_landings(log: &str) -> Vec<Landing> {
    let mut landings: Vec<Landing> = log
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_matches('\n').splitn(9, '\x1f');
            let sha = fields.next()?.trim();
            let date = fields.next()?.trim();
            let committed = fields.next()?.trim().parse().unwrap_or(0);
            let name = fields.next()?.trim();
            let branch = fields.next()?.lines().next().unwrap_or("").trim();
            if sha.is_empty() || branch.is_empty() {
                return None;
            }
            let kind = trailer_lines(fields.next())
                .first()
                .and_then(|kind| Kind::parse(kind));
            let check = trailer_lines(fields.next()).into_iter().next();
            Some(Landing {
                sha: sha.to_owned(),
                date: date.to_owned(),
                name: name.to_owned(),
                branch: branch.to_owned(),
                committed,
                kind,
                check,
                pins: trailer_lines(fields.next()),
                supersedes: trailer_lines(fields.next()),
            })
        })
        .collect();
    landings.reverse();
    landings
}

/// Every landing `main` already carries as a `-x` cherry-pick, read off the
/// `(cherry picked from commit <sha>)` lines in `git log --format=%b origin/main`.
pub fn shipped_shas(main_log: &str) -> BTreeSet<String> {
    main_log
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("(cherry picked from commit ")?
                .strip_suffix(')')
        })
        .map(str::trim)
        .filter(|sha| sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_owned)
        .collect()
}

/// The landings whose SHA is not in `shipped`, in the order given.
pub fn unshipped(landings: Vec<Landing>, shipped: &BTreeSet<String>) -> Vec<Landing> {
    landings
        .into_iter()
        .filter(|landing| !shipped.contains(&landing.sha))
        .collect()
}

/// For each landing that shares a changed path with another, the names of
/// those others, keyed by SHA. Landings with no shared path are absent.
pub fn overlaps(paths: &[(Landing, Vec<String>)]) -> BTreeMap<String, Vec<String>> {
    let mut map = BTreeMap::new();
    for (landing, own) in paths {
        let others: Vec<String> = paths
            .iter()
            .filter(|(other, theirs)| {
                other.sha != landing.sha && theirs.iter().any(|path| own.contains(path))
            })
            .map(|(other, _)| other.name.clone())
            .collect();
        if !others.is_empty() {
            map.insert(landing.sha.clone(), others);
        }
    }
    map
}

/// The one landing `wanted` names: a SHA prefix of at least four hex digits,
/// or a case-insensitive substring of the name. Anything but exactly one match
/// is refused with the candidates, and so is naming nothing at all.
pub fn resolve<'a>(landings: &'a [Landing], wanted: &str) -> Result<&'a Landing, String> {
    let wanted = wanted.trim();
    // Empty text is a substring of every name, so it matches the whole menu —
    // and would resolve, silently, whenever exactly one landing is unshipped.
    if wanted.is_empty() {
        return Err("--change must name a landing (a name substring or a SHA prefix)".to_owned());
    }
    let lowered = wanted.to_lowercase();
    let is_hex = wanted.len() >= 4 && wanted.chars().all(|c| c.is_ascii_hexdigit());
    let matches: Vec<&Landing> = landings
        .iter()
        .filter(|landing| {
            (is_hex && landing.sha.starts_with(&lowered))
                || landing.name.to_lowercase().contains(&lowered)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "no unshipped landing matches \"{wanted}\"; run `cargo xtask landings` for the menu"
        )),
        many => Err(format!(
            "\"{wanted}\" matches {} landings: {}; name one of them",
            many.len(),
            many.iter()
                .map(|landing| format!("{} \"{}\"", gate::short(&landing.sha), landing.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The order the menu and the release summary group landings in: what the
/// owner sees first, then landings with no kind, then the rest.
fn group_order(kind: Option<Kind>) -> u8 {
    match kind {
        Some(Kind::Feature) => 0,
        Some(Kind::Fix) => 1,
        Some(Kind::Look) => 2,
        Some(Kind::Speed) => 3,
        None => 4,
        Some(Kind::Internal) => 5,
        Some(Kind::Record) => 6,
    }
}

fn group_heading(kind: Option<Kind>) -> &'static str {
    kind.map_or("Unclassified", Kind::heading)
}

/// The menu: landings grouped by kind, oldest first within a group, each with
/// its check line, how many releases it has waited, and the overlap line.
pub fn render(
    landings: &[Landing],
    overlaps: &BTreeMap<String, Vec<String>>,
    waited: &BTreeMap<String, usize>,
) -> String {
    let mut sorted: Vec<&Landing> = landings.iter().collect();
    sorted.sort_by_key(|landing| group_order(landing.kind));
    let mut text = String::new();
    let mut heading = None;
    for landing in sorted {
        let group = group_heading(landing.kind);
        if heading != Some(group) {
            let _ = writeln!(text, "{group}");
            heading = Some(group);
        }
        let _ = write!(
            text,
            "  {}  {}  {}  ({})",
            gate::short(&landing.sha),
            landing.date,
            landing.name,
            landing.branch
        );
        match waited.get(&landing.sha).copied().unwrap_or(0) {
            0 => {}
            1 => text.push_str("  waited 1 release"),
            count => {
                let _ = write!(text, "  waited {count} releases");
            }
        }
        text.push('\n');
        if let Some(check) = &landing.check {
            let _ = writeln!(text, "      check: {check}");
        }
        if let Some(others) = overlaps.get(&landing.sha) {
            let _ = writeln!(text, "      touches files with: {}", others.join(", "));
        }
    }
    text
}

/// How many of the release times in `bumps` came after each landing: the
/// releases that went out while it waited on Dev.
pub fn waited(landings: &[Landing], bumps: &[i64]) -> BTreeMap<String, usize> {
    landings
        .iter()
        .map(|landing| {
            let count = bumps
                .iter()
                .filter(|time| **time > landing.committed)
                .count();
            (landing.sha.clone(), count)
        })
        .collect()
}

/// One landing as a promotion record keeps it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub sha: String,
    pub name: String,
    #[serde(default)]
    pub kind: Option<Kind>,
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default)]
    pub pins: Vec<String>,
}

impl From<&Landing> for Entry {
    fn from(landing: &Landing) -> Self {
        Self {
            sha: landing.sha.clone(),
            name: landing.name.clone(),
            kind: landing.kind,
            check: landing.check.clone(),
            pins: landing.pins.clone(),
        }
    }
}

/// An earlier landing a release replaces, from an included landing's
/// `Supersedes:` trailer. `sha` and `name` are the earlier landing's, or the
/// reference as written when it names no landing git can find.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Replacement {
    pub sha: String,
    pub name: String,
    pub reason: String,
    /// The name of the included landing that carries the trailer.
    pub by: String,
}

/// The replacements the `included` landings declare, each resolved against
/// `known` (every landing git can see) by SHA prefix or exact name.
pub fn replacements(included: &[&Landing], known: &[Landing]) -> Vec<Replacement> {
    let mut result = Vec::new();
    for landing in included {
        for value in &landing.supersedes {
            let Some(supersede) = crate::pins::parse_supersede(value) else {
                continue;
            };
            let earlier = known
                .iter()
                .find(|other| supersede.names(std::slice::from_ref(&other.sha), &other.name));
            result.push(Replacement {
                sha: earlier.map_or_else(|| supersede.reference.clone(), |e| e.sha.clone()),
                name: earlier.map_or_else(String::new, |e| e.name.clone()),
                reason: supersede.reason.clone(),
                by: landing.name.clone(),
            });
        }
    }
    result
}

fn counts_line(entries: &[&Entry]) -> Option<String> {
    let internal = entries
        .iter()
        .filter(|entry| entry.kind == Some(Kind::Internal))
        .count();
    let record = entries
        .iter()
        .filter(|entry| entry.kind == Some(Kind::Record))
        .count();
    (internal + record > 0).then(|| format!("{internal} internal, {record} record"))
}

/// What a release carries, grouped for the owner: user-facing landings by
/// kind with their check lines, what stays on Dev, what earlier behavior is
/// replaced, and counts for internal and record landings.
pub fn release_summary(
    version: &str,
    included: &[Entry],
    left_out: &[Entry],
    replaced: &[Replacement],
) -> String {
    let mut text = match included.len() {
        1 => format!("Interface {version} — ships 1 landing\n"),
        count => format!("Interface {version} — ships {count} landings\n"),
    };
    let mut shown: Vec<&Entry> = included.iter().filter(|e| user_facing(e.kind)).collect();
    shown.sort_by_key(|entry| group_order(entry.kind));
    let mut heading = None;
    for entry in shown {
        let group = group_heading(entry.kind);
        if heading != Some(group) {
            let _ = writeln!(text, "{group}");
            heading = Some(group);
        }
        let _ = writeln!(text, "  {} {}", gate::short(&entry.sha), entry.name);
        if let Some(check) = &entry.check {
            let _ = writeln!(text, "          check: {check}");
        }
    }
    let hidden: Vec<&Entry> = left_out.iter().filter(|e| !user_facing(e.kind)).collect();
    let visible: Vec<&Entry> = left_out.iter().filter(|e| user_facing(e.kind)).collect();
    if !visible.is_empty() || !hidden.is_empty() {
        let _ = writeln!(text, "Left out — stays on Dev only");
        for entry in &visible {
            let kind = entry.kind.map_or("unclassified", Kind::as_str);
            let _ = writeln!(
                text,
                "  {} {}  ({kind})",
                gate::short(&entry.sha),
                entry.name
            );
        }
        if let Some(counts) = counts_line(&hidden) {
            let _ = writeln!(text, "  and {counts}");
        }
    }
    if !replaced.is_empty() {
        let _ = writeln!(text, "Replaces earlier behavior");
        for replacement in replaced {
            let _ = writeln!(
                text,
                "  {} {}\n          replaced by \"{}\": {}",
                gate::short(&replacement.sha),
                replacement.name,
                replacement.by,
                replacement.reason
            );
        }
    }
    let internal: Vec<&Entry> = included.iter().filter(|e| !user_facing(e.kind)).collect();
    if let Some(counts) = counts_line(&internal) {
        let _ = writeln!(text, "Also: {counts}");
    }
    text
}

/// The trailers a release's bump commit carries after its `Promotion-run:`
/// marker: every included landing, then every user-facing one left out.
pub fn release_trailers(included: &[Entry], left_out: &[Entry]) -> String {
    let mut text = String::new();
    for entry in included {
        let _ = writeln!(
            text,
            "{SHIPS_TRAILER} {} {}",
            gate::short(&entry.sha),
            entry.name
        );
    }
    for entry in left_out.iter().filter(|e| user_facing(e.kind)) {
        let _ = writeln!(
            text,
            "{LEAVES_OUT_TRAILER} {} {}",
            gate::short(&entry.sha),
            entry.name
        );
    }
    text
}

/// The user-facing `left_out` landings no `--leave-out` selector names. A
/// selector that names nothing, or names a landing the release includes, is
/// refused: acknowledging an omission has to mean one.
pub fn unacknowledged<'a>(
    all: &'a [Landing],
    left_out: &[&'a Landing],
    leave_out: &[String],
) -> Result<Vec<&'a Landing>, String> {
    let mut named = BTreeSet::new();
    for selector in leave_out {
        let landing =
            resolve(all, selector).map_err(|refusal| format!("--leave-out: {refusal}"))?;
        if !left_out.iter().any(|other| other.sha == landing.sha) {
            return Err(format!(
                "--leave-out \"{}\" names {} \"{}\", which this release includes",
                selector.trim(),
                gate::short(&landing.sha),
                landing.name
            ));
        }
        named.insert(landing.sha.clone());
    }
    Ok(left_out
        .iter()
        .filter(|landing| user_facing(landing.kind) && !named.contains(&landing.sha))
        .copied()
        .collect())
}

/// The refusal for user-facing landings a release would leave out silently.
pub fn unacknowledged_message(landings: &[&Landing]) -> String {
    let mut text = format!(
        "this release would leave {} user-facing landing(s) on Dev without saying so:\n",
        landings.len()
    );
    for landing in landings {
        let kind = landing.kind.map_or("unclassified", Kind::as_str);
        let _ = writeln!(
            text,
            "    {} \"{}\" ({kind}){}",
            gate::short(&landing.sha),
            landing.name,
            landing
                .check
                .as_ref()
                .map_or_else(String::new, |check| format!(": {check}"))
        );
    }
    text.push_str(
        "Show the owner the summary above. To keep one on Dev, add --leave-out \"<sha>\" for it; \
         to ship it, include it with --change or promote a revision that contains it.",
    );
    text
}

/// What promote says when a landing does not apply on top of `main`.
pub fn conflict_message(landing: &Landing, overlapping: &[String]) -> String {
    let mut text = format!(
        "landing {} \"{}\" does not apply on main by itself.",
        gate::short(&landing.sha),
        landing.name
    );
    if overlapping.is_empty() {
        text.push_str(" No other unshipped landing touches its files; ");
    } else {
        let _ = write!(
            text,
            " It shares files with: {}. Add them with --change, or ",
            overlapping.join(", ")
        );
    }
    text.push_str("ship everything with `promote --version <v>` and no --change.");
    text
}

/// The landings `origin/dev` has that `origin/main` lacks, oldest first, with
/// the ones `main` already carries as `-x` cherry-picks removed. Reads the
/// remote-tracking refs as they are; the caller fetches first.
pub fn unshipped_landings(root: &Path) -> Result<Vec<Landing>> {
    unshipped_at(
        root,
        &gate::rev_parse(root, "origin/main")?,
        &gate::rev_parse(root, "origin/dev")?,
    )
}

pub fn unshipped_at(root: &Path, main: &str, dev: &str) -> Result<Vec<Landing>> {
    let log = gate::git_capture(
        root,
        &[
            "log",
            "--first-parent",
            "--merges",
            &format!("--format={LANDING_LOG_FORMAT}"),
            &format!("{main}..{dev}"),
        ],
    )?;
    let main = gate::git_capture(root, &["log", "--format=%b", main])?;
    let mut landings = unshipped(parse_landings(&log), &shipped_shas(&main));
    // A landing from before kinds that only touched documentation is
    // bookkeeping: the same `record` closeout gives such a landing today.
    for landing in landings.iter_mut().filter(|landing| landing.kind.is_none()) {
        if docs_only(&landing_paths(root, &landing.sha)?) {
            landing.kind = Some(Kind::Record);
        }
    }
    Ok(landings)
}

/// Whether every path is documentation, including the docs a suite reads.
pub fn docs_only<S: AsRef<str>>(paths: &[S]) -> bool {
    paths
        .iter()
        .all(|path| gate::is_documentation(path.as_ref()))
}

/// The paths a landing changes: its first-parent diff.
pub fn landing_paths(root: &Path, sha: &str) -> Result<Vec<String>> {
    let mut paths: Vec<String> =
        gate::git_capture(root, &["diff", "--name-only", &format!("{sha}^1"), sha])?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
    if paths.len() > 1 {
        paths.retain(|path| path != crate::update_log::UPDATES_DOC);
    }
    Ok(paths)
}

/// [`overlaps`] over the real diffs of `landings`.
pub fn landing_overlaps(
    root: &Path,
    landings: &[Landing],
) -> Result<BTreeMap<String, Vec<String>>> {
    let mut paths = Vec::with_capacity(landings.len());
    for landing in landings {
        paths.push((landing.clone(), landing_paths(root, &landing.sha)?));
    }
    Ok(overlaps(&paths))
}

/// `cargo xtask landings`: fetch, then print the menu.
pub fn landings_command(root: &Path) -> Result<()> {
    gate::git_capture(root, &["fetch", "origin"])?;
    let landings = unshipped_landings(root)?;
    if landings.is_empty() {
        println!("landings: dev has nothing main lacks");
        return Ok(());
    }
    let overlaps = landing_overlaps(root, &landings)?;
    let waited = waited(&landings, &release_times(root, "origin/main")?);
    print!("{}", render(&landings, &overlaps, &waited));
    println!(
        "landings: {} unshipped; ship one with `cargo xtask promote --version <v> --change \
         \"<name or sha>\"`",
        landings.len()
    );
    Ok(())
}

/// Committer times of the release bump commits `rev` contains.
pub fn release_times(root: &Path, rev: &str) -> Result<Vec<i64>> {
    Ok(gate::git_capture(
        root,
        &[
            "log",
            "--fixed-strings",
            "--grep=chore(release): bump Interface to",
            "--format=%ct",
            rev,
        ],
    )?
    .lines()
    .filter_map(|line| line.trim().parse().ok())
    .collect())
}

/// Every landing `rev` contains, merges and cherry-picks alike: any commit
/// carrying `Landed-from:`. Oldest first.
pub fn known_landings(root: &Path, rev: &str) -> Result<Vec<Landing>> {
    let log = gate::git_capture(
        root,
        &[
            "log",
            "--fixed-strings",
            &format!("--grep={LANDED_FROM_TRAILER}"),
            &format!("--format={LANDING_LOG_FORMAT}"),
            rev,
        ],
    )?;
    Ok(parse_landings(&log))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subjects(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn landing_name_takes_the_change_text_or_the_single_subject() {
        assert_eq!(
            landing_name(
                Some("  Saved workspaces file under their own project "),
                &[]
            ),
            Ok("Saved workspaces file under their own project".to_owned())
        );
        assert_eq!(
            landing_name(None, &subjects(&["fix(sidebar): one thing"])),
            Ok("fix(sidebar): one thing".to_owned())
        );
        let two = landing_name(None, &subjects(&["a", "b"])).unwrap_err();
        assert!(two.contains("--change"), "{two}");
        assert!(two.contains("2 commits"), "{two}");
        let none = landing_name(None, &[]).unwrap_err();
        assert!(none.contains("--change"), "{none}");
    }

    #[test]
    fn landing_name_refuses_empty_multiline_and_long_text() {
        assert!(
            landing_name(Some("   "), &[])
                .unwrap_err()
                .contains("empty")
        );
        assert!(
            landing_name(Some("one\ntwo"), &[])
                .unwrap_err()
                .contains("one line")
        );
        let long = "x".repeat(MAX_CHANGE_CHARS + 1);
        let refusal = landing_name(Some(&long), &[]).unwrap_err();
        assert!(refusal.contains("101 characters"), "{refusal}");
        assert!(landing_name(Some(&"y".repeat(MAX_CHANGE_CHARS)), &[]).is_ok());
    }

    fn pins(list: &[&str]) -> Vec<String> {
        subjects(list)
    }

    #[test]
    fn landing_meta_applies_the_kind_rules() {
        let pin = "a.rs::proves_it";
        // Feature, fix and look need a check and a pin.
        for kind in ["feature", "fix", "look"] {
            let meta = landing_meta(Some(kind), Some("You see it"), &pins(&[pin]), false)
                .expect("complete");
            assert_eq!(meta.kind.as_str(), kind);
            let no_check = landing_meta(Some(kind), None, &pins(&[pin]), false).unwrap_err();
            assert!(no_check.contains("--check"), "{no_check}");
            let no_pin = landing_meta(Some(kind), Some("You see it"), &[], false).unwrap_err();
            assert!(no_pin.contains("--pin"), "{no_pin}");
        }
        // Speed needs a check but not a pin; internal and record need neither.
        assert!(landing_meta(Some("speed"), Some("Faster"), &[], false).is_ok());
        assert!(landing_meta(Some("speed"), None, &[], false).is_err());
        assert!(landing_meta(Some("Internal"), None, &[], false).is_ok());
        assert!(landing_meta(Some("record"), None, &[], false).is_ok());
        // Docs-only defaults to record; an explicit kind still wins.
        assert_eq!(
            landing_meta(None, None, &[], true).unwrap().kind,
            Kind::Record
        );
        assert_eq!(
            landing_meta(Some("internal"), None, &[], true)
                .unwrap()
                .kind,
            Kind::Internal
        );
        let missing = landing_meta(None, None, &[], false).unwrap_err();
        assert!(missing.contains("--kind"), "{missing}");
        assert!(
            missing.contains("feature, fix, look, speed, internal, record"),
            "{missing}"
        );
        let unknown = landing_meta(Some("chore"), None, &[], false).unwrap_err();
        assert!(unknown.contains("\"chore\" is not a kind"), "{unknown}");
        // Duplicate pins collapse; malformed ones are refused.
        let meta = landing_meta(
            Some("internal"),
            None,
            &pins(&[pin, " a.rs::proves_it "]),
            false,
        )
        .unwrap();
        assert_eq!(meta.pins, pins(&[pin]));
        assert!(landing_meta(Some("internal"), None, &pins(&["nope"]), false).is_err());
    }

    #[test]
    fn landing_meta_refuses_empty_multiline_and_long_checks() {
        let check = |text: &str| landing_meta(Some("speed"), Some(text), &[], false);
        assert!(check("  ").unwrap_err().contains("empty"));
        assert!(check("a\nb").unwrap_err().contains("one line"));
        let long = "x".repeat(MAX_CHECK_CHARS + 1);
        assert!(check(&long).unwrap_err().contains("161 characters"));
        assert_eq!(
            check(&format!(" {} ", "y".repeat(MAX_CHECK_CHARS)))
                .unwrap()
                .check,
            Some("y".repeat(MAX_CHECK_CHARS))
        );
    }

    #[test]
    fn landing_message_lists_subjects_then_one_trailer_block() {
        let mut meta = landing_meta(
            Some("feature"),
            Some("Dragging shows a Swap preview"),
            &pins(&["a.ts::shows swap", "b.rs::drags"]),
            false,
        )
        .unwrap();
        meta.supersedes = vec!["1234567 — replaced".to_owned()];
        assert_eq!(
            landing_message("Name", &subjects(&["first", "second"]), &meta, "fix/x"),
            "Name\n\nfirst\nsecond\n\nLanding-kind: feature\n\
             Landing-check: Dragging shows a Swap preview\nLanding-pin: a.ts::shows swap\n\
             Landing-pin: b.rs::drags\nSupersedes: 1234567 — replaced\nLanded-from: fix/x\n"
        );
        let record = landing_meta(None, None, &[], true).unwrap();
        assert_eq!(
            landing_message("Name", &[], &record, "fix/x"),
            "Name\n\nLanding-kind: record\nLanded-from: fix/x\n"
        );
    }

    /// Two landings (one with every trailer, one from before kinds) and a
    /// release merge-back, newest first, as `git log` prints them.
    const LOG: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\x1f2026-09-07\x1f1790000200\x1fSecond landing\
\x1ffeat/two\n\x1ffeature\n\x1fYou see two\n\x1fa.ts::one\nb.rs::two\n\x1f1234567 — old\n\x1e\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x1f2026-09-06\x1f1790000100\x1fFirst landing\x1ffix/one\n\
\x1f\x1f\x1f\x1f\x1e\
9999999999999999999999999999999999999999\x1f2026-09-05\x1f1790000000\x1fRelease 0.3.66 merged back \
to dev\x1f\x1f\x1f\x1f\x1f\x1e";

    fn landing(sha: &str, name: &str) -> Landing {
        Landing {
            sha: sha.repeat(40),
            date: "2026-09-06".to_owned(),
            name: name.to_owned(),
            branch: "b".to_owned(),
            committed: 100,
            kind: None,
            check: None,
            pins: vec![],
            supersedes: vec![],
        }
    }

    fn kinded(sha: &str, name: &str, kind: Kind) -> Landing {
        Landing {
            kind: Some(kind),
            check: Some(format!("see {name}")),
            ..landing(sha, name)
        }
    }

    #[test]
    fn parse_landings_keeps_trailer_carriers_oldest_first() {
        let landings = parse_landings(LOG);
        assert_eq!(landings.len(), 2);
        assert_eq!(landings[0].name, "First landing");
        assert_eq!(landings[0].sha, "a".repeat(40));
        assert_eq!(landings[0].date, "2026-09-06");
        assert_eq!(landings[0].branch, "fix/one");
        assert_eq!(landings[0].committed, 1_790_000_100);
        assert_eq!(landings[0].kind, None, "a landing from before kinds");
        assert!(landings[0].pins.is_empty());
        assert_eq!(landings[1].name, "Second landing");
        assert_eq!(landings[1].kind, Some(Kind::Feature));
        assert_eq!(landings[1].check.as_deref(), Some("You see two"));
        assert_eq!(landings[1].pins, pins(&["a.ts::one", "b.rs::two"]));
        assert_eq!(landings[1].supersedes, pins(&["1234567 — old"]));
        assert!(parse_landings("").is_empty());
    }

    #[test]
    fn shipped_shas_reads_cherry_pick_lines_only() {
        let body = "Promoted\n\n(cherry picked from commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)\n\
                    (cherry picked from commit notahash)\nsome other line\n";
        let shipped = shipped_shas(body);
        assert_eq!(shipped.len(), 1);
        assert!(shipped.contains(&"a".repeat(40)));
        let left = unshipped(parse_landings(LOG), &shipped);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].name, "Second landing");
    }

    #[test]
    fn overlaps_names_the_other_landings_sharing_a_path() {
        let a = landing("a", "A");
        let b = landing("b", "B");
        let c = landing("c", "C");
        let paths = vec![
            (a.clone(), vec!["x.rs".to_owned(), "y.rs".to_owned()]),
            (b.clone(), vec!["y.rs".to_owned()]),
            (c.clone(), vec!["z.rs".to_owned()]),
        ];
        let map = overlaps(&paths);
        assert_eq!(map.get(&a.sha), Some(&vec!["B".to_owned()]));
        assert_eq!(map.get(&b.sha), Some(&vec!["A".to_owned()]));
        assert_eq!(map.get(&c.sha), None);
    }

    #[test]
    fn resolve_by_unique_sha_prefix_or_name_substring() {
        let landings = vec![
            landing("a", "Saved workspaces"),
            landing("b", "Attention tray"),
        ];
        assert_eq!(resolve(&landings, "aaaa").unwrap().name, "Saved workspaces");
        assert_eq!(
            resolve(&landings, "attention").unwrap().name,
            "Attention tray"
        );
        assert_eq!(
            resolve(&landings, "SAVED").unwrap().name,
            "Saved workspaces"
        );
        let none = resolve(&landings, "nothing").unwrap_err();
        assert!(none.contains("cargo xtask landings"), "{none}");
        let both = resolve(&landings, "a").unwrap_err();
        assert!(both.contains("2 landings"), "{both}");
        // Naming nothing is refused before it matches everything.
        let empty = resolve(&landings[..1], "  ").unwrap_err();
        assert!(empty.contains("must name a landing"), "{empty}");
    }

    #[test]
    fn render_groups_by_kind_with_checks_waits_and_overlaps() {
        let a = landing("a", "A");
        let b = kinded("b", "B", Kind::Fix);
        let c = kinded("c", "C", Kind::Record);
        let mut map = BTreeMap::new();
        map.insert(a.sha.clone(), vec!["B".to_owned()]);
        let waits = waited(&[a.clone(), b.clone(), c.clone()], &[50, 150, 200]);
        assert_eq!(waits.get(&a.sha), Some(&2));
        let text = render(&[a, b, c], &map, &waits);
        assert_eq!(
            text,
            "Fixes\n  bbbbbbb  2026-09-06  B  (b)  waited 2 releases\n      check: see B\n\
             Unclassified\n  aaaaaaa  2026-09-06  A  (b)  waited 2 releases\n\
             \x20     touches files with: B\n\
             Record\n  ccccccc  2026-09-06  C  (b)  waited 2 releases\n      check: see C\n"
        );
        let once = render(
            &[landing("d", "D")],
            &BTreeMap::new(),
            &waited(&[landing("d", "D")], &[101]),
        );
        assert!(once.contains("waited 1 release\n"), "{once}");
    }

    fn entry(landing: &Landing) -> Entry {
        Entry::from(landing)
    }

    #[test]
    fn release_summary_groups_user_facing_landings_and_counts_the_rest() {
        let included = vec![
            entry(&kinded("c", "Internal cleanup", Kind::Internal)),
            entry(&kinded("b", "Fix drag", Kind::Fix)),
            entry(&kinded("a", "Swap preview", Kind::Feature)),
            entry(&landing("d", "Old landing")),
            entry(&kinded("e", "Notes", Kind::Record)),
        ];
        let left_out = vec![
            entry(&kinded("f", "Slide preview", Kind::Look)),
            entry(&kinded("9", "More notes", Kind::Record)),
        ];
        let replaced = vec![Replacement {
            sha: "8".repeat(40),
            name: "Thin caret".to_owned(),
            reason: "owner chose blue".to_owned(),
            by: "Fix drag".to_owned(),
        }];
        assert_eq!(
            release_summary("0.3.136", &included, &left_out, &replaced),
            "Interface 0.3.136 — ships 5 landings\n\
             Features\n  aaaaaaa Swap preview\n          check: see Swap preview\n\
             Fixes\n  bbbbbbb Fix drag\n          check: see Fix drag\n\
             Unclassified\n  ddddddd Old landing\n\
             Left out — stays on Dev only\n  fffffff Slide preview  (look)\n  and 0 internal, 1 record\n\
             Replaces earlier behavior\n  8888888 Thin caret\n          replaced by \"Fix drag\": owner chose blue\n\
             Also: 1 internal, 1 record\n"
        );
        assert_eq!(
            release_summary("1.0.0", &included[..1], &[], &[]),
            "Interface 1.0.0 — ships 1 landing\nAlso: 1 internal, 0 record\n"
        );
    }

    #[test]
    fn release_trailers_ship_everything_and_leave_out_the_user_facing() {
        let included = vec![
            entry(&kinded("a", "A", Kind::Feature)),
            entry(&kinded("c", "C", Kind::Record)),
        ];
        let left_out = vec![
            entry(&landing("b", "B")),
            entry(&kinded("d", "D", Kind::Internal)),
        ];
        assert_eq!(
            release_trailers(&included, &left_out),
            "Ships: aaaaaaa A\nShips: ccccccc C\nLeaves-out: bbbbbbb B\n"
        );
    }

    #[test]
    fn unacknowledged_lists_silent_user_facing_omissions() {
        let all = vec![
            kinded("a", "Swap preview", Kind::Feature),
            landing("b", "Old landing"),
            kinded("c", "Notes", Kind::Record),
            kinded("d", "Faster", Kind::Speed),
        ];
        let left_out: Vec<&Landing> = all[1..].iter().collect();
        let silent = unacknowledged(&all, &left_out, &[]).unwrap();
        let silent_names: Vec<&str> = silent.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(silent_names, vec!["Old landing", "Faster"]);
        let named = unacknowledged(&all, &left_out, &["old".to_owned()]).unwrap();
        assert_eq!(named.len(), 1);
        assert!(
            unacknowledged(&all, &left_out, &["old".to_owned(), "dddd".to_owned()])
                .unwrap()
                .is_empty()
        );
        let included = unacknowledged(&all, &left_out, &["swap".to_owned()]).unwrap_err();
        assert!(
            included.contains("which this release includes"),
            "{included}"
        );
        let nothing = unacknowledged(&all, &left_out, &["zzz".to_owned()]).unwrap_err();
        assert!(nothing.starts_with("--leave-out"), "{nothing}");
        let message = unacknowledged_message(&silent);
        assert!(message.contains("2 user-facing landing(s)"), "{message}");
        assert!(
            message.contains("bbbbbbb \"Old landing\" (unclassified)"),
            "{message}"
        );
        assert!(message.contains("(speed): see Faster"), "{message}");
        assert!(message.contains("--leave-out"), "{message}");
    }

    #[test]
    fn replacements_resolve_the_earlier_landing_or_keep_the_reference() {
        let earlier = landing("e", "Thin caret");
        let mut by = kinded("f", "Blue caret", Kind::Look);
        by.supersedes = pins(&["eeeeeee — owner chose blue", "Nowhere — gone", "malformed"]);
        let found = replacements(&[&by], &[earlier.clone()]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].sha, earlier.sha);
        assert_eq!(found[0].name, "Thin caret");
        assert_eq!(found[0].reason, "owner chose blue");
        assert_eq!(found[0].by, "Blue caret");
        assert_eq!(found[1].sha, "Nowhere");
        assert_eq!(found[1].name, "");
    }

    #[test]
    fn docs_only_means_documentation_paths_only() {
        assert!(docs_only(&["docs/TODO.md", "notes.md"]));
        assert!(!docs_only(&["docs/TODO.md", "xtask/src/pins.rs"]));
        assert!(docs_only::<&str>(&[]));
        // Docs a suite reads are gated, and still documentation.
        assert!(docs_only(&[
            "docs/fixtures/pane-presentation-v1.json",
            "docs/PROTOCOL_FACTS.md",
            "docs/agent-map.json"
        ]));
    }

    #[test]
    fn user_facing_hides_only_internal_and_record() {
        assert!(user_facing(None));
        for kind in Kind::ALL {
            assert_eq!(
                user_facing(Some(kind)),
                !matches!(kind, Kind::Internal | Kind::Record)
            );
            assert_eq!(Kind::parse(&kind.as_str().to_uppercase()), Some(kind));
        }
    }

    #[test]
    fn conflict_message_names_the_landing_and_its_overlaps() {
        let text = conflict_message(&landing("a", "A"), &["B".to_owned()]);
        assert!(text.contains("\"A\""), "{text}");
        assert!(text.contains('B'), "{text}");
        assert!(text.contains("--change"), "{text}");
    }
}
