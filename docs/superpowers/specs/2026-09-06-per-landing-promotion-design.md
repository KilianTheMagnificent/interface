# Per-landing promotion — design

Date: 2026-09-06. Branch: `feat/per-landing-promotion` (main checkout, off
`origin/dev` `a79e392`). Status: approved in chat by the owner, section by
section; not yet implemented.

## Goal

The owner ships one change at a time from `dev` to `main` while other changes
keep evolving on `dev`, and every change that lands on `dev` is named at
landing time so it can be picked later. Telling an agent "ship X" is one
command. `dev` keeps everything, shipped or not, so new work keeps building on
X after X ships.

Today every closeout fast-forwards `dev` to the agent's branch tip (the merge
of `origin/dev` happens on the agent's side), so landings leave no first-parent
unit to pick, and `promote` requires `HEAD == origin/dev` and pushes it to
`main`, so `main` can only ever take all of `dev`.

## Non-goals

- Rewriting `dev`, rebasing agents' branches, or making agents cut branches
  from `main`. Agents keep building on unshipped work.
- A changelog file or ledger outside git. The landing commits are the ledger.
- Resuming a selective release that died mid-way. It is rebuilt from scratch;
  only the gate and packaging are expensive and they rerun regardless.
- Changing `dev-restart`, `check --changed`, `worktree-prune`, the gate stamp,
  the release worktree, or feed staging.

## Vocabulary

- **Landing**: one merge commit on `dev` produced by `closeout`, first parent
  the previous `dev` tip, second parent the agent's branch tip, subject the
  owner-readable description, trailer `Landed-from: <branch>`.
- **Unshipped landing**: a landing in `origin/main..origin/dev` whose SHA does
  not appear in any `(cherry picked from commit <sha>)` line on `origin/main`.
- **Release merge-back**: the merge of `origin/main` into `dev` after a
  selective release; a first-parent merge on `dev` carrying `Release: <version>`
  and no `Landed-from:`, so it is never listed as a landing.

## Workflow

1. Land. `cargo xtask closeout --change "Saved workspaces file under their own
   project"`. Refused without `--change` unless the branch adds exactly one
   non-merge commit over `origin/dev`, whose subject then becomes the name.
2. Menu. `cargo xtask landings` prints every unshipped landing: short SHA,
   date, name, `Landed-from` branch, and "touches files with: <names>" when
   another unshipped landing changes a common path.
3. Ship one or several. `cargo xtask promote --version 0.3.66 --change "<name
   or SHA>" [--change …]`. The release gets the full gate and the packaged
   smoke, `main` and the feed move, then `dev` merges `main` back.
4. Ship everything. `cargo xtask promote --version …` with no `--change` is
   today's fast-forward of all of `dev`, preconditions and resume rules intact.

## Mechanics

### Closeout

Guards as today (clean tree, on a task branch, not `dev`/`main`, settled-look
trailer), plus the `--change` rule. The text is trimmed, must be one line,
non-empty, and at most 100 characters.

Steps, per attempt (two attempts, as today):

