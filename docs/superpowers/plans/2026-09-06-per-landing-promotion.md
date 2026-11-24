# Per-landing promotion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every closeout lands on `dev` as one named merge commit, `cargo xtask landings` lists the ones `main` lacks, and `cargo xtask promote --change …` ships only the chosen ones to `main` while `dev` keeps everything.

**Architecture:** `closeout` writes the landing with `git commit-tree` (tree = the branch's tree, first parent = `origin/dev`, second parent = the branch tip) so `git log --first-parent --merges origin/main..origin/dev` is the menu. A new `xtask/src/landings.rs` holds the pure logic (naming rule, message, log parsing, shipped-set filter, overlap, resolution) plus thin git wrappers. `promote` gains a selective mode that cherry-picks landings with `-x -m 1` onto a head detached from `origin/main`, then reuses today's bump, gate, package and staging, pushes `main`, and merges `main` back into `dev`.

**Tech Stack:** Rust 2024 xtask crate (`anyhow`, `clap` derive, `serde_json`, `tempfile` already in `xtask/Cargo.toml`), git plumbing via `std::process::Command`. Tests: `cargo test -p xtask`.

**Spec:** `docs/superpowers/specs/2026-09-06-per-landing-promotion-design.md`

## Global Constraints

- The `--change` text is trimmed, one line, non-empty, at most 100 characters.
- Landing trailer is exactly `Landed-from: <branch>`; release merge-back trailer is exactly `Release: <version>` and carries no `Landed-from:`.
- A shipped landing is one whose full SHA appears in a `(cherry picked from commit <sha>)` line anywhere on `origin/main`.
- `dev` and `main` are only ever pushed fast-forward. Nothing rewrites either.
- Ship-all `promote` (no `--change`) keeps every existing precondition, resume rule and stamp skip; do not change its behaviour.
- Refusals use the existing `closeout refused: …` / `promote refused: …` style and say what to do next. Every failure after `main` is pushed prints the exact commands that finish the release.
- Commit messages end with the two trailers this session uses:
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01JwXuHMw85hTFaDncu7kCes`.
- Run `cargo fmt --all` before each commit; the gate runs `cargo fmt --all -- --check` and strict Clippy (`-D warnings`, workspace lints). Doc comments on every `pub` item.

---

## File structure

- Create `xtask/src/landings.rs` — pure functions and git wrappers for landings: `landing_name`, `landing_message`, `Landing`, `LANDING_LOG_FORMAT`, `parse_landings`, `shipped_shas`, `unshipped`, `overlaps`, `resolve`, `render`, `conflict_message`, `unshipped_landings`, `landing_paths`, `landings_command`.
- Modify `xtask/src/closeout.rs` — `closeout(root, change)` builds and pushes the landing instead of the branch tip; `git_output` becomes `pub(crate)`.
- Modify `xtask/src/promote.rs` — `bump` takes its body; new `promote_landings(...)` selective mode.
- Modify `xtask/src/main.rs` — `mod landings;`, `Landings` subcommand, `Closeout { change }`, `Promote { changes }`.
- Modify `AGENTS.md` (Closeout section) and `docs/areas/xtask.md`.

---

### Task 1: Landing name and message (pure)

**Files:**
- Create: `xtask/src/landings.rs`
- Modify: `xtask/src/main.rs` (add `mod landings;` next to `mod closeout;`)

**Interfaces:**
- Produces:
  - `pub const LANDED_FROM_TRAILER: &str = "Landed-from:"`
  - `pub const RELEASE_TRAILER: &str = "Release:"`
  - `pub const MAX_CHANGE_CHARS: usize = 100`
  - `pub fn landing_name(change: Option<&str>, subjects: &[String]) -> Result<String, String>`
  - `pub fn landing_message(name: &str, subjects: &[String], branch: &str) -> String`

- [ ] **Step 1: Create the module with failing tests**

Create `xtask/src/landings.rs`:

```rust
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

/// The trailer every landing carries; commits without it are not landings.
pub const LANDED_FROM_TRAILER: &str = "Landed-from:";
/// The trailer a release merge-back carries instead, so the menu skips it.
pub const RELEASE_TRAILER: &str = "Release:";
/// The longest `--change` text closeout accepts.
pub const MAX_CHANGE_CHARS: usize = 100;

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

/// The landing commit message: the name, the subjects landed, the trailer.
pub fn landing_message(name: &str, subjects: &[String], branch: &str) -> String {
    let mut message = format!("{name}\n\n");
    if !subjects.is_empty() {
        for subject in subjects {
            message.push_str(subject.trim());
            message.push('\n');
        }
        message.push('\n');
    }
    message.push_str(&format!("{LANDED_FROM_TRAILER} {branch}\n"));
    message
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
            landing_name(Some("  Saved workspaces file under their own project "), &[]),
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
        assert!(landing_name(Some("   "), &[]).unwrap_err().contains("empty"));
        assert!(landing_name(Some("one\ntwo"), &[]).unwrap_err().contains("one line"));
        let long = "x".repeat(MAX_CHANGE_CHARS + 1);
        let refusal = landing_name(Some(&long), &[]).unwrap_err();
        assert!(refusal.contains("101 characters"), "{refusal}");
        assert!(landing_name(Some(&"y".repeat(MAX_CHANGE_CHARS)), &[]).is_ok());
    }

    #[test]
    fn landing_message_lists_subjects_and_ends_with_the_trailer() {
        assert_eq!(
            landing_message("Name", &subjects(&["first", "second"]), "fix/x"),
            "Name\n\nfirst\nsecond\n\nLanded-from: fix/x\n"
        );
        assert_eq!(
            landing_message("Name", &[], "fix/x"),
            "Name\n\nLanded-from: fix/x\n"
        );
    }
}
```

- [ ] **Step 2: Register the module and run the tests to see them fail to compile**

In `xtask/src/main.rs` add `mod landings;` after `mod gate;`.

Run: `cargo test -p xtask landings 2>&1 | tail -20`
Expected: compiles and the three tests PASS (the module is self-contained; if anything fails, fix it before moving on). rustc flags the unused module items until later tasks use them, so add `#[allow(dead_code)]` on the `mod landings;` line for now; Task 5 removes it once every item has a caller.

- [ ] **Step 3: Run clippy and fmt**

Run: `cargo fmt --all && cargo clippy --locked -p xtask --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add xtask/src/landings.rs xtask/src/main.rs
git commit -m "feat(xtask): landing name rule and message" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01JwXuHMw85hTFaDncu7kCes"
```

---

