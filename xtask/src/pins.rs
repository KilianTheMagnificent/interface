//! Shipped pins: the tests a landing names as proof of its behavior.
//!
//! A landing's `Landing-pin: <path>::<test name>` trailers name tests. Once
//! the landing is on `main` — by ancestry or as a `-x` cherry-pick, both of
//! which keep the landing message — its pins are SHIPPED. A later push that
//! deletes a shipped pin's file, drops its name, or changes a line inside the
//! test must carry `Supersedes: <landing> — <reason>` naming the landing, so
//! undoing shipped behavior is something an agent says out loud and the next
//! release summary shows the owner. Promote checks the candidate tree again,
//! for anything that reached dev without closeout.
//!
//! The pure functions take strings and are unit tested without a repository;
//! the wrappers at the bottom run git.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
};

use anyhow::{Context, Result, bail};

use crate::{gate, landings};

/// One pin, split at the first `::`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pin {
    pub path: String,
    pub test: String,
}

/// Parses `<repo-relative path>::<test name>`. The test name is the Rust
/// function name or the exact `it`/`test` title, not a module path.
pub fn parse_pin(value: &str) -> Result<Pin, String> {
    let value = value.trim();
    let refusal = || {
        format!(
            "--pin \"{value}\" must be <repo-relative path>::<test name>, e.g. \
             xtask/src/pins.rs::parse_pin_splits_at_the_first_separator"
        )
    };
    let (path, test) = value.split_once("::").ok_or_else(refusal)?;
    let (path, test) = (path.trim(), test.trim());
    if path.is_empty()
        || test.is_empty()
        || path.starts_with('/')
        || path.split('/').any(|part| part == "..")
    {
        return Err(refusal());
    }
    Ok(Pin {
        path: path.to_owned(),
        test: test.to_owned(),
    })
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The 0-based line of every occurrence of the test `name` in `text`, one entry per
/// occurrence. Where the name starts or ends with an identifier character,
/// the neighbouring character must not be one, so `foo` does not match
/// `foo_bar`.
pub fn occurrences(text: &str, name: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    if name.is_empty() {
        return lines;
    }
    let first = name.chars().next().is_some_and(is_ident);
    let last = name.chars().next_back().is_some_and(is_ident);
    for (index, line) in text.lines().enumerate() {
        let mut from = 0;
        while let Some(at) = line[from..].find(name) {
            let start = from + at;
            let end = start + name.len();
            let before = line[..start].chars().next_back();
            let after = line[end..].chars().next();
            let glued =
                first && before.is_some_and(is_ident) || last && after.is_some_and(is_ident);
            if !glued {
                lines.push(index);
            }
            from = end;
        }
    }
    lines
}

/// Whether a pinned file's tests close with a brace (Rust, Swift) rather than
/// with the parenthesis of an `it(`/`test(` call (JavaScript, TypeScript).
pub fn closes_with_brace(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("rs") || extension.eq_ignore_ascii_case("swift")
    })
}

/// The change in bracket depth one line makes, ignoring string contents and
/// `//` comments: braces for Rust and Swift, parentheses for JavaScript/
/// TypeScript, where the test is an `it(`/`test(` call. `'` is a quote only in
/// parenthesis mode, since Rust lifetimes use it unpaired. There a `/` where a
/// value belongs opens a regex literal, whose quotes and brackets are not code
/// either.
fn line_depth(line: &str, brace: bool, depth: &mut i64, opened: &mut bool) {
    let (open, close) = if brace { ('{', '}') } else { ('(', ')') };
    let chars: Vec<char> = line.chars().collect();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    // Inside a regex's `[...]`, where `/` does not close it.
    let mut class = false;
    // The last character outside a string, space aside: what a `/` follows.
    let mut last: Option<char> = None;
    for (index, &c) in chars.iter().enumerate() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if q == '/' && (c == '[' || c == ']') {
                class = c == '[';
            } else if c == q && !class {
                quote = None;
                last = Some(c);
            }
            continue;
        }
        if c == '/' && chars.get(index + 1) == Some(&'/') {
            break;
        }
        if c == '"' || c == '`' || (!brace && c == '\'') {
            quote = Some(c);
        } else if !brace
            && c == '/'
            && last.is_none_or(|before| "(,=:[!&|?{};+-*%<>~^".contains(before))
        {
            quote = Some('/');
            class = false;
        } else if c == open {
            *depth += 1;
            *opened = true;
        } else if c == close {
            *depth -= 1;
        }
        if !c.is_whitespace() {
            last = Some(c);
        }
    }
}

