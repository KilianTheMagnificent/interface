# Release contents and shipped pins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every landing states its kind, what the owner should see and the tests
that prove it; every release records what it ships and leaves out; shipped
tests cannot be rewritten without a `Supersedes:` trailer.

**Architecture:** Metadata lives in git trailers on the landing merge written by
`closeout`. `promote` splits unshipped landings into included and left out,
requires `--leave-out` for user-facing omissions, stores both lists in a
schema-2 promotion record and on the bump commit, and prints a grouped summary.
A new `pins.rs` guards shipped `Landing-pin:` tests at closeout and re-checks
them in the release candidate.

**Tech Stack:** Rust (`xtask` crate), clap, serde, git CLI.

**Spec:** `docs/superpowers/specs/2026-09-24-release-contents-design.md`

## Global Constraints

- Kinds: `feature`, `fix`, `look`, `speed`, `internal`, `record`; no trailer = unclassified.
- User-facing: `feature`, `fix`, `look`, `speed`, unclassified.
- `--check` one line, non-empty, at most 160 characters.
- Pin format `<repo-relative path>::<test name>`; the name must occur exactly once (identifier-bounded) in the file.
- Trailers: `Landing-kind:`, `Landing-check:`, `Landing-pin:`, `Supersedes:`, `Ships:`, `Leaves-out:`; `Landed-from:` stays last on a landing.
- `Supersedes: <sha prefix ≥7 hex | exact landing name> — <reason>`.
- Promotion record schema 2; schema 1 still reads and resumes.
- The bump body keeps the exact `Promotion-run:` marker line.
- No new dependencies. Verify with `cargo test --locked -p xtask`, `cargo xtask navigate --check`, then `cargo xtask check`.

## Review Focus

- An agent passes `--kind` on a docs-only landing: honour the explicit kind.
- A shipped pin's file is renamed in the push: refuse unless superseded.
- A test is appended directly after a pinned test: must not trigger the guard.
- A selective release omits a landing that added the test an included landing pins: the candidate re-check names the missing pin.
- A schema-1 promotion record resumes with an empty `--leave-out`.

## File map

- Modify `xtask/src/landings.rs`: `Kind`, trailer constants, `LandingMeta`
  validation, extended `landing_message`/`Landing`/`parse_landings`, `Entry`,
  `split_release`, `release_summary`, grouped `render` with waited counts.
- Create `xtask/src/pins.rs`: pin parsing, bounded occurrences, test spans,
  hunk intersection, `Supersede` parsing and matching, shipped-pin log parsing,
  closeout `guard`, `validate_new`, promote `verify_tree`.
- Modify `xtask/src/closeout.rs`: accept `LandingRequest`, infer/validate
  metadata after merge, run the pin guard, copy branch `Supersedes:` trailers,
  refuse a resume with different metadata.
- Modify `xtask/src/promotion_run.rs` + `promote.rs`: `--leave-out`, split,
  acknowledgment, schema 2, bump trailers, summary output, pin re-check.
- Modify `xtask/src/main.rs`: new flags.
- Docs: `AGENTS.md`, `docs/AGENT_RELEASE_WORKFLOW.md`, `docs/areas/xtask.md`,
  `docs/agent-map.json` (pipeline entry points), `docs/TODO.md`.

### Task 1: Landing metadata in `landings.rs`

**Produces:**
- `pub enum Kind { Feature, Fix, Look, Speed, Internal, Record }` with
  `parse(&str) -> Option<Kind>`, `as_str()`, `label()` (group heading),
  serde lowercase.
- `pub fn user_facing(kind: Option<Kind>) -> bool`
- `pub struct LandingMeta { kind: Kind, check: Option<String>, pins: Vec<String>, supersedes: Vec<String> }`
- `pub fn landing_meta(kind: Option<&str>, check: Option<&str>, pins: &[String], docs_only: bool) -> Result<LandingMeta, String>`
- `pub fn landing_message(name, subjects, meta: &LandingMeta, branch) -> String`
- `Landing` gains `committed: i64, kind: Option<Kind>, check: Option<String>, pins: Vec<String>, supersedes: Vec<String>`.
- `pub struct Entry { sha, name, kind: Option<Kind>, check: Option<String> }` (serde) and `Entry::from(&Landing)`.
- `pub struct Replacement { sha, name, reason, by }` (serde).
- `pub fn release_summary(version, included: &[Entry], left_out: &[Entry], replacements: &[Replacement]) -> String`
- `pub fn render(landings, overlaps, waited: &BTreeMap<String, usize>) -> String` grouped by kind.

