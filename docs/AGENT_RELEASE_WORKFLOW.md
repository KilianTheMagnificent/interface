# Promotion and production staging

Read this before promoting, packaging or recovering a promotion. The root
`AGENTS.md` controls authority: promotion and staging need the owner's approval
for the candidate, and nothing here authorizes an extra action.

    cargo xtask promote --version <next> --revision <approved-dev-sha>
    cargo xtask promote --version <next> --change "<name or sha>"

Omit `--revision` to select HEAD, which must already be on dev and include
current main. `--change` repeats and selects named landings in dev order.
Preparation happens entirely in the managed detached `~/.interface-release`
checkout; the caller's branch and working tree are never switched. The
approved source, selection, version bump, checks, signed package, smoke
result and timings have a durable promotion record. The approved revision
includes authority for its version bump. New versions must exceed the source
version and production feed. A conflicting selection reports overlapping
landings and stops without publishing.

Every promotion splits the unshipped landings on the pinned dev tip into
those it includes and those it leaves out, and prints a release summary:
user-facing landings grouped by kind (Features, Fixes, Looks, Speed,
Unclassified) with their check lines, what stays on Dev, what earlier behavior
the included landings supersede, and counts of internal and record landings.
A user-facing landing left out without `--leave-out "<name or sha>"` is
refused, after the summary prints, so `--dry-run` is the approval view.
`--leave-out` naming an included landing or nothing is refused. Landings from
before kinds are unclassified unless they touched only documentation, which
makes them `record`. The schema-2 promotion record keeps `included`,
`left_out` and `supersedes`, and the bump commit carries one `Ships:` trailer
per included landing and one `Leaves-out:` per user-facing omission under its
`Promotion-run:` marker. Schema-1 records still resume. Before packaging,
promote checks that every shipped pin and every included landing's pin still
names exactly one test in the candidate (superseded ones excepted); a
selective release that leaves out the landing adding a test an included
landing pins stops there.

Verified artifacts are archived outside iCloud under `~/.interface-pipeline`.
All coordinated packaging, promotion and manual staging share one writer
lock. Promotion integrates the candidate into the latest dev and validates
that tree, then pushes main and dev atomically as fast-forwards. Concurrent
landings retry integration with the same approved candidate and artifact.
It stages the feed at `~/Library/Application Support/Interface/update-feed`,
verifies the manifest and artifact bytes, and nudges the running app to
check immediately. Installing or restarting production remains a separate
owner-requested action.

Putting a promoted release in front of copies on other people's Macs is a
third, separate owner-requested step: `cargo xtask publish-update`, described
in [Public updates](PUBLIC_UPDATES.md). Promotion never does it.

`--dry-run` reads preconditions and prints the pinned plan. `--no-push`
prepares and verifies the candidate, leaving refs and feed alone. Rerun the
same command without `--no-push` to publish the exact verified bytes.
Both whole-dev and selective promotions resume from their recorded work,
even after refs were pushed but feed staging failed. Never change the
selection during a retry or use `--allow-same-version` as a recovery shortcut.
`--release-worktree <path>` selects a different managed checkout for a new
run; a resumed run retains its original worktree.

## Known traps

Release-only environment facts; the shared ones stay in the root `AGENTS.md`.

- `cargo xtask package-mac` refuses a checkout under `/private/tmp`: the
  builder's private temp parent would overlap the project. Package from the
  release worktree, not from an agent worktree in `/private/tmp`.
- `promote` seeds a new release worktree from the local checkout (`git worktree
  add --no-checkout`, `rsync`, `git reset`) and retries a cancelled mmap up to
  three times, from when the repository lived under iCloud Drive. Reuse
  `~/.interface-release` for its `node_modules`; `promote` reinstalls only when
  the lockfile actually changed.