### Task 2: Parsing landings, the shipped filter, overlap, resolution (pure)

**Files:**
- Modify: `xtask/src/landings.rs`

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, Eq, PartialEq)] pub struct Landing { pub sha: String, pub date: String, pub name: String, pub branch: String }`
  - `pub const LANDING_LOG_FORMAT: &str` — the `--format` string for `git log`
  - `pub fn parse_landings(log: &str) -> Vec<Landing>` — oldest first
  - `pub fn shipped_shas(main_log: &str) -> BTreeSet<String>`
  - `pub fn unshipped(landings: Vec<Landing>, shipped: &BTreeSet<String>) -> Vec<Landing>`
  - `pub fn overlaps(paths: &[(Landing, Vec<String>)]) -> BTreeMap<String, Vec<String>>` — sha → names of other landings sharing a path
  - `pub fn resolve<'a>(landings: &'a [Landing], wanted: &str) -> Result<&'a Landing, String>`
  - `pub fn render(landings: &[Landing], overlaps: &BTreeMap<String, Vec<String>>) -> String`
  - `pub fn conflict_message(landing: &Landing, overlapping: &[String]) -> String`

- [ ] **Step 1: Add the failing tests**

Append inside `mod tests` in `xtask/src/landings.rs`:

```rust
    const LOG: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\x1f2026-09-07\x1fSecond landing\x1ffeat/two\n\x1e\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x1f2026-09-06\x1fFirst landing\x1ffix/one\n\x1e\
9999999999999999999999999999999999999999\x1f2026-09-05\x1fRelease 0.3.66 merged back to dev\x1f\n\x1e";

    fn landing(sha: &str, name: &str) -> Landing {
        Landing {
            sha: sha.repeat(40),
            date: "2026-09-06".to_owned(),
            name: name.to_owned(),
            branch: "b".to_owned(),
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
        assert_eq!(landings[1].name, "Second landing");
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
        let landings = vec![landing("a", "Saved workspaces"), landing("b", "Attention tray")];
        assert_eq!(resolve(&landings, "aaaa").unwrap().name, "Saved workspaces");
        assert_eq!(resolve(&landings, "attention").unwrap().name, "Attention tray");
        assert_eq!(resolve(&landings, "SAVED").unwrap().name, "Saved workspaces");
        let none = resolve(&landings, "nothing").unwrap_err();
        assert!(none.contains("cargo xtask landings"), "{none}");
        let both = resolve(&landings, "a").unwrap_err();
        assert!(both.contains("2 landings"), "{both}");
    }

    #[test]
    fn render_prints_one_line_per_landing_and_overlaps_indented() {
        let a = landing("a", "A");
        let b = landing("b", "B");
        let mut map = BTreeMap::new();
        map.insert(a.sha.clone(), vec!["B".to_owned()]);
        let text = render(&[a, b], &map);
        assert_eq!(
            text,
            "aaaaaaa  2026-09-06  A  (b)\n    touches files with: B\nbbbbbbb  2026-09-06  B  (b)\n"
        );
    }

    #[test]
    fn conflict_message_names_the_landing_and_its_overlaps() {
        let text = conflict_message(&landing("a", "A"), &["B".to_owned()]);
        assert!(text.contains("\"A\""), "{text}");
        assert!(text.contains("B"), "{text}");
        assert!(text.contains("--change"), "{text}");
    }
```

Add `use std::collections::{BTreeMap, BTreeSet};` at the top of the file (after the module doc).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p xtask landings 2>&1 | grep -E "error\[|cannot find" | head -5`
Expected: compile errors for `Landing`, `parse_landings`, etc.

- [ ] **Step 3: Implement**

Add above `#[cfg(test)]` in `xtask/src/landings.rs`:

```rust
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
}

/// The `git log --format` that [`parse_landings`] reads: sha, date, subject
/// and the `Landed-from` trailer value, separated by U+001F, one record per
/// U+001E. Control separators cannot appear in a subject or a branch name.
pub const LANDING_LOG_FORMAT: &str =
    "%H%x1f%as%x1f%s%x1f%(trailers:key=Landed-from,valueonly)%x1e";

/// The landings in a `git log --first-parent --merges` run over
/// `origin/main..origin/dev`, oldest first. A merge without the trailer (a
/// release merge-back, a hand merge) is not a landing and is dropped.
pub fn parse_landings(log: &str) -> Vec<Landing> {
    let mut landings: Vec<Landing> = log
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_matches('\n').splitn(4, '\x1f');
            let sha = fields.next()?.trim();
            let date = fields.next()?.trim();
            let name = fields.next()?.trim();
            let branch = fields.next()?.lines().next().unwrap_or("").trim();
            if sha.is_empty() || branch.is_empty() {
                return None;
            }
            Some(Landing {
                sha: sha.to_owned(),
                date: date.to_owned(),
                name: name.to_owned(),
                branch: branch.to_owned(),
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
/// is refused with the candidates.
pub fn resolve<'a>(landings: &'a [Landing], wanted: &str) -> Result<&'a Landing, String> {
    let wanted = wanted.trim();
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
                .map(|landing| format!("{} \"{}\"", &landing.sha[..7], landing.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The menu: one line per landing, oldest first, then an indented overlap line.
pub fn render(landings: &[Landing], overlaps: &BTreeMap<String, Vec<String>>) -> String {
    let mut text = String::new();
    for landing in landings {
        text.push_str(&format!(
            "{}  {}  {}  ({})\n",
            &landing.sha[..7.min(landing.sha.len())],
            landing.date,
            landing.name,
            landing.branch
        ));
        if let Some(others) = overlaps.get(&landing.sha) {
            text.push_str(&format!("    touches files with: {}\n", others.join(", ")));
        }
    }
    text
}

/// What promote says when a landing does not apply on top of `main`.
pub fn conflict_message(landing: &Landing, overlapping: &[String]) -> String {
    let mut text = format!(
        "landing {} \"{}\" does not apply on main by itself.",
        &landing.sha[..7.min(landing.sha.len())],
        landing.name
    );
    if overlapping.is_empty() {
        text.push_str(" No other unshipped landing touches its files; ");
    } else {
        text.push_str(&format!(
            " It shares files with: {}. Add them with --change, or ",
            overlapping.join(", ")
        ));
    }
    text.push_str("ship everything with `promote --version <v>` and no --change.");
    text
}
```

- [ ] **Step 4: Run the tests, clippy, fmt**

Run: `cargo fmt --all && cargo test -p xtask landings 2>&1 | tail -5 && cargo clippy --locked -p xtask --all-targets -- -D warnings 2>&1 | tail -3`
Expected: all landings tests PASS; no warnings.