Tests: kind table (required check/pins per kind, docs-only → record, explicit
kind on docs-only honoured, unknown kind refused with the list), check length
and newline refusals, message trailer order, `parse_landings` round-trip with
two pins and a supersede, unclassified legacy records, summary grouping/order/
omitted groups/counts, grouped render with waited counts.

### Task 2: `pins.rs`

**Produces:**
- `pub struct Pin { path: String, test: String }`, `pub fn parse_pin(&str) -> Result<Pin, String>`
- `pub fn occurrences(text: &str, test: &str) -> Vec<usize>` (0-based lines, identifier-bounded)
- `pub fn test_span(text: &str, test: &str, rust: bool) -> Option<(usize, usize)>` (0-based inclusive; leading `#[`/`//` lines included; end by bracket balance, quotes and `//` comments skipped; `'` is not a quote in Rust)
- `pub fn diff_touches(diff: &str, span: (usize, usize)) -> bool` (`-U0` hunks; old-range intersection, insertions strictly inside)
- `pub struct Supersede { reference: String, reason: String }`, `pub fn parse_supersede(&str) -> Option<Supersede>`, `Supersede::names(&self, shas: &[String], name: &str) -> bool`
- `pub struct ShippedPin { pin: String, shas: Vec<String>, name: String }`, `pub fn parse_pin_log(log: &str) -> Vec<ShippedPin>`
- git wrappers: `shipped_pins(root, rev)`, `supersedes_in(root, rev)`, `validate_new(root, rev, pins)`, `guard(root, dev) -> Result<()>`, `verify_tree(root, rev, pins: &[ShippedPin], supersedes: &[Supersede]) -> Result<()>`

Tests: pin parse errors, bounded occurrences (`foo` vs `foo_bar`), Rust span
with `#[test]`/`#[tokio::test]`, TS one-liner and block spans, lifetimes, braces
in strings, span at end of file, hunk intersection (change inside, insertion
inside, append after the closing line not touching), supersede parse with em
dash / ` - ` / missing reason, sha-prefix and exact-name matching with
near-misses, pin-log merge of a landing and its cherry-pick.

### Task 3: closeout integration

`closeout(root, change, request: &LandingRequest)` with
`LandingRequest { kind: Option<String>, check: Option<String>, pins: Vec<String> }`.
After the merge and settled-look guard: `pins::guard(root, &dev)`, then
`landing_meta(..., docs_only = !product_changed(dev, HEAD))`,
`pins::validate_new(root, "HEAD", &meta.pins)`, supersedes gathered from
`<dev>..HEAD` bodies and validated against known landings. `land` writes the
trailers. A resumed landing refuses explicitly passed flags that differ.

Tests (temp repos): feature without `--pin` refused, nothing pushed; absent or
duplicated pin name refused; docs-only lands as `record`; shipped pin edit
refused, then passes with `Supersedes:` and the landing carries the trailer;
a test appended after the pinned one passes.

### Task 4: promote integration

`Request.leave_out: Vec<String>`; `Run` schema 2 with `#[serde(default)]`
`leave_out`, `included`, `left_out`, `supersedes`. `plan` computes the split
(`landings::split_release`), replacements from included landings' supersedes,
and for whole-dev runs `pins::verify_tree` on the approved revision. `execute`
prints the summary, then refuses unacknowledged user-facing left-outs (dry run
too, after printing). `prepare` re-checks pins on the candidate before the bump
and writes `Ships:`/`Leaves-out:` under the marker. Completion prints what is
still on Dev only.

Tests: split for whole-dev and selective; `--leave-out` naming an included
landing or nothing refused; unacknowledged list; bump body trailers keep the
marker line; schema-1 JSON deserializes; `verify_tree` refuses a missing pin
and accepts a superseded one.

### Task 5: docs and gate

Update `AGENTS.md` closeout/promotion steps, `docs/AGENT_RELEASE_WORKFLOW.md`,
`docs/areas/xtask.md`, `docs/agent-map.json`, `docs/TODO.md`. Run
`cargo test --locked -p xtask`, `cargo xtask navigate --check`, `cargo xtask
check`, then closeout with this feature's own kind, check and pins.