/// The 0-based inclusive line span of the one test `name` names: from the
/// attribute and comment lines directly above its name line to the line that
/// closes it. `None` unless the name occurs exactly once. `brace` is
/// [`closes_with_brace`] for the pinned file.
pub fn test_span(text: &str, name: &str, brace: bool) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let [line] = occurrences(text, name)[..] else {
        return None;
    };
    let mut start = line;
    while start > 0 {
        let above = lines[start - 1].trim_start();
        if above.starts_with("#[") || above.starts_with("//") {
            start -= 1;
        } else {
            break;
        }
    }
    let end = layout_end(&lines, line, brace).unwrap_or_else(|| depth_end(&lines, line, brace));
    Some((start, end))
}

/// How a line at a test's own indentation can begin the next test.
const SIBLING_STARTS: [&str; 10] = [
    "it(",
    "it.",
    "test(",
    "test.",
    "describe(",
    "describe.",
    "#[",
    "fn ",
    "async fn ",
    "func ",
];

/// Where formatted code closes the test on `line`: that line itself when it
/// opens and closes the test there, else the first later closing line at its
/// indentation, or the line before a sibling test that starts first. Lines at
/// any other indentation are body or string contents. Only the name line is
/// lexed, so a quote in a regex, a template literal or a multi-line string in
/// the body cannot move the end.
fn layout_end(lines: &[&str], line: usize, brace: bool) -> Option<usize> {
    let (mut depth, mut opened) = (0, false);
    line_depth(lines[line], brace, &mut depth, &mut opened);
    if opened && depth <= 0 {
        return Some(line);
    }
    let indent = indentation(lines[line]);
    let mut last = line;
    for (index, content) in lines.iter().enumerate().skip(line + 1) {
        let code = content.trim();
        if code.is_empty() {
            continue;
        }
        if indentation(content) == indent {
            if closes_test(code, brace) {
                return Some(index);
            }
            if SIBLING_STARTS.iter().any(|start| code.starts_with(start)) {
                return Some(last);
            }
        }
        last = index;
    }
    None
}

/// Whether a trimmed line at the test's indentation is its closing line: a
/// lone `}` in Rust and Swift; in JavaScript a `}` that ends the `it(`/`test(`
/// call (`});`, `}, 10_000);`), not one that opens the next block
/// (`}, () => {`).
fn closes_test(code: &str, brace: bool) -> bool {
    if brace {
        code == "}"
    } else {
        code.starts_with('}') && code.trim_end_matches(';').ends_with(')')
    }
}

/// The line where the bracket count from `line` returns to zero, or the last
/// line: the fallback for a test with no closing line where formatting puts
/// one.
fn depth_end(lines: &[&str], line: usize, brace: bool) -> usize {
    let mut depth = 0;
    let mut opened = false;
    for (index, content) in lines.iter().enumerate().skip(line) {
        line_depth(content, brace, &mut depth, &mut opened);
        if opened && depth <= 0 {
            return index;
        }
    }
    lines.len().saturating_sub(1)
}

/// The width of a line's leading whitespace, in bytes.
fn indentation(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Whether a `--unified=0` diff of one file changes a line inside `span`
/// (0-based, of the old file): a hunk whose old lines intersect it, or an
/// insertion after a span line other than its last. Text appended after a
/// test's closing line is a new neighbour, not a change to the test.
pub fn diff_touches(diff: &str, span: (usize, usize)) -> bool {
    let (first, last) = (span.0 + 1, span.1 + 1);
    diff.lines()
        .filter_map(|line| line.strip_prefix("@@ -"))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter_map(|old| {
            let (start, count) = old.split_once(',').unwrap_or((old, "1"));
            Some((start.parse::<usize>().ok()?, count.parse::<usize>().ok()?))
        })
        .any(|(start, count)| {
            if count == 0 {
                start >= first && start < last
            } else {
                start <= last && start + count > first
            }
        })
}

/// One `Supersedes:` trailer value: `<sha prefix or exact name> — <reason>`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Supersede {
    pub reference: String,
    pub reason: String,
}

/// Splits a `Supersedes:` value at its first ` — `, ` – `, ` -- ` or ` - `.
/// `None` when the reference or the reason is missing.
pub fn parse_supersede(value: &str) -> Option<Supersede> {
    let value = value.trim();
    [" — ", " – ", " -- ", " - "].iter().find_map(|separator| {
        let (reference, reason) = value.split_once(separator)?;
        let (reference, reason) = (reference.trim(), reason.trim());
        (!reference.is_empty() && !reason.is_empty()).then(|| Supersede {
            reference: reference.to_owned(),
            reason: reason.to_owned(),
        })
    })
}