- [ ] **Step 5: Commit**

```bash
git add xtask/src/landings.rs
git commit -m "feat(xtask): parse, filter, resolve and render landings" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01JwXuHMw85hTFaDncu7kCes"
```

---

### Task 3: Closeout writes the landing

**Files:**
- Modify: `xtask/src/closeout.rs` (module doc; `closeout` signature and body; `git_output` visibility; new `land`)
- Modify: `xtask/src/main.rs` (`Closeout { change }`)
- Test: `xtask/src/closeout.rs` (new temp-repo test)

**Interfaces:**
- Consumes: `landings::landing_name`, `landings::landing_message`, `landings::shipped_shas`.
- Produces:
  - `pub fn closeout(root: &Path, change: Option<&str>) -> Result<()>`
  - `pub(crate) fn git_output(root: &Path, arguments: &[&str]) -> Result<Output>`
  - `pub(crate) fn is_non_fast_forward(stderr: &str) -> bool` (already `pub`)

- [ ] **Step 1: Write the failing temp-repo test**

Append to `mod tests` in `xtask/src/closeout.rs`:

```rust
    /// Runs git in `dir` with a fixed identity, failing the test on error.
    /// (`Command` and `fs` arrive through `use super::*`.)
    fn git_in(dir: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=xtask test",
                "-c",
                "user.email=xtask@test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(arguments)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf-8").trim().to_owned()
    }

    #[test]
    fn closeout_lands_a_first_parent_merge_that_cherry_picks_onto_main() {
        let temp = tempfile::tempdir().expect("temp dir");
        let origin = temp.path().join("origin.git");
        let work = temp.path().join("work");
        git_in(temp.path(), &["init", "--bare", "--quiet", &origin.to_string_lossy()]);
        git_in(temp.path(), &["init", "--quiet", "-b", "main", &work.to_string_lossy()]);
        // The landing is written with commit-tree, which reads the repo config.
        git_in(&work, &["config", "user.name", "xtask test"]);
        git_in(&work, &["config", "user.email", "xtask@test"]);
        git_in(&work, &["remote", "add", "origin", &origin.to_string_lossy()]);
        fs::write(work.join("README.md"), "base\n").expect("readme");
        git_in(&work, &["add", "README.md"]);
        git_in(&work, &["commit", "--quiet", "-m", "base"]);
        git_in(&work, &["push", "--quiet", "origin", "HEAD:main", "HEAD:dev"]);
        git_in(&work, &["fetch", "--quiet", "origin"]);
        let base = git_in(&work, &["rev-parse", "HEAD"]);

        git_in(&work, &["switch", "--quiet", "-c", "fix/thing"]);
        fs::write(work.join("thing.txt"), "one\n").expect("thing");
        git_in(&work, &["add", "thing.txt"]);
        git_in(&work, &["commit", "--quiet", "-m", "fix: add thing"]);
        fs::write(work.join("thing.txt"), "two\n").expect("thing");
        git_in(&work, &["commit", "--quiet", "-am", "fix: refine thing"]);

        // Two commits and no --change: refused, nothing pushed.
        let refusal = closeout(&work, None).unwrap_err().to_string();
        assert!(refusal.contains("--change"), "{refusal}");
        assert_eq!(git_in(&work, &["rev-parse", "origin/dev"]), base);

        closeout(&work, Some("Adds the thing")).expect("closeout lands");

        let landing = git_in(&work, &["rev-parse", "HEAD"]);
        assert_eq!(git_in(&work, &["rev-parse", "origin/dev"]), landing);
        assert_eq!(git_in(&work, &["symbolic-ref", "--short", "HEAD"]), "fix/thing");
        assert_eq!(git_in(&work, &["rev-parse", "HEAD^1"]), base);
        assert_eq!(git_in(&work, &["log", "-1", "--format=%s"]), "Adds the thing");
        let body = git_in(&work, &["log", "-1", "--format=%b"]);
        assert!(body.contains("fix: add thing\nfix: refine thing"), "{body}");
        assert!(body.contains("Landed-from: fix/thing"), "{body}");
        assert_eq!(git_in(&work, &["diff", "--name-only", "HEAD^1", "HEAD"]), "thing.txt");
        assert_eq!(
            git_in(&work, &["log", "--first-parent", "--merges", "--format=%s", "origin/main..origin/dev"]),
            "Adds the thing"
        );
        assert!(git_in(&work, &["status", "--porcelain"]).is_empty());

        // The landing ships onto main as one cherry-pick that records its source.
        git_in(&work, &["checkout", "--quiet", "--detach", "origin/main"]);
        git_in(&work, &["cherry-pick", "-x", "-m", "1", &landing]);
        assert_eq!(fs::read_to_string(work.join("thing.txt")).expect("thing"), "two\n");
        let main_body = git_in(&work, &["log", "-1", "--format=%b"]);
        assert!(
            crate::landings::shipped_shas(&main_body).contains(&landing),
            "{main_body}"
        );

        // A branch that adds nothing after the merge has nothing to land.
        git_in(&work, &["switch", "--quiet", "fix/thing"]);
        let nothing = closeout(&work, Some("Nothing")).unwrap_err().to_string();
        assert!(nothing.contains("nothing to land"), "{nothing}");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p xtask closeout_lands 2>&1 | grep -E "error\[|expected" | head -5`
Expected: compile error — `closeout` takes one argument.

- [ ] **Step 3: Implement the landing build**

In `xtask/src/closeout.rs`:

Replace the module doc's first two paragraphs with:

