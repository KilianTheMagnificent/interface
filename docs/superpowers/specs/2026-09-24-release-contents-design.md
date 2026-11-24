# Release contents and shipped pins — design

Date: 2026-09-24. Branch: `docs/release-contents-20260924` (off `origin/dev`
`19f4759`). Status: approved in chat by the owner, section by section; the owner asked
for it to be built on 2026-09-24. Implemented on `feat/release-contents-20260924`
(plan `docs/superpowers/plans/2026-09-24-release-contents.md`).

Builds on `2026-09-06-per-landing-promotion-design.md`, whose vocabulary
(landing, unshipped landing, release merge-back) this spec keeps.

## Goal

When several landings go out as one release, the owner can see exactly what
the release carries, what it leaves on Dev, and what earlier behavior it
replaces, and nothing leaves a release or undoes shipped behavior silently.

The owner has seen both failures this addresses:

1. **Worked in Dev, not in production.** A change was running in Interface Dev
   and the production update did not carry it.
2. **Shipped, then regressed.** A later update brought old behavior back.

## Evidence (measured 2026-09-24 against `origin/dev` `35a8f33`)

- 66 of the 178 landings since 2026-09-10 were already on Dev when at least one
  staged release shipped without them; some waited four or five releases. None
  is missing from `main` today, so the failure is silent delay, not permanent
  loss. Hand-picked releases (`--change`) skip every landing they do not name,
  and whole-dev releases pinned to an older revision (0.3.117–0.3.123) skip
  every landing after it. Neither case prints or records what it skipped.
- Whole-dev promotion records carry `selected: []`: 0.3.135's contents exist
  nowhere except as a git range, and nothing checks afterwards that each
  landing shipped.
- A landing is a one-line name and a branch. Bookkeeping landings ("Record …")
  are indistinguishable from features in `cargo xtask landings`.
- 24 merges on Dev since 2026-09-10 needed hand conflict resolution. Spot
  checks of those that dropped lines found protocol-version bumps, reformatting
  and deliberate replacements, not a confirmed lost change. Git cannot see
  behavior a later landing undid by rewriting code or its test.

## Non-goals

- Showing release contents in the app or the updater feed. Agent reports
  first; an app view is a later, separately versioned change.
- Backfilling kinds or pins onto past landings.
- Inferring a kind from the diff beyond "docs-only means record".
- A ledger file in the repository or a store outside git. The landing and
  release commits are the ledger.
- Changing how landings merge, `dev-restart`, the gate, packaging, or feed
  staging.

## Vocabulary

- **Kind**: what a landing is — `feature`, `fix`, `look`, `speed`, `internal`
  or `record`. A landing without a `Landing-kind:` trailer is **unclassified**,
  unless it touched only documentation: then it reads as `record`, the kind
  closeout gives such a landing today.
- **User-facing**: kinds `feature`, `fix`, `look`, `speed`, and unclassified.
- **Check**: one owner-readable line saying what the owner should see if the
  landing worked.
- **Pin**: a test named by a landing as proof of its behavior, written
  `<repo-relative path>::<test name>`. The test name is the Rust function name
  or the exact `it`/`test` title string.
- **Shipped pin**: a `Landing-pin:` trailer on any commit reachable from
  `origin/main`. Cherry-picks keep the landing message, so this covers both
  whole-dev and hand-picked releases without a separate list.
- **Superseded pin**: a pin of a landing named by a `Supersedes:` trailer on any
  commit reachable from the commit being checked: the branch `HEAD` for the
  closeout guard, the release candidate for the promote re-check.
- **Left out**: an unshipped landing on the Dev tip pinned at run creation that
  a release does not include.

## 1. Every landing says what it is

    cargo xtask closeout --change "<name>" --kind <kind> --check "<what you should see>" [--pin <path>::<test name>]...

Closeout writes these into the landing merge's trailer block, before the
existing `Landed-from:`:

    Landing-kind: feature
    Landing-check: Dragging a pane shows a floating Swap preview
    Landing-pin: electron/src/renderer/components/TerminalGrid.test.ts::shows the swap preview while dragging
    Landed-from: feat/swap-preview