impl Supersede {
    /// Whether this names a landing known by `shas` and `name`: a hex prefix
    /// of at least seven digits of one SHA, or the whole name, ignoring case.
    pub fn names(&self, shas: &[String], name: &str) -> bool {
        let reference = self.reference.to_lowercase();
        let hex = reference.len() >= 7 && reference.chars().all(|c| c.is_ascii_hexdigit());
        (hex && shas.iter().any(|sha| sha.starts_with(&reference)))
            || reference == name.trim().to_lowercase()
    }
}

/// A pin some landing on `main` carries. `shas` are the commits that carry it
/// — a landing and its cherry-pick, and the SHA a cherry-pick came from — and
/// `name` is the landing's name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShippedPin {
    pub pin: String,
    pub shas: Vec<String>,
    pub name: String,
}

impl ShippedPin {
    fn superseded(&self, supersedes: &[Supersede]) -> bool {
        supersedes
            .iter()
            .any(|supersede| supersede.names(&self.shas, &self.name))
    }
}

/// The `git log --format` [`parse_pin_log`] reads: SHA, subject, the pin
/// trailer values and the body, whose `(cherry picked from commit …)` line
/// names the landing a cherry-pick copies.
pub const PIN_LOG_FORMAT: &str = "%H%x1f%s%x1f%(trailers:key=Landing-pin,valueonly)%x1f%b%x1e";

/// Every pin in a [`PIN_LOG_FORMAT`] log, one entry per pin and landing name,
/// merging a landing with its cherry-picks.
pub fn parse_pin_log(log: &str) -> Vec<ShippedPin> {
    let mut pins: Vec<ShippedPin> = Vec::new();
    for record in log.split('\x1e') {
        let mut fields = record.trim_matches('\n').splitn(4, '\x1f');
        let (Some(sha), Some(name), Some(values)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (sha, name) = (sha.trim(), name.trim());
        if sha.is_empty() {
            continue;
        }
        let mut shas = vec![sha.to_owned()];
        shas.extend(landings::shipped_shas(fields.next().unwrap_or("")));
        for pin in values.lines().map(str::trim).filter(|pin| !pin.is_empty()) {
            if let Some(existing) = pins.iter_mut().find(|p| p.pin == pin && p.name == name) {
                for sha in &shas {
                    if !existing.shas.contains(sha) {
                        existing.shas.push(sha.clone());
                    }
                }
            } else {
                pins.push(ShippedPin {
                    pin: pin.to_owned(),
                    shas: shas.clone(),
                    name: name.to_owned(),
                });
            }
        }
    }
    pins
}

/// Why a pin does not name exactly one test at some revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Missing {
    File,
    Name,
    Duplicate,
}

impl Missing {
    /// `after` is the pinned file at the revision checked.
    fn of(after: Option<&str>, test: &str) -> Option<Self> {
        match after.map(|text| occurrences(text, test).len()) {
            None => Some(Self::File),
            Some(1) => None,
            Some(0) => Some(Self::Name),
            Some(_) => Some(Self::Duplicate),
        }
    }

    /// For a shipped pin: what happened to it.
    fn lost(self) -> &'static str {
        match self {
            Self::File => "its file is gone (deleted or moved)",
            Self::Name => "the test name is gone from its file",
            Self::Duplicate => "the test name now occurs more than once",
        }
    }

    /// For a new pin: what is wrong with it.
    fn invalid(self) -> &'static str {
        match self {
            Self::File => "no such file in this tree",
            Self::Name => "the test name does not occur in the file",
            Self::Duplicate => "the test name occurs more than once in the file",
        }
    }
}

/// The refusal text for pins in trouble: each pin, why, and its landing, then
/// the trailer that clears each landing.
fn trouble_message(intro: &str, trouble: &[(&ShippedPin, &str)], remedy: bool) -> String {
    let mut text = format!("{intro}\n");
    let mut landings: Vec<(&str, &str)> = Vec::new();
    for (pin, why) in trouble {
        let sha = gate::short(&pin.shas[0]);
        let _ = writeln!(
            text,
            "    {} — {why}\n        pinned by {sha} \"{}\"",
            pin.pin, pin.name
        );
        if !landings.iter().any(|(s, _)| *s == sha) {
            landings.push((sha, &pin.name));
        }
    }
    if remedy {
        text.push_str(
            "If this behavior is meant to change, add to a commit in the push, one per landing:\n",
        );
        for (sha, _) in &landings {
            let _ = writeln!(
                text,
                "    {} {sha} — <why the earlier behavior is replaced>",
                landings::SUPERSEDES_TRAILER
            );
        }
        text.push_str("Otherwise restore the test.");
    }
    text
}