```rust
//! `cargo xtask closeout`: the AGENTS.md "commit and push to dev" step, and
//! nothing more.
//!
//! The recipe is `git fetch origin dev`, `git merge --no-edit origin/dev` into
//! the task branch, then one landing commit pushed to `dev`: a merge whose
//! first parent is `origin/dev`, whose second parent is the branch tip, whose
//! tree is the branch's tree, whose subject is the `--change` text, and which
//! carries `Landed-from: <branch>`. The landing is written with `git
//! commit-tree`, so `dev` is never checked out, and the branch is then moved
//! onto it, so the agent ends at the dev tip as before. Every landing is a
//! first-parent merge on `dev`, which is what lets `cargo xtask landings` list
//! them and `promote --change` ship one with a `-m 1` cherry-pick. The
//! refusals an agent is supposed to apply by hand stay: never close out a
//! dirty tree, never push from `dev`, `main`, or a detached HEAD, never land
//! without a name. Several agents close out at once, so a push rejected as
//! non-fast-forward refetches and merges once before giving up. A conflicted
//! merge is left in place for the operator: this command resolves nothing and
//! gates nothing.
```

Replace `pub fn closeout(root: &Path) -> Result<()>` through its closing brace with:

```rust
/// Runs the closeout for the checkout at `root`, landing it under `change`.
pub fn closeout(root: &Path, change: Option<&str>) -> Result<()> {
    let branch = current_branch(root)?;
    let dirty = !gate::git_capture(root, &["status", "--porcelain", "--untracked-files=no"])?
        .trim()
        .is_empty();
    if let Err(refusal) = closeout_guard(
        branch.as_deref().unwrap_or_default(),
        branch.is_none(),
        dirty,
    ) {
        bail!("closeout refused: {refusal}");
    }
    let branch = branch.unwrap_or_default();

    // Two attempts, no more: the second exists only because another agent can
    // land on dev between this fetch and this push.
    for attempt in 1..=2_u32 {
        fetch(root)?;
        merge(root)?;
        guard_settled_looks(root)?;
        let old = gate::rev_parse(root, "origin/dev")?;
        let landing = land(root, &branch, change)?;
        let push = git_output(root, &["push", "origin", &format!("{landing}:dev")])?;
        let stderr = String::from_utf8_lossy(&push.stderr).into_owned();
        eprint!("{stderr}");
        if push.status.success() {
            // The landing's tree is the working tree; only the branch ref moves.
            gate::git_capture(root, &["reset", "--soft", &landing])?;
            println!(
                "closeout: landed {branch} on dev as {}..{}",
                gate::short(&old),
                gate::short(&landing)
            );
            return Ok(());
        }
        if attempt == 1 && is_non_fast_forward(&stderr) {
            println!("closeout: dev moved under the push; fetching and merging once more");
            continue;
        }
        bail!("`git push origin {landing}:dev` failed with {}", push.status);
    }
    unreachable!("the loop either pushes or bails")
}

/// Writes the landing commit for the merged branch and returns its object
/// name. Nothing in the working tree changes.
fn land(root: &Path, branch: &str, change: Option<&str>) -> Result<String> {
    let added = gate::git_capture(root, &["rev-list", "--count", "origin/dev..HEAD"])?;
    if added.trim() == "0" {
        bail!("closeout refused: nothing to land; {branch} adds nothing to origin/dev");
    }
    let subjects: Vec<String> =
        gate::git_capture(
            root,
            &["log", "--reverse", "--no-merges", "--format=%s", "origin/dev..HEAD"],
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
    let message = crate::landings::landing_message(&name, &subjects, branch);
    let file = tempfile::Builder::new()
        .prefix("interface-landing-")
        .tempfile()
        .context("could not create the landing message file")?;
    fs::write(file.path(), message)
        .with_context(|| format!("could not write {}", file.path().display()))?;
    let tree = gate::rev_parse(root, "HEAD^{tree}")?;
    let dev = gate::rev_parse(root, "origin/dev")?;
    let head = gate::rev_parse(root, "HEAD")?;
    let landing = gate::git_capture(
        root,
        &[
            "commit-tree",
            &tree,
            "-p",
            &dev,
            "-p",
            &head,
            "-F",
            &file.path().to_string_lossy(),
        ],
    )?;
    println!("closeout: landing \"{name}\"");
    Ok(landing.trim().to_owned())
}
```

Change `fn git_output(` to `pub(crate) fn git_output(`.

In `xtask/src/main.rs` replace:

```rust
    /// Merge origin/dev into this task branch and push it to dev.
    Closeout,
```

with:

```rust
    /// Merge origin/dev into this task branch and land it on dev as one named
    /// merge commit.
    ///
    /// The landing's subject is `--change`; without it, closeout refuses
    /// unless the branch adds exactly one commit, whose subject then names
    /// the landing. `cargo xtask landings` lists the landings main lacks and
    /// `promote --change` ships one of them.
    Closeout {
        /// One line saying what this landing is, for the owner to pick later.
        #[arg(long)]
        change: Option<String>,
    },
```

and `Task::Closeout => closeout::closeout(&root),` with
`Task::Closeout { change } => closeout::closeout(&root, change.as_deref()),`.

- [ ] **Step 4: Run the test, the whole xtask suite, clippy, fmt**

Run: `cargo fmt --all && cargo test -p xtask 2>&1 | tail -5 && cargo clippy --locked -p xtask --all-targets -- -D warnings 2>&1 | tail -3`
Expected: all PASS, including `closeout_lands_a_first_parent_merge_that_cherry_picks_onto_main`; no warnings.

- [ ] **Step 5: Commit**

```bash
git add xtask/src/closeout.rs xtask/src/main.rs
git commit -m "feat(xtask): closeout lands one named merge commit on dev" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01JwXuHMw85hTFaDncu7kCes"
```

---

### Task 4: `cargo xtask landings`

**Files:**
- Modify: `xtask/src/landings.rs` (git wrappers and the command)
- Modify: `xtask/src/main.rs` (`Landings` subcommand)

**Interfaces:**
- Consumes: `gate::git_capture`, `parse_landings`, `shipped_shas`, `unshipped`, `overlaps`, `render`.
- Produces:
  - `pub fn unshipped_landings(root: &Path) -> Result<Vec<Landing>>` — fetches nothing; callers fetch
  - `pub fn landing_paths(root: &Path, sha: &str) -> Result<Vec<String>>`
  - `pub fn landing_overlaps(root: &Path, landings: &[Landing]) -> Result<BTreeMap<String, Vec<String>>>`
  - `pub fn landings_command(root: &Path) -> Result<()>`

- [ ] **Step 1: Extend the temp-repo test to cover the wrappers**

In `xtask/src/closeout.rs`, inside `closeout_lands_a_first_parent_merge_that_cherry_picks_onto_main`, just before the `// The landing ships onto main` comment, add:

```rust
        let listed = crate::landings::unshipped_landings(&work).expect("landings read");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].sha, landing);
        assert_eq!(listed[0].name, "Adds the thing");
        assert_eq!(listed[0].branch, "fix/thing");
        assert_eq!(
            crate::landings::landing_paths(&work, &landing).expect("paths"),
            vec!["thing.txt".to_owned()]
        );
```

and after the cherry-pick assertions (after the `shipped_shas` assert), add:

```rust
        git_in(&work, &["push", "--quiet", "origin", "HEAD:main"]);
        git_in(&work, &["fetch", "--quiet", "origin"]);
        assert!(
            crate::landings::unshipped_landings(&work).expect("landings read").is_empty(),
            "a -x cherry-pick on main marks the landing shipped"
        );
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p xtask closeout_lands 2>&1 | grep -E "error\[" | head -3`
Expected: `unshipped_landings` and `landing_paths` not found.

- [ ] **Step 3: Implement the wrappers and the command**

Put these imports at the top of `xtask/src/landings.rs`, together with the existing `use std::collections::{BTreeMap, BTreeSet};` (rustfmt wants every `use` before the first item):

```rust
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::Result;

use crate::gate;
```

Then append above `#[cfg(test)]`:

```rust
/// The landings `origin/dev` has that `origin/main` lacks, oldest first, with
/// the ones `main` already carries as `-x` cherry-picks removed. Reads the
/// remote-tracking refs as they are; the caller fetches first.
pub fn unshipped_landings(root: &Path) -> Result<Vec<Landing>> {
    let log = gate::git_capture(
        root,
        &[
            "log",
            "--first-parent",
            "--merges",
            &format!("--format={LANDING_LOG_FORMAT}"),
            "origin/main..origin/dev",
        ],
    )?;
    let main = gate::git_capture(root, &["log", "--format=%b", "origin/main"])?;
    Ok(unshipped(parse_landings(&log), &shipped_shas(&main)))
}

/// The paths a landing changes: its first-parent diff.
pub fn landing_paths(root: &Path, sha: &str) -> Result<Vec<String>> {
    Ok(gate::git_capture(
        root,
        &["diff", "--name-only", &format!("{sha}^1"), sha],
    )?
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .map(str::to_owned)
    .collect())
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
    print!("{}", render(&landings, &overlaps));
    println!(
        "landings: {} unshipped; ship one with `cargo xtask promote --version <v> --change \
         \"<name or sha>\"`",
        landings.len()
    );
    Ok(())
}
```

In `xtask/src/main.rs` add to `enum Task` after `Closeout { … }`:

```rust
    /// List the landings on dev that main does not carry yet.
    ///
    /// One line per landing, oldest first: short SHA, date, name, task
    /// branch, and which other unshipped landings change a common file. A
    /// landing already shipped by `promote --change` is recognized by the
    /// `cherry picked from` line on main and left out.
    Landings,
```

and in `main()`: `Task::Landings => landings::landings_command(&root),`.

- [ ] **Step 4: Run tests, clippy, fmt**

Run: `cargo fmt --all && cargo test -p xtask 2>&1 | tail -5 && cargo clippy --locked -p xtask --all-targets -- -D warnings 2>&1 | tail -3`
Expected: all PASS; no warnings.

- [ ] **Step 5: Try it on the real repository**

Run: `cargo xtask landings`
Expected: `landings: dev has nothing main lacks` (or the current unshipped landings if any exist; none carry the trailer yet, so the list is empty). This proves the git invocations run against a real remote.

- [ ] **Step 6: Commit**

```bash
git add xtask/src/landings.rs xtask/src/main.rs xtask/src/closeout.rs
git commit -m "feat(xtask): landings lists what dev has that main lacks" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01JwXuHMw85hTFaDncu7kCes"
```

---

### Task 5: `promote --change` ships chosen landings

**Files:**
- Modify: `xtask/src/promote.rs` (module doc; `bump` signature; new `promote_landings` and helpers)
- Modify: `xtask/src/main.rs` (`Promote { changes }` and routing)

**Interfaces:**
- Consumes: `landings::{Landing, unshipped_landings, landing_overlaps, resolve, conflict_message, RELEASE_TRAILER}`, `closeout::{git_output, is_non_fast_forward}`, existing `bump`, `package`, `check_release_worktree`, `default_release_worktree`, `feed_version`, `is_ancestor`, `assert_promoted_head`, `release::{default_feed_target, stage_update_feed, nudge_running_production_app, compare_versions}`.
- Produces:
  - `pub fn promote_landings(root: &Path, version: &str, changes: &[String], dry_run: bool, no_push: bool, release_worktree: Option<PathBuf>) -> Result<()>`
  - `fn bump(root: &Path, version: &str, body: &str) -> Result<()>` (was `dev_head: &str`)
  - `pub fn version_in_package_json(text: &str) -> Result<String>` (pure, tested)
  - `pub fn selected_in_dev_order<'a>(landings: &'a [Landing], wanted: &[String]) -> Result<Vec<&'a Landing>>` (pure, tested)
  - `pub fn finish_commands(promoted: &str, version: &str, release: &Path) -> String` (pure, tested)