Rules, checked before the landing commit is built:

| Kind | `--check` | `--pin` |
|---|---|---|
| feature, fix, look | required | at least one |
| speed | required | optional |
| internal | not needed | optional |
| record | not needed | optional |

- No `--kind` and the landing is docs-only (the existing `product_changed`
  test is false): kind `record`, no other flag needed. No `--kind` on any other
  landing: refused, listing the kinds.
- `--check` is trimmed, one line, non-empty, at most 160 characters.
- Each pin's file must exist in the landing tree and its test name must occur
  in that file exactly once; zero or several occurrences are refused with the
  count, asking for a more specific name.
- A resumed landing (`resumed_landing`) keeps the trailers it was written
  with; a retry does not accept different flags.
- `landing_message` gains the trailers; `parse_landings` reads `kind`, `check`
  and `pins` alongside `branch` (new `LANDING_LOG_FORMAT` fields).

## 2. Every release lists what it ships and what it leaves out

### Selection

At run creation `promote` already reads `origin/dev` once. It additionally
lists every unshipped landing up to that tip and splits it:

- Whole-dev at revision R: included = unshipped landings that are ancestors of
  R; left out = the rest.
- Hand-picked: included = the `--change` landings; left out = the rest.

Every user-facing left-out landing must be acknowledged with
`--leave-out "<name or sha>"` (repeatable, resolved like `--change`).
Otherwise promote refuses before preparing anything, naming each landing with
its kind and check. `--leave-out` naming an included landing, or matching
nothing, is refused. `internal` and `record` landings left out need no
acknowledgment and appear as a count.

`--leave-out` selectors join `selectors` in the run record, so the existing
rule "never change the selection during a retry" covers them. Landings that
arrive on Dev while a run is publishing are not part of its selection; the
completion output lists them as "landed during this promotion — next release"
and does not block publication of the verified artifact.

### Record

Promotion records move to schema 2 with three lists, each entry
`{sha, name, kind, check}`: `included`, `left_out`, and `supersedes` (the last
entry `{sha, name, reason, by}` where `by` is the superseding landing). Schema-1
records still read, with empty lists, and resume unchanged.

The bump commit body keeps its `Promotion-run:` marker line, which resume
detection matches exactly, and adds one trailer per landing in the same block:

    Promotion-run: 0.3.136 <approved sha> <created_at>
    Ships: 4650f92 Preview project outputs beside agent terminals
    Ships: 211a968 Restore saved panes after startup retries
    Leaves-out: 9977dc7 Slide the output preview in and out like the agent overview

`Ships:` lists every included landing; `Leaves-out:` lists user-facing left-out
landings. The bump's version-only content check is unaffected.

### What the owner sees

A pure `release_summary` renders, and promote prints it for `--dry-run`, at
run start and at completion:

    Interface 0.3.136 — ships 7 landings
    Features
      4650f92 Preview project outputs beside agent terminals
              check: an Output preview opens beside each agent terminal
    Fixes
      211a968 Restore saved panes after startup retries
              check: panes saved before a restart reopen instead of failing
    Left out — stays on Dev only
      9977dc7 Slide the output preview in and out  (look)
    Replaces earlier behavior
      9bd3b24 Thin caret is the default terminal cursor
              replaced by 2ae6e08 Blue terminal typing caret:
              "the owner chose the blue caret over the thin grey one"
    Also: 2 internal, 3 record

Groups print in the order Features, Fixes, Looks, Speed, Unclassified; empty
groups are omitted. The AGENTS.md promotion step requires agents to show this
summary (from `--dry-run`) to the owner before asking for approval.

### Dev versus production

`cargo xtask landings` becomes that view: unshipped landings grouped by kind,
each with its check line and "waited N releases", where N counts
`chore(release): bump` commits reachable from `origin/main` committed after the
landing. The "touches files with" overlap line stays.