1. `git fetch origin dev`, then read the dev tip once (`git rev-parse
   origin/dev`) and use that SHA for every later step of the attempt:
   `refs/remotes/origin/dev` is shared with every other worktree of the
   repository, and another agent's fetch may move it mid-run. `git merge
   --no-edit <dev>` into the branch, leaving conflicts in the agent's tree
   exactly as now.
2. Settled-look guard over `<dev>..HEAD`, as now.
3. If `git rev-list --count <dev>..HEAD` is 0, refuse: "nothing to land".
4. Build the landing without touching the working tree, and only while `<dev>`
   is still an ancestor of `HEAD` (`git merge-base --is-ancestor`), so the
   first parent is always a commit the branch really merged:
   `git commit-tree HEAD^{tree} -p <dev> -p HEAD -F <message>`.
   Message: subject = the change text; blank line; body = one line per
   non-merge commit subject in `<dev>..HEAD`; blank line; trailer
   `Landed-from: <branch>`.
5. `git push origin <landing>:dev`. On the first attempt, either race —
   a non-fast-forward rejection, or a `<dev>` step 4 found the branch no longer
   contains — refetches and repeats from step 1; the two share the two
   attempts, and the second loss says to rerun.
6. `git update-ref refs/heads/<branch> <landing> <branch tip>` so the branch
   sits on the pushed landing (its tree is already the working tree) and a
   checkout switched mid-run is refused instead of dragged along. Print the
   pushed range and the name.

Because the branch already contains `<dev>` after step 1, the landing's
`-m 1` diff is exactly what the branch adds, and `git log --first-parent
--merges origin/main..origin/dev` lists landings in dev order.

### Landings command

1. `git fetch origin`.
2. Candidates: `git log --first-parent --merges origin/main..origin/dev
   --format=…` with body, kept to commits whose trailers include `Landed-from:`.
3. Shipped set: every `<sha>` in a `(cherry picked from commit <sha>)` line of
   `git log origin/main --format=%B`. The whole of `main` is scanned once per
   invocation; a `--since` bound would be fragile and the history is small.
4. Unshipped = candidates minus shipped. Paths per landing: `git diff
   --name-only <sha>^1 <sha>`. Overlap = shared paths between two unshipped
   landings.
5. Print oldest first: `abc1234  2026-09-06  Saved workspaces … (fix/workspace
   -folder-grouping)`, then an indented "touches files with: …" line when
   applicable. With no unshipped landings print "dev has nothing main lacks".

### Selective promote

Preconditions: clean tree; `git fetch origin`; requested version strictly newer
than `electron/package.json` at `origin/main` and than the feed (same rule as
today, read from `origin/main` instead of the tree); each `--change` resolves
to exactly one unshipped landing by unique SHA prefix or unique
case-insensitive substring of the subject, else refuse naming the candidates.

Steps:

1. Record the current HEAD (branch or detached) to restore on failure.
   `git checkout --detach origin/main`.
2. For each selected landing, oldest first: `git cherry-pick -x -m 1 <sha>`.
   On conflict: `git cherry-pick --abort`, restore HEAD, refuse with the
   landing's name and the unshipped landings that share a path with it,
   suggesting `--change` for them or a ship-all promote.
3. Bump as today; the bump body reads "Promoted landings <short shas> from dev
   by cargo xtask promote."
4. Full gate (`check --full` semantics; a fresh tree has no stamp).
5. Package and smoke in the release worktree, seeded from the promoted HEAD as
   today.
6. `git push origin <promoted>:main`, guarded by `origin/main` being an
   ancestor of it (it is, by construction, unless main moved; then refuse and
   leave the local commits for a rerun).
7. Merge-back: `git checkout --detach origin/dev`; `git merge --no-ff
   --no-edit -m "Release <version> merged back to dev" -m "Release: <version>"
   origin/main`; `git push origin HEAD:dev`, with closeout's one-retry race
   handling. A conflict here leaves `main` pushed: abort the merge, restore
   HEAD, and print the merge and push commands plus the `stage-update` command.
8. Stage the feed as today, then restore HEAD.

`--dry-run` resolves the landings and prints the plan without changing
anything. `--no-push` stops after step 5 and prints steps 6–8 as commands.

Ship-all promote is unchanged: `HEAD == origin/dev`, `origin/main` an
ancestor, stamp skip, resume classification, push dev then main.

### Code placement

- `xtask/src/landings.rs` (new): landing message assembly, the `--change`
  rule, parsing landings from log output, the shipped-set filter, overlap,
  `--change` resolution, and the cherry-pick loop. Pure functions take strings;
  thin wrappers run git.
- `xtask/src/closeout.rs`: the landing build replaces the branch-tip push.
- `xtask/src/promote.rs`: a selective branch in `promote()` gated on
  `--change`, reusing `bump`, `run_gate`, `package`, and the feed staging.
- `xtask/src/main.rs`: `landings` subcommand; `closeout --change`; `promote
  --change` (repeatable).

## Error handling

Refusals keep the `closeout refused: …` / `promote refused: …` style and say
what to do next. Every failure after `main` is pushed prints the exact commands
that finish the release, as the current recovery messages do. No step rewrites
`dev` or `main`; all pushes are fast-forwards.

## Testing

Unit tests in the existing xtask style (pure functions, no git): the
`--change` rule, landing message assembly, landing parsing and the shipped-set
filter, overlap detection, `--change` resolution (unique, ambiguous, unknown,
empty), dev-order sorting.

Two temp-repo integration tests with a bare origin. The first: closeout
produces a first-parent landing whose `-m 1` diff equals the branch's changes
and moves the branch onto it; `git cherry-pick -x -m 1` of that landing onto
`main` reproduces the change and records the source SHA; the landings parser
then reports it as shipped. The second, over two checkouts of one origin: a
landing is never written on a dev tip the branch has not merged, so a peer's
landing cannot be reverted by the next closeout.

Gate: `cargo xtask check --changed` on the xtask crate.

## Rollout

- AGENTS.md Closeout section: `--change`, `cargo xtask landings`, the "ship X"
  form of promote, and the report line naming the shipped landings.
- `docs/areas/xtask.md`: same.
- Branches in flight keep working. `cargo xtask` builds from the branch's own
  `xtask/`, so a branch cut before this lands rejects `--change` as an unknown
  flag: run `git fetch origin dev && git merge --no-edit origin/dev`, then
  rerun with `--change`. Codex, Grok and Antigravity read the same AGENTS.md
  and the refusal text.
- This change lands through the new closeout as the first landing.