- [ ] **Step 1: Write the failing unit tests**

Append inside `mod tests` in `xtask/src/promote.rs`:

```rust
    fn landing(sha: char, name: &str) -> crate::landings::Landing {
        crate::landings::Landing {
            sha: sha.to_string().repeat(40),
            date: "2026-09-06".to_owned(),
            name: name.to_owned(),
            branch: "b".to_owned(),
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
        let landings = vec![landing('a', "First"), landing('b', "Second"), landing('c', "Third")];
        let picked = selected_in_dev_order(
            &landings,
            &["third".to_owned(), "aaaa".to_owned(), "First".to_owned()],
        )
        .unwrap();
        let names: Vec<&str> = picked.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["First", "Third"]);
        let refusal = selected_in_dev_order(&landings, &["nope".to_owned()]).unwrap_err();
        assert!(refusal.to_string().contains("cargo xtask landings"), "{refusal}");
        assert!(selected_in_dev_order(&landings, &[]).is_err());
    }

    #[test]
    fn finish_commands_spell_out_main_the_merge_back_and_the_feed() {
        let text = finish_commands("abc1234", "0.3.66", Path::new("/r/release"));
        assert!(text.contains("git push origin abc1234:main"), "{text}");
        assert!(text.contains("git checkout --detach origin/dev"), "{text}");
        assert!(text.contains("Release: 0.3.66"), "{text}");
        assert!(text.contains("git push origin HEAD:dev"), "{text}");
        assert!(text.contains("cargo xtask stage-update --source /r/release"), "{text}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p xtask version_in_package_json selected_landings finish_commands 2>&1 | grep -E "error\[" | head -3`
Expected: the three functions are not found.

- [ ] **Step 3: Change `bump` to take its body**

In `xtask/src/promote.rs` change the signature `fn bump(root: &Path, version: &str, dev_head: &str) -> Result<()>` to `fn bump(root: &Path, version: &str, body: &str) -> Result<()>`, and inside it replace

```rust
            "-m",
            &format!(
                "Promoted from dev {} by cargo xtask promote.",
                gate::short(dev_head)
            ),
```