## 3. Shipped behavior cannot be undone silently

### Closeout guard

A new `xtask/src/pins.rs` runs beside the settled-look guard, over the same
pinned `<dev>..HEAD` range. Closeout now fetches `main` with `dev` so the
guard sees current releases. It reads shipped pins from `git log origin/main`,
removes superseded ones, and refuses the push when, for any remaining pin:

- the pinned file is deleted or renamed away,
- the test name no longer occurs in the file, or
- a changed line falls inside the test's span.

The span runs from the attribute (`#[`) and comment (`//`) lines directly
above the name line to the line that closes the test: the matching brace in
Rust, the matching parenthesis of the `it(`/`test(` call in JavaScript and
TypeScript, skipping string contents and `//` comments. A hunk counts when its
old lines intersect the span, or when it inserts after a span line other than
the last, so adding `#[ignore]` between `#[test]` and the function, or turning
`it(` into `it.skip(`, is a change, while a new test appended after the closing
line is not. (Implementation refinement of the approved "next test start" rule,
which would have flagged every test appended after a pinned one.)

The refusal names the pin, its landing (short sha and name) and the trailer
that clears it:

    Supersedes: <landing sha prefix of 7+ hex, or its exact name> — <reason>

The reference must match one shipped landing: a sha prefix, or the full name
compared case-insensitively. One trailer clears every pin of that landing. A
move of a pinned test is a supersede; the moving landing should pin the new
location.

### Promote re-check

Before packaging, promote checks the candidate tree: every shipped pin and
every pin of an included landing, less those superseded in the candidate's
history, must exist with its name occurring exactly once. A failure stops the
run before packaging and names the pin and landing. This covers commits that
reached Dev without closeout.

### Limits

A change that breaks shipped behavior without touching its pin fails that
pinned test, and the gate stops it. The guard covers the other route: editing
the test to agree with the new behavior. Behavior no pin covers is visible only
through the check lines in the summary.

## Error handling

Every refusal names the landing (short sha and name), the rule it broke, and
the exact flag or trailer that satisfies it. Refusals happen before any
commit, push, preparation or packaging, so nothing needs undoing. Git failures
propagate as today.

## Testing

Pure functions in `landings.rs` and `pins.rs`, unit tested on strings like the
existing ones:

- trailer format and parse round-trip, including missing trailers
  (unclassified) and multiple pins;
- kind rules table, docs-only default, check length and line limits;
- included/left-out split for whole-dev and hand-picked selections, and
  `--leave-out` resolution and refusals;
- `release_summary` grouping, ordering and omitted groups;
- test-span detection for Rust and TypeScript, including `#[ignore]`,
  `it.skip(` and a pin at the end of a file;
- `Supersedes:` matching by sha prefix and exact name, and non-matching
  near-misses.

Integration tests on the existing temp-repository fixtures in `closeout.rs` and
`promotion_run.rs`:

- closeout refuses a feature landing without `--pin`, and a pin whose name is
  absent or duplicated;
- a docs-only landing becomes `record` with no flags;
- a whole-dev promotion at an older revision refuses until `--leave-out` names
  each later user-facing landing, and records both lists and the `Ships:` /
  `Leaves-out:` trailers;
- editing a shipped pin's span refuses without `Supersedes:` and passes with it;
- promote stops when a shipped pin is missing from the candidate;
- a schema-1 promotion record resumes.

Only `xtask/` changes, so `cargo xtask check --changed` runs the Rust suite.

## Documentation

- `AGENTS.md` closeout step 1: the new flags and kind rules; promotion step 3:
  show the `--dry-run` summary before asking for approval; left-out landings
  are acknowledged, never dropped.
- `docs/AGENT_RELEASE_WORKFLOW.md`: `--leave-out`, schema-2 records, the
  summary, the pin re-check.
- `xtask/AGENTS.md`, `docs/CODEMAP.md` and `docs/agent-map.json`: the new
  `pins.rs` module.