// ---- git wrappers ----------------------------------------------------------

/// The file at `rev:path`, or `None` when that revision has no such file.
fn show(root: &Path, rev: &str, path: &str) -> Result<Option<String>> {
    let output = gate::git_command(
        root,
        &["show", &format!("{rev}:{path}")],
        gate::GitStderr::Null,
    )
    .output()
    .context("failed to launch git")?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Every pin carried by a commit `rev` contains.
pub fn shipped_pins(root: &Path, rev: &str) -> Result<Vec<ShippedPin>> {
    let log = gate::git_capture(
        root,
        &[
            "log",
            "--fixed-strings",
            &format!("--grep={}", landings::PIN_TRAILER),
            &format!("--format={PIN_LOG_FORMAT}"),
            rev,
        ],
    )?;
    Ok(parse_pin_log(&log))
}

/// Every well-formed `Supersedes:` trailer on a commit in `range` (a revision
/// or `a..b`).
pub fn supersedes_in(root: &Path, range: &str) -> Result<Vec<Supersede>> {
    Ok(gate::git_capture(
        root,
        &[
            "log",
            "--fixed-strings",
            &format!("--grep={}", landings::SUPERSEDES_TRAILER),
            "--format=%(trailers:key=Supersedes,valueonly)",
            range,
        ],
    )?
    .lines()
    .filter_map(parse_supersede)
    .collect())
}

/// Closeout's check of a new landing's `--pin` values against its own tree:
/// each file exists at `rev` and names its test exactly once.
pub fn validate_new(root: &Path, rev: &str, pins: &[String]) -> Result<()> {
    let mut problems = String::new();
    for value in pins {
        let pin = parse_pin(value).map_err(anyhow::Error::msg)?;
        let after = show(root, rev, &pin.path)?;
        if let Some(missing) = Missing::of(after.as_deref(), &pin.test) {
            let _ = writeln!(problems, "    {} — {}", value.trim(), missing.invalid());
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    bail!(
        "closeout refused: a --pin must name one test in this tree, by its function name or \
         exact title:\n{problems}Name a more specific test or fix the path."
    )
}

/// The `Supersedes:` trailers the branch's own commits carry (`dev..HEAD`),
/// each required to parse and to name a landing `HEAD` contains, so a typo
/// cannot slip through as a declaration.
pub fn branch_supersedes(root: &Path, dev: &str) -> Result<Vec<String>> {
    let values: Vec<String> = gate::git_capture(
        root,
        &[
            "log",
            "--format=%(trailers:key=Supersedes,valueonly)",
            &format!("{dev}..HEAD"),
        ],
    )?
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .map(str::to_owned)
    .collect();
    if values.is_empty() {
        return Ok(values);
    }
    let known = landings::known_landings(root, "HEAD")?;
    let mut unique: Vec<String> = Vec::new();
    for value in values {
        let Some(supersede) = parse_supersede(&value) else {
            bail!(
                "closeout refused: \"{} {value}\" needs a landing and a reason: \
                 `{} <sha or exact name> — <reason>`",
                landings::SUPERSEDES_TRAILER,
                landings::SUPERSEDES_TRAILER
            );
        };
        if !known
            .iter()
            .any(|landing| supersede.names(std::slice::from_ref(&landing.sha), &landing.name))
        {
            bail!(
                "closeout refused: \"{} {value}\" names no landing; use the landing's SHA (7+ \
                 hex digits) or its exact name",
                landings::SUPERSEDES_TRAILER
            );
        }
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    Ok(unique)
}

/// Files at one revision, each read at most once however many pins name it.
struct Files<'a> {
    root: &'a Path,
    rev: &'a str,
    read: BTreeMap<String, Option<String>>,
}

impl<'a> Files<'a> {
    const fn new(root: &'a Path, rev: &'a str) -> Self {
        Self {
            root,
            rev,
            read: BTreeMap::new(),
        }
    }

    /// The file at this revision, or `None` when the revision has no such file.
    fn get(&mut self, path: &str) -> Result<Option<&str>> {
        if !self.read.contains_key(path) {
            let text = show(self.root, self.rev, path)?;
            self.read.insert(path.to_owned(), text);
        }
        Ok(self.read.get(path).and_then(Option::as_deref))
    }
}

/// The paths that differ between `dev` and `HEAD`, or `None` when git cannot
/// compare them, which reads every pin. Renames are listed as the deletion and
/// the addition they are, so a moved pinned file still reads as gone.
fn changed_between(root: &Path, dev: &str) -> Result<Option<BTreeSet<String>>> {
    let output = gate::git(
        root,
        &[
            "diff",
            "--no-color",
            "--no-renames",
            "--name-only",
            "-z",
            dev,
            "HEAD",
        ],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        output
            .stdout
            .split(|&byte| byte == 0)
            .filter(|name| !name.is_empty())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect(),
    ))
}

/// Whether `path` is spelled the way `git diff --name-only` lists paths, so
/// its absence from that list proves the file unchanged. A pin spelled any
/// other way (`./a.rs`, `a//b.rs`, a non-ASCII name git may normalize) is read.
fn listed_spelling(path: &str) -> bool {
    path.is_ascii() && path.split('/').all(|part| !part.is_empty() && part != ".")
}

/// Closeout's guard: refuses a push (`dev..HEAD`) that deletes, drops or
/// changes a shipped pin no `Supersedes:` reachable from `HEAD` releases. A
/// pin already broken on `dev` is not this push's doing and is left to the
/// promote check. A file the push leaves byte-identical cannot lose or change
/// a test, so only pins in files `dev..HEAD` changes are read, each file once.
pub fn guard(root: &Path, dev: &str) -> Result<()> {
    let Ok(main) = gate::rev_parse(root, "origin/main") else {
        return Ok(());
    };
    let shipped = shipped_pins(root, &main)?;
    if shipped.is_empty() {
        return Ok(());
    }
    let supersedes = supersedes_in(root, "HEAD")?;
    let changed = changed_between(root, dev)?;
    let (mut before_files, mut after_files) = (Files::new(root, dev), Files::new(root, "HEAD"));
    let mut diffs: BTreeMap<String, String> = BTreeMap::new();
    let mut trouble = Vec::new();
    for pin in shipped.iter().filter(|pin| !pin.superseded(&supersedes)) {
        let Ok(parsed) = parse_pin(&pin.pin) else {
            continue;
        };
        if changed
            .as_ref()
            .is_some_and(|changed| listed_spelling(&parsed.path) && !changed.contains(&parsed.path))
        {
            continue;
        }
        let Some(before) = before_files.get(&parsed.path)? else {
            continue;
        };
        let Some(span) = test_span(before, &parsed.test, closes_with_brace(&parsed.path)) else {
            continue;
        };
        let after = after_files.get(&parsed.path)?;
        let why = Missing::of(after, &parsed.test)
            .map(Missing::lost)
            .or_else(|| {
                let diff = diffs.entry(parsed.path.clone()).or_insert_with(|| {
                    gate::git_capture(
                        root,
                        &[
                            "diff",
                            "--no-color",
                            "--no-ext-diff",
                            "--unified=0",
                            dev,
                            "HEAD",
                            "--",
                            &parsed.path,
                        ],
                    )
                    .unwrap_or_default()
                });
                diff_touches(diff, span).then_some("the test changed")
            });
        if let Some(why) = why {
            trouble.push((pin, why));
        }
    }
    if trouble.is_empty() {
        return Ok(());
    }
    bail!(
        "closeout refused: {}",
        trouble_message(
            "this push changes tests that shipped landings pin as proof of their behavior:",
            &trouble,
            true
        )
    )
}

/// Promote's check of a release candidate: every pin `main` carries and every
/// pin of an included landing, less those superseded in the candidate's
/// history, still names exactly one test at `candidate`.
pub fn verify_release(
    root: &Path,
    candidate: &str,
    main: &str,
    included: &[landings::Entry],
) -> Result<()> {
    let mut pins = shipped_pins(root, main)?;
    for entry in included {
        for pin in &entry.pins {
            pins.push(ShippedPin {
                pin: pin.clone(),
                shas: vec![entry.sha.clone()],
                name: entry.name.clone(),
            });
        }
    }
    if pins.is_empty() {
        return Ok(());
    }
    let supersedes = supersedes_in(root, candidate)?;
    let mut files = Files::new(root, candidate);
    let mut trouble = Vec::new();
    for pin in pins.iter().filter(|pin| !pin.superseded(&supersedes)) {
        let Ok(parsed) = parse_pin(&pin.pin) else {
            continue;
        };
        if let Some(missing) = Missing::of(files.get(&parsed.path)?, &parsed.test) {
            trouble.push((pin, missing.lost()));
        }
    }
    if trouble.is_empty() {
        return Ok(());
    }
    bail!(
        "promote refused: {}The candidate is missing proof of shipped or included behavior; \
         land a fix (or a Supersedes: trailer) on dev and promote again.",
        trouble_message("the release candidate lost pinned tests:", &trouble, false)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{git_in as git, init_repo};

    #[test]
    fn parse_pin_splits_at_the_first_separator() {
        assert_eq!(
            parse_pin(" xtask/src/pins.rs::a test :: with colons "),
            Ok(Pin {
                path: "xtask/src/pins.rs".to_owned(),
                test: "a test :: with colons".to_owned(),
            })
        );
        for bad in [
            "no-separator",
            "::name",
            "path::",
            "/abs/x.rs::t",
            "a/../b.rs::t",
        ] {
            let refusal = parse_pin(bad).unwrap_err();
            assert!(
                refusal.contains("<repo-relative path>::<test name>"),
                "{refusal}"
            );
        }
    }

    #[test]
    fn occurrences_respect_identifier_boundaries() {
        let text = "fn foo() {}\nfn foo_bar() {}\n// foo\nlet x = afoo;\n";
        assert_eq!(occurrences(text, "foo"), vec![0, 2]);
        assert_eq!(occurrences(text, "foo_bar"), vec![1]);
        // A title with spaces is bounded by its quotes.
        let ts = "it(\"shows swap\", () => {});\nit(\"shows swap preview\", () => {});\n";
        assert_eq!(occurrences(ts, "shows swap preview"), vec![1]);
        assert_eq!(occurrences(ts, "shows swap").len(), 2);
        assert!(occurrences(text, "").is_empty());
    }

    const RUST: &str = "\
use x;

#[test]
fn first() {
    let s = \"}\"; // a brace in a string and a } in a comment
    assert!(true);
}

#[tokio::test]
#[ignore]
async fn second<'a>() {
    let r: &'a str = \"x\";
}
";

    #[test]
    fn rust_spans_start_at_attributes_and_end_at_the_closing_brace() {
        assert_eq!(test_span(RUST, "first", true), Some((2, 6)));
        assert_eq!(test_span(RUST, "second", true), Some((8, 12)));
        assert_eq!(test_span(RUST, "missing", true), None);
    }

    #[test]
    fn typescript_spans_follow_the_call_parentheses() {
        let ts = "\
describe(\"grid\", () => {
  it(\"one liner (with parens)\", () => expect(1).toBe(1));
  // pinned below
  it('shows the swap preview', () => {
    const s = \")\";
    expect(s).toBe(\")\");
  });
  it(\"next\", () => {});
});
";
        assert_eq!(
            test_span(ts, "one liner (with parens)", false),
            Some((1, 1))
        );
        assert_eq!(test_span(ts, "shows the swap preview", false), Some((2, 6)));
        // A regex literal's quote, bracket and escaped slash are not code;
        // a division is not a regex.
        let regex = "\
it(\"matches markup\", () => {
  expect(html).toMatch(/icon\"[^>/]*><\\/span>(/u);
  expect(half(4) / 2).toBe(1);
});
it(\"after\", () => {});
";
        assert_eq!(test_span(regex, "matches markup", false), Some((0, 3)));
        // An unclosed test runs to the end of the file.
        assert_eq!(
            test_span("it(\"open\", () => {\n", "open", false),
            Some((0, 0))
        );
    }

    #[test]
    fn spans_end_where_formatting_closes_the_test_whatever_the_body_holds() {
        // A regex after a keyword reads as a division, and a template
        // literal's lines as code: neither may move the end.
        let ts = "\
test(\"reads markup\", () => {
  const closes = (html: string) => { return /\\)\"/u.test(html); };
  const probe = `(async () => {
    await run(\"x\");
})`;
  expect(closes(probe)).toBe(true);
});
test(\"after\", () => {});
";
        assert_eq!(test_span(ts, "reads markup", false), Some((0, 6)));
        // A `}` that opens the next block of the call is not its end.
        let options = "\
test(\"stock is unsafe\", {
  skip: false,
}, () => {
  assert.ok(true);
});
";
        assert_eq!(test_span(options, "stock is unsafe", false), Some((0, 4)));
        // A close off the test's own indentation ends before the sibling.
        let misaligned = "\
describe(\"d\", () => {
  it(\"treats a cancelled discovery as unknown\",
    async () => {
      expect(await load()).toBeNull();
    });

  it(\"asks once\", async () => {
  });
});
";
        assert_eq!(
            test_span(misaligned, "treats a cancelled discovery as unknown", false),
            Some((1, 4))
        );
        // A script embedded at the test's own indentation is contents.
        let script = "\
#[tokio::test]
async fn gated() {
    let script = format!(r#\"
import json
result = {{\"a\": 1}}
}}
\"#);
    assert!(script.contains('a'));
}
";
        assert_eq!(test_span(script, "gated", true), Some((0, 8)));
    }

    #[test]
    fn swift_tests_close_with_a_brace() {
        assert!(closes_with_brace(
            "apps/interface-ios/MacAddressTests.swift"
        ));
        assert!(closes_with_brace("xtask/src/pins.rs"));
        assert!(!closes_with_brace("electron/src/a.test.ts"));
        let swift = "\
final class T: XCTestCase {
    /// The parentheses of `async throws` are not the test's end.
    func testAMissingIdentity() async throws {
        let origin = URL(string: \"https://x-\\(UUID()).example/\")!
        do {
            _ = try await lookup(origin)
        } catch PhoneError.identityMissing { /* expected */ }
    }

    func testNext() {}
}
";
        let brace = closes_with_brace("MacAddressTests.swift");
        assert_eq!(
            test_span(swift, "testAMissingIdentity", brace),
            Some((1, 7))
        );
        assert_eq!(test_span(swift, "testNext", brace), Some((9, 9)));
    }

    #[test]
    fn diff_touches_changes_and_inner_insertions_but_not_appended_neighbours() {
        let span = (2, 6); // old lines 3..=7
        assert!(diff_touches("@@ -5 +5 @@\n-a\n+b\n", span));
        assert!(diff_touches("@@ -1,3 +1,0 @@\n", span));
        assert!(diff_touches("@@ -3,0 +4 @@\n+#[ignore]\n", span));
        assert!(!diff_touches("@@ -7,0 +8,4 @@\n+\n+#[test]\n", span));
        assert!(!diff_touches("@@ -8,2 +8,2 @@\n", span));
        assert!(!diff_touches("@@ -1,2 +1,2 @@\n", span));
        assert!(!diff_touches("", span));
    }

    #[test]
    fn parse_supersede_needs_a_reference_and_a_reason() {
        assert_eq!(
            parse_supersede("2ae6e08 — owner chose the blue caret"),
            Some(Supersede {
                reference: "2ae6e08".to_owned(),
                reason: "owner chose the blue caret".to_owned(),
            })
        );
        assert_eq!(
            parse_supersede("Thin caret default - replaced by blue").map(|s| s.reference),
            Some("Thin caret default".to_owned())
        );
        assert_eq!(parse_supersede("2ae6e08"), None);
        assert_eq!(parse_supersede("2ae6e08 — "), None);
    }

    #[test]
    fn supersede_names_a_landing_by_sha_prefix_or_exact_name() {
        let shas = vec!["2ae6e08f".repeat(5)];
        let by_sha = parse_supersede("2AE6E08 — r").unwrap();
        assert!(by_sha.names(&shas, "Blue caret"));
        let short = parse_supersede("2ae6e0 — r").unwrap();
        assert!(!short.names(&shas, "Blue caret"), "six digits are too few");
        let by_name = parse_supersede("blue CARET — r").unwrap();
        assert!(by_name.names(&[], "Blue caret"));
        let partial = parse_supersede("Blue — r").unwrap();
        assert!(!partial.names(&[], "Blue caret"), "a name must match whole");
    }

    #[test]
    fn parse_pin_log_merges_a_landing_with_its_cherry_pick() {
        let landing = "a".repeat(40);
        let pick = "b".repeat(40);
        let log = format!(
            "{pick}\x1fSwap preview\x1fx.ts::shows swap\n\x1fbody\n(cherry picked from commit {landing})\n\x1e\
             {landing}\x1fSwap preview\x1fx.ts::shows swap\ny.rs::drags\n\x1fbody\n\x1e\
             {}\x1fNo pins\x1f\x1f\x1e",
            "c".repeat(40)
        );
        let pins = parse_pin_log(&log);
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].pin, "x.ts::shows swap");
        assert_eq!(pins[0].shas, vec![pick.clone(), landing.clone()]);
        assert_eq!(pins[1].pin, "y.rs::drags");
        assert_eq!(pins[1].shas, vec![landing]);
        let cleared = ShippedPin {
            pin: "p".into(),
            shas: vec![pick],
            name: "Swap preview".into(),
        };
        assert!(cleared.superseded(&[parse_supersede("swap preview — r").unwrap()]));
        assert!(!cleared.superseded(&[parse_supersede("other — r").unwrap()]));
    }

    #[test]
    fn listed_spelling_is_only_what_git_diff_prints() {
        assert!(listed_spelling("xtask/src/pins.rs"));
        assert!(listed_spelling("a b/c.test.ts"));
        for other in ["./t.rs", "a//b.rs", "a/./b.rs", "a/", "caf\u{e9}.rs"] {
            assert!(!listed_spelling(other), "{other}");
        }
    }

    #[test]
    fn guard_reads_only_pins_in_files_the_push_changes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        init_repo(root);
        let test = |name: &str| format!("#[test]\nfn {name}() {{\n    assert!(true);\n}}\n");
        std::fs::write(root.join("t.rs"), test("proves_a")).expect("t.rs");
        std::fs::write(root.join("u.rs"), test("proves_b")).expect("u.rs");
        std::fs::write(root.join("other.rs"), "fn other() {}\n").expect("other.rs");
        git(root, &["add", "."]);
        // `./u.rs` names the same file git lists as `u.rs`, so it is read.
        let shipped = "Ship A\n\nLanding-pin: t.rs::proves_a\nLanding-pin: ./u.rs::proves_b";
        git(root, &["commit", "--quiet", "-m", shipped]);
        git(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let dev = git(root, &["rev-parse", "HEAD"]);

        std::fs::write(root.join("other.rs"), "fn other() { 1; }\n").expect("other.rs");
        git(root, &["commit", "--quiet", "-am", "touch other"]);
        guard(root, &dev).expect("a push that leaves pinned files alone passes");

        git(root, &["reset", "--quiet", "--hard", &dev]);
        git(root, &["mv", "t.rs", "moved.rs"]);
        git(root, &["commit", "--quiet", "-m", "move t"]);
        let moved = guard(root, &dev).unwrap_err().to_string();
        assert!(
            moved.contains("t.rs::proves_a — its file is gone"),
            "a rename is a deletion: {moved}"
        );

        git(root, &["reset", "--quiet", "--hard", &dev]);
        std::fs::write(
            root.join("u.rs"),
            test("proves_b").replace("true", "1 == 1"),
        )
        .expect("u.rs");
        git(root, &["commit", "--quiet", "-am", "reword b"]);
        let changed = guard(root, &dev).unwrap_err().to_string();
        assert!(
            changed.contains("./u.rs::proves_b — the test changed"),
            "{changed}"
        );
        assert!(!changed.contains("t.rs::proves_a"), "{changed}");
    }

    #[test]
    fn verify_release_refuses_a_candidate_that_lost_a_pin_unless_superseded() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        init_repo(root);
        std::fs::write(root.join("t.rs"), "#[test]\nfn proves_a() {}\n").expect("t.rs");
        git(root, &["add", "t.rs"]);
        let shipped =
            "Ship A\n\nLanding-kind: fix\nLanding-pin: t.rs::proves_a\nLanded-from: fix/a";
        git(root, &["commit", "--quiet", "-m", shipped]);
        let main = git(root, &["rev-parse", "HEAD"]);
        assert!(verify_release(root, &main, &main, &[]).is_ok());

        // An included landing whose pin is not in the candidate: the shape of a
        // selective release that left out the landing adding its test.
        let included = landings::Entry {
            sha: "e".repeat(40),
            name: "Needs B".to_owned(),
            kind: Some(landings::Kind::Feature),
            check: None,
            pins: vec!["t.rs::proves_b".to_owned()],
        };
        let refusal = verify_release(root, &main, &main, &[included])
            .unwrap_err()
            .to_string();
        assert!(
            refusal.contains("t.rs::proves_b — the test name is gone"),
            "{refusal}"
        );
        assert!(refusal.contains("\"Needs B\""), "{refusal}");

        git(root, &["rm", "--quiet", "t.rs"]);
        git(root, &["commit", "--quiet", "-m", "drop the test"]);
        let lost = git(root, &["rev-parse", "HEAD"]);
        let refusal = verify_release(root, &lost, &main, &[])
            .unwrap_err()
            .to_string();
        assert!(
            refusal.contains("t.rs::proves_a — its file is gone"),
            "{refusal}"
        );
        assert!(refusal.contains(gate::short(&main)), "{refusal}");

        let declared = format!(
            "Retire A\n\nSupersedes: {} — A is retired",
            gate::short(&main)
        );
        git(
            root,
            &["commit", "--quiet", "--allow-empty", "-m", &declared],
        );
        let superseded = git(root, &["rev-parse", "HEAD"]);
        assert!(verify_release(root, &superseded, &main, &[]).is_ok());
    }
}