with

```rust
            "-m",
            body,
```

In `promote()`, change `Resume::No => bump(root, version, &facts.head)?,` to:

```rust
        Resume::No => bump(
            root,
            version,
            &format!(
                "Promoted from dev {} by cargo xtask promote.",
                gate::short(&facts.head)
            ),
        )?,
```

- [ ] **Step 4: Add the pure helpers**

Add to `xtask/src/promote.rs` after `bump_subject`:

```rust
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
    ensure!(!wanted.is_empty(), "promote --change needs at least one landing");
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

/// The commands that finish a selective release from a packaged `promoted`
/// commit: push main, merge it back into dev, stage the feed.
pub fn finish_commands(promoted: &str, version: &str, release: &Path) -> String {
    format!(
        "    git push origin {promoted}:main\n\
         \x20   git fetch origin dev && git checkout --detach origin/dev && \
         git merge --no-ff --no-edit -m \"Release {version} merged back to dev\" \
         -m \"{} {version}\" {promoted} && git push origin HEAD:dev\n\
         \x20   cargo xtask stage-update --source {}\n",
        crate::landings::RELEASE_TRAILER,
        release.display()
    )
}
```

- [ ] **Step 5: Run the unit tests**

Run: `cargo fmt --all && cargo test -p xtask version_in_package_json selected_landings finish_commands 2>&1 | tail -5`
Expected: 3 PASS.

- [ ] **Step 6: Implement `promote_landings`**

Add to `xtask/src/promote.rs` after `promote()`:

```rust
/// Where HEAD was before a selective promote detached it, so it can be put
/// back.
struct Position {
    branch: Option<String>,
    sha: String,
}

fn current_position(root: &Path) -> Result<Position> {
    let output = gate::git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|name| !name.is_empty());
    Ok(Position {
        branch,
        sha: gate::rev_parse(root, "HEAD")?,
    })
}

fn restore(root: &Path, position: &Position) {
    let result = match position.branch.as_deref() {
        Some(branch) => gate::git_capture(root, &["checkout", "--quiet", branch]),
        None => gate::git_capture(root, &["checkout", "--quiet", "--detach", &position.sha]),
    };
    if let Err(error) = result {
        println!(
            "promote: could not put HEAD back on {} ({error}); do it by hand",
            position.branch.as_deref().unwrap_or(&position.sha)
        );
    }
}

/// `promote --change …`: ship the named landings and nothing else.
///
/// Builds the release on a head detached from `origin/main` — one `-x -m 1`
/// cherry-pick per landing, oldest first — then runs the same bump, full gate,
/// package-and-smoke and feed staging as a whole-dev promote, pushes `main`,
/// and merges `main` back into `dev` so the next landing sees the release.
/// Nothing here resumes: a rerun rebuilds the head from scratch, and only the
/// gate and the packaging cost anything.
pub fn promote_landings(
    root: &Path,
    version: &str,
    changes: &[String],
    dry_run: bool,
    no_push: bool,
    release_worktree: Option<PathBuf>,
) -> Result<()> {
    let worktree = release_worktree.map_or_else(default_release_worktree, Ok)?;
    println!("promote: step 1/7 — preconditions (selective)");
    ensure!(
        gate::tree_is_clean(root)?,
        "promote needs a clean working tree; commit or set aside your changes first."
    );
    check_release_worktree(root, &worktree)?;
    gate::git_capture(root, &["fetch", "origin"])?;
    let landings = crate::landings::unshipped_landings(root)?;
    let selected = selected_in_dev_order(&landings, changes)?;
    let main_version = version_in_package_json(&gate::git_capture(
        root,
        &["show", &format!("origin/main:{PACKAGE_JSON}")],
    )?)?;
    ensure!(
        compare_versions(version, &main_version)? == Ordering::Greater,
        "Requested version {version} must be newer than main's {PACKAGE_JSON} {main_version}."
    );
    if let Some(feed) = feed_version()? {
        ensure!(
            compare_versions(version, &feed)? == Ordering::Greater,
            "Requested version {version} must be newer than the staged feed's {feed}."
        );
    }
    let origin_main = gate::rev_parse(root, "origin/main")?;
    println!(
        "promote: shipping {} landing(s) onto main {} as Interface {version}:",
        selected.len(),
        gate::short(&origin_main)
    );
    for landing in &selected {
        println!("  {}  {}", gate::short(&landing.sha), landing.name);
    }
    if dry_run {
        println!("promote: dry run — the plan:");
        println!("  2. detach at origin/main and cherry-pick each landing with -x -m 1");
        println!("  3. bump to {version} and commit");
        println!("  4. full gate (a fresh tree has no stamp)");
        println!("  5. package and smoke in {}", worktree.display());
        if no_push {
            println!("  6. and 7. skipped (--no-push): the finishing commands are printed");
        } else {
            println!("  6. push main, merge it back into dev, push dev");
            println!("  7. stage the update feed");
        }
        return Ok(());
    }

    let origin = current_position(root)?;
    println!("promote: step 2/7 — cherry-pick onto origin/main");
    gate::git_capture(root, &["checkout", "--quiet", "--detach", &origin_main])?;
    for landing in &selected {
        let pick = gate::git(root, &["cherry-pick", "-x", "-m", "1", &landing.sha])?;
        if pick.status.success() {
            println!("promote: picked {} \"{}\"", gate::short(&landing.sha), landing.name);
            continue;
        }
        let _ = gate::git(root, &["cherry-pick", "--abort"]);
        let overlapping = crate::landings::landing_overlaps(root, &landings)?
            .remove(&landing.sha)
            .unwrap_or_default();
        restore(root, &origin);
        bail!(
            "promote refused: {}",
            crate::landings::conflict_message(landing, &overlapping)
        );
    }
    let shorts: Vec<&str> = selected.iter().map(|l| gate::short(&l.sha)).collect();
    if let Err(error) = bump(
        root,
        version,
        &format!(
            "Promoted landings {} from dev by cargo xtask promote.",
            shorts.join(", ")
        ),
    ) {
        restore(root, &origin);
        return Err(error);
    }
    let promoted = gate::rev_parse(root, "HEAD")?;
    let leave = || {
        format!(
            "the release head {} is committed locally (detached); nothing pushed. Fix and rerun \
             the same promote; it rebuilds the head. HEAD is left there for inspection.",
            gate::short(&promoted)
        )
    };
    println!("promote: step 3/7 — full gate");
    gate::run_full_gate(root).with_context(leave)?;
    let packaged = package(root, &worktree, &promoted, Resume::No, version).with_context(leave)?;
    if no_push {
        println!("promote: steps 6/7 and 7/7 skipped (--no-push)");
        println!("  Interface {packaged} packaged as {} in {}", gate::short(&promoted), worktree.display());
        println!("  Finish it with:");
        print!("{}", finish_commands(&promoted, version, &worktree.join("release")));
        println!("  then put HEAD back: git checkout {}", origin.branch.as_deref().unwrap_or(&origin.sha));
        return Ok(());
    }
    println!("promote: step 6/7 — push main, then merge it back into dev");
    assert_promoted_head(root, &promoted, "nothing was pushed")?;
    gate::git_capture(root, &["fetch", "origin", "main"])?;
    ensure!(
        is_ancestor(root, "origin/main", &promoted)?,
        "origin/main moved while promote ran; nothing was pushed. Rerun the same promote."
    );
    gate::git_capture(root, &["push", "origin", &format!("{promoted}:main")])
        .context("nothing was pushed; rerun the same promote")?;
    println!("promote: pushed {} to main", gate::short(&promoted));
    let release = worktree.join("release");
    if let Err(error) = merge_back(root, &promoted, version) {
        restore(root, &origin);
        return Err(error).with_context(|| {
            format!(
                "main carries {}; dev does not, and no feed is staged. Finish it with:\n{}",
                gate::short(&promoted),
                finish_commands(&promoted, version, &release)
            )
        });
    }
    println!("promote: step 7/7 — stage the update feed");
    let target = release::default_feed_target()?;
    let staged = release::stage_update_feed(&release, &target, false)
        .with_context(|| half_promoted(&release))?;
    release::nudge_running_production_app();
    restore(root, &origin);
    println!("promote: done");
    println!(
        "  Interface {packaged} promoted as {} carrying {} landing(s)",
        gate::short(&promoted),
        selected.len()
    );
    println!("  feed {}", staged.target_dir.display());
    println!(
        "Next: the running app should show Restart to update; installing remains an owner action."
    );
    Ok(())
}

/// Merges the promoted release into `dev` as one `Release:`-trailered merge
/// and pushes it, retrying once when another landing wins the race.
fn merge_back(root: &Path, promoted: &str, version: &str) -> Result<()> {
    let subject = format!("Release {version} merged back to dev");
    let trailer = format!("{} {version}", crate::landings::RELEASE_TRAILER);
    for attempt in 1..=2_u32 {
        gate::git_capture(root, &["fetch", "origin", "dev"])?;
        gate::git_capture(root, &["checkout", "--quiet", "--detach", "origin/dev"])?;
        let merge = crate::closeout::git_output(
            root,
            &["merge", "--no-ff", "--no-edit", "-m", &subject, "-m", &trailer, promoted],
        )?;
        if !merge.status.success() {
            let _ = gate::git(root, &["merge", "--abort"]);
            bail!(
                "merging {} back into dev conflicts: {}",
                gate::short(promoted),
                String::from_utf8_lossy(&merge.stdout).trim()
            );
        }
        let push = crate::closeout::git_output(root, &["push", "origin", "HEAD:dev"])?;
        let stderr = String::from_utf8_lossy(&push.stderr).into_owned();
        eprint!("{stderr}");
        if push.status.success() {
            println!("promote: merged {} back into dev", gate::short(promoted));
            return Ok(());
        }
        if attempt == 1 && crate::closeout::is_non_fast_forward(&stderr) {
            println!("promote: dev moved under the merge-back; merging once more");
            continue;
        }
        bail!("`git push origin HEAD:dev` failed with {}", push.status);
    }
    unreachable!("the loop either pushes or bails")
}
```

Update the module doc at the top of `promote.rs`: after the `--dry-run` paragraph add:

```rust
//!
//! `--change <name or sha>` (repeatable) is the selective release: instead of
//! promoting all of `dev`, it detaches at `origin/main`, cherry-picks each
//! named landing (`cargo xtask landings` lists them) with `-x -m 1`, oldest
//! first, then bumps, gates in full, packages, pushes `main`, merges `main`
//! back into `dev` under a `Release:` trailer, and stages the feed. It never
//! resumes: a rerun rebuilds the same head. Without `--change` nothing about
//! the whole-dev promotion changes.
```

- [ ] **Step 7: Wire the flag**

In `xtask/src/main.rs`, in `Task::Promote` add after `version`:

```rust
        /// Ship only this landing (name substring or SHA prefix); repeatable.
        /// Without it, all of dev is promoted.
        #[arg(long = "change")]
        changes: Vec<String>,
```

and replace the `Task::Promote { … } => promote::promote(…)` arm with:

```rust
        Task::Promote {
            version,
            changes,
            dry_run,
            no_push,
            release_worktree,
        } => {
            if changes.is_empty() {
                promote::promote(&root, &version, dry_run, no_push, release_worktree)
            } else {
                promote::promote_landings(
                    &root,
                    &version,
                    &changes,
                    dry_run,
                    no_push,
                    release_worktree,
                )
            }
        }
```

Also extend the `Promote` doc comment's first paragraph with one sentence: `With --change, only the named landings are shipped (see \`landings\`).` Remove the `#[allow(dead_code)]` from `mod landings;` — every item now has a caller.

- [ ] **Step 8: Run tests, clippy, fmt, and a dry run**

Run: `cargo fmt --all && cargo test -p xtask 2>&1 | tail -5 && cargo clippy --locked -p xtask --all-targets -- -D warnings 2>&1 | tail -3`
Expected: all PASS; no warnings.

Run: `cargo xtask promote --version 9.9.9 --change nothing --dry-run 2>&1 | tail -3`
Expected: `promote refused: no unshipped landing matches "nothing"; run \`cargo xtask landings\` for the menu` (there are no landings yet). This proves the preconditions run against the real remote without changing anything.

- [ ] **Step 9: Commit**

```bash
git add xtask/src/promote.rs xtask/src/main.rs
git commit -m "feat(xtask): promote --change ships chosen landings to main" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01JwXuHMw85hTFaDncu7kCes"
```

---

### Task 6: Documentation and the first landing

**Files:**
- Modify: `AGENTS.md` (Closeout section, steps 1, 3 and 4)
- Modify: `docs/areas/xtask.md` (module list; the `promote` paragraph; a `closeout`/`landings` paragraph)

- [ ] **Step 1: AGENTS.md, step 1**

Replace the block from `       cargo xtask closeout` through `runs.` (the paragraph ending "The latest push is what the Dev app runs.") with:

```markdown
       cargo xtask closeout --change "<one line saying what this lands>"

   It fetches `origin/dev`, merges it into the branch you are on, and lands the
   branch on `dev` as ONE merge commit named by `--change`: first parent the
   previous dev tip, trailer `Landed-from: <branch>`. The name is what the owner
   reads when choosing what to ship, so write it for them ("Saved workspaces
   file under their own project"), not as a commit subject. Without `--change`
   closeout refuses unless the branch adds exactly one commit, whose subject
   then names the landing. It refuses a dirty tree and refuses to run while you
   are on `dev` or `main` — `git checkout dev` is refused locally anyway,
   because the local `dev` branch is pinned to the
   `~/Desktop/Projects/.interface-dev` checkout that runs the Dev app. If your
   work sits directly on local `main`, move it to a branch first
   (`git switch -c <task-branch>`). Do not leave product changes in the working
   tree. Several agents close out concurrently: if the push is rejected as
   non-fast-forward, run `closeout` again. The latest landing is what the Dev
   app runs. `cargo xtask landings` lists the landings `main` does not carry
   yet.
```

- [ ] **Step 2: AGENTS.md, step 3**

After the sentence ending `Installing or restarting the production app is a separate, owner-requested step.` (end of step 3), add a new paragraph inside step 3:

```markdown
   To ship ONE landing and leave the rest on `dev` — the owner says "ship X" —
   name it:

       cargo xtask promote --version <next> --change "<name or sha>"

   `--change` repeats. It detaches at `origin/main`, cherry-picks each named
   landing (`-x -m 1`, oldest first), bumps, runs the full gate and the
   packaged smoke, pushes `main`, merges `main` back into `dev` under a
   `Release:` trailer, and stages the feed; `dev` keeps every landing, shipped
   or not, so later work still builds on them. A landing that does not apply
   on `main` by itself is refused with the landings that share its files; add
   them with `--change` or ship everything. It never resumes: rerun the same
   command and it rebuilds the head. Run it from any clean checkout; it puts
   HEAD back where it found it.
```

- [ ] **Step 3: AGENTS.md, step 4**

Replace `4. Report the \`dev\` push, Dev-app restart, \`main\` promotion, and production
   staging as distinct states.` with:

```markdown
4. Report the `dev` landing (its name and range), Dev-app restart, `main`
   promotion (which landings it carried), and production staging as distinct
   states.
```

- [ ] **Step 4: docs/areas/xtask.md**

In the module list, change the `main.rs` line's command list to `check` (`--changed`, `--base <rev>`, `--full`), `package-mac` (`--smoke`, `--trust-gate`), `verify-package`, `stage-update`, `promote` (`--change`), `closeout` (`--change`), `landings`, `dev-restart`, `worktree-prune`. Add after the `promote.rs` bullet:

```markdown
- `xtask/src/landings.rs` — named landings: the `--change` rule and landing
  message closeout writes, the `git log --first-parent --merges` parser, the
  shipped-set filter (`cherry picked from commit` lines on `main`), overlap by
  changed path, `--change` resolution, and the `landings` menu.
- `xtask/src/closeout.rs` — the AGENTS.md closeout: merge `origin/dev` into the
  task branch, refuse silent settled-look changes, land the branch on `dev` as
  one `commit-tree` merge (first parent `origin/dev`, `Landed-from:` trailer),
  push it, move the branch onto it.
```

After the `promote` paragraph (ending "since step 4 detaches the one it uses.") add:

```markdown
`cargo xtask promote --version <v> --change <name or sha>…` is the selective
release: it detaches at `origin/main`, cherry-picks each named unshipped landing
with `-x -m 1` oldest first (a conflict aborts, restores HEAD, and names the
landings sharing files), bumps, runs the full gate, packages and smokes as
above, pushes `main`, merges `main` back into `dev` as a `Release: <v>` merge
(one retry on a lost push race), stages the feed, and restores HEAD. It never
resumes; a rerun rebuilds the head. `cargo xtask landings` prints the menu it
picks from: every `Landed-from:` first-parent merge in `origin/main..origin/dev`
whose SHA no `cherry picked from commit` line on `main` names.
```

- [ ] **Step 5: Commit**

```bash
git add AGENTS.md docs/areas/xtask.md
git commit -m "docs: closeout --change, landings, and promote --change" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01JwXuHMw85hTFaDncu7kCes"
```

- [ ] **Step 6: Gate and land this work through the new closeout**

Run: `cargo xtask check --changed 2>&1 | tail -3`
Expected: exit 0 (Rust + docs diff; the renderer half is skipped or run, either is fine).

Run: `cargo xtask closeout --change "Named landings on dev and promote --change to ship them one at a time"`
Expected: `closeout: landing "Named landings on dev and promote --change to ship them one at a time"` then `closeout: landed feat/per-landing-promotion on dev as <old>..<new>`.

Run: `cargo xtask landings`
Expected: one line naming this landing.

Run: `cargo xtask dev-restart` (product code did not change, so this is optional; skip it).

Record the landing SHA in the session note for the ledger (`_AI/Sessions/…`) per the vault instructions.

---

## Self-review

**Spec coverage.** Workflow 1 (closeout `--change`, the single-commit rule, refusal) → Task 3. Workflow 2 (`landings` menu with overlaps) → Tasks 2 and 4. Workflow 3 (selective promote, gate, smoke, main, merge-back, feed) → Task 5. Workflow 4 (ship-all unchanged) → Task 5 routes on `changes.is_empty()` and leaves `promote()` intact apart from `bump`'s body argument. Closeout mechanics steps 1–6 → Task 3 (`fetch`, `merge`, guard, `rev-list --count`, `commit-tree`, push, `reset --soft`). Landings command steps 1–5 → Task 4. Selective promote preconditions and steps 1–8, `--dry-run`, `--no-push` → Task 5. Code placement → file structure above. Error handling → refusal strings in Tasks 3 and 5, `finish_commands`. Testing → unit tests in Tasks 1, 2, 5 and the temp-repo test in Tasks 3–4. Rollout → Task 6.

**Placeholders.** None: every step carries its code or its command and expected output.

**Type consistency.** `Landing { sha, date, name, branch }` is used identically in `landings.rs`, `promote.rs` tests and `promote_landings`. `closeout(root, change: Option<&str>)` matches `main.rs`'s `change.as_deref()`. `bump(root, version, body)` is called with a body in both `promote()` and `promote_landings()`. `git_output` and `is_non_fast_forward` are reached as `crate::closeout::…` from `promote.rs`; `git_output` becomes `pub(crate)` in Task 3.
