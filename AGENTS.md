# Interface Rust agent rules

Read `~/Developer/Projects/AGENTS.md` for shared workspace rules.
Project rules below take precedence over shared defaults.

## Unfinished work and resuming

`docs/TODO.md` is this project's shared to-do and resume list. Read the relevant
entry before resuming a task. If an older checkout lacks it, inspect the latest
`origin/dev:docs/TODO.md` before creating a competing list. When nothing in an
entry remains open, move it verbatim to `docs/TODO-done.md`. This list is where
this project's handoffs go; the shared `_AI` vault needs no session note for
work recorded here.

Before ending or handing off a task with requested work unfinished, update that
list automatically without waiting for the owner to ask. Record what is complete,
what remains, why work stopped, the branch/revision and relevant files, checks
passed or unrun, any blocker/approval boundary, and the exact next safe step.
Keep unfinished acceptance checks open; a partial implementation is not complete.
Preserve other entries and reconcile concurrent edits. Link the entry in the
final response. This handoff does not replace completing work that can continue
within the existing authorization; respect an explicit stop immediately and
save the handoff when possible.

## Find your way

When you don't already know which files own a change, read `docs/CODEMAP.md`
and run `cargo xtask navigate '<task terms or path>'` from the repository root
(`--json` for machine-readable owners, rules, entry points, tests and
constraints; without Cargo, read `docs/agent-map.json`). Before editing an
area, read its `AGENTS.md`; file references are not guaranteed to be loaded
automatically by an agent harness. `docs/AGENT_TASKS.md` is the brief format
for work handed to another agent or a smaller model. Update the map when moving
an entry point or adding an area; the gate checks it.

This repository is the clean-room Rust port of
`~/Developer/Projects/Interface`.

- Treat the TypeScript repository as a read-only behavioral oracle. Read it to
  answer "what did 0.1.169 do". Never edit it from this repository and never
  overwrite its unrelated working changes.
- The strict Interface 0.1.169 language port is complete and recorded in
  `docs/PARITY_STATUS.md`; product work beyond it is authorized. Preserve the
  accepted parity surface unless a new behavior is deliberately versioned and
  tested.
- First-party domain, persistence, provider, orchestration, terminal, updater,
  and CLI behavior must be Rust. The existing Electron lifecycle/preload shell
  and React renderer are the explicit exception: they may render state and
  translate frozen DTOs, but must not own canonical business logic or stores.
- Preserve accepted persisted schemas. Wire changes require an explicit
  protocol version and cross-language contract tests. Rust and Node must never
  write the same canonical store concurrently.
- Keep provider effects fail-closed: distinguish definitely-unsent from unknown
  outcomes, preserve idempotency, and never replay ambiguous effects blindly.
- `cargo xtask check` is the canonical deterministic gate. Say plainly when a
  live-provider, packaged-app, signing, updater, or installed-runtime gate was
  not run.

## Closeout

**Settled looks.** `docs/SETTLED_LOOKS.md` lists the visual decisions the owner
has made. Changing one — even to something quieter, even with the pin tests
updated — needs the owner's approval in your session. `closeout` refuses a push
that changes a line carrying one of an entry's markers unless a commit in the
push carries `Settled-look: <entry name> — approved by the owner <date>`; add
that trailer only once you have the approval, and update the entry in the same
commit. When you ask, say that a yes also approves recording that trailer, so
the approval in the session covers the commit that records it. A look the owner
settles with you goes into that file before you close out. Read only the
entries whose markers your change touches; closeout names them.

When a task is finished, the session that owns it closes out the completed
work on `dev` immediately, without waiting for the owner to ask. A subagent
hands its work back to the session that started it and closes out only when
its brief says to.
`cargo xtask check` must be green before the work is committed or pushed.
Pushing to `dev` and refreshing the Dev app are the automatic release actions;
promotion to `main` and updater staging are separate, owner-requested steps.

1. Gate, commit, then land and verify Dev.
   `cargo xtask check --changed` is enough for any diff: it runs the suites the
   changed paths need. Rust paths (`crates/`, `apps/`, `xtask/`) run the Rust
   suite and `electron/` the Electron suite; shared contracts, files the other
   side's tests read (`gate::CROSS_LANGUAGE`, e.g. `xtask/src/package.rs`) and
   unclassified paths run both. `cargo xtask check` and `--full` always run
   both suites. Successful suites leave receipts for their exact source bytes
   and the tools that suite runs, including checks made before committing. A version-only application bump preserves
   those receipts; dependency versions do not.

       cargo xtask closeout --change "<one line saying what this lands>" \
           --kind <kind> --check "<what the owner should see>" --pin <path>::<test name>

   Every landing says what it is. `--kind` is `feature`, `fix`, `look`,
   `speed`, `internal` or `record`; a docs-only landing is `record` without
   flags. Feature, fix, look and speed need `--check`, one line for the owner.
   Feature, fix and look need at least one `--pin`, the test that proves the
   behavior (Rust function name or exact `it`/`test` title). Once a landing
   ships, a push that deletes, moves or edits one of its pinned tests is
   refused unless a commit in it carries
   `Supersedes: <landing sha or exact name> — <reason>`; every supersede is
   shown to the owner in the next release summary.

   Closeout fetches and merges dev, checks the integrated source (reusing only
   matching receipts), creates one named first-parent merge with
   `Landed-from: <branch>`, and pushes it. It refuses dirty trees, detached
   HEAD, dev and main. Keep the name meaningful to the owner choosing a release.
   It retries a lost push race with the newly integrated source checked again.
   For product changes it then refreshes Dev and waits for a renderer → preload
   → Rust host read to succeed on a clean checkout containing the landing.
   Docs-only changes skip the refresh. A refresh failure does not undo a pushed
   landing: rerun closeout with the same name, or `cargo xtask dev-restart`.

2. The standalone Dev refresh remains available.

       cargo xtask dev-restart

   Unless Interface Dev is the app in front, the relaunch opens hidden and
   appears when the owner next switches to it, so a landing never pulls Dev
   over the owner's work.
   `/Applications/Interface Dev.app` runs this source checkout with its updater
   disabled. `/Applications/Interface.app` is production. If the Dev checkout
   is missing, recreate it from a plain shell with
   `git -C ~/Developer/Interface-Rust worktree add ~/Developer/.interface-dev dev`.
   Do not repeat this command after a successful closeout refresh.

3. Promotion and production staging still require the owner's approval.

       cargo xtask promote --version <next> --revision <approved-dev-sha>
       cargo xtask promote --version <next> --change "<name or sha>"

   Run the same command with `--dry-run` first. When the owner's request already
   names what ships and the release summary matches it, go ahead; otherwise
   show the owner the summary (what ships, grouped by kind with check lines,
   what stays on Dev, and what earlier behavior is replaced) and wait. Promote
   refuses to leave a user-facing landing on Dev silently; add
   `--leave-out "<sha>"` for one only after the owner agrees to keep it back.
   Never change the approved selection (`--change`, `--leave-out`) during a
   retry. Installing or restarting production is a separate owner-requested
   action, and so is publishing a release to other people's copies
   (`cargo xtask publish-update`, `docs/PUBLIC_UPDATES.md`). Read `docs/AGENT_RELEASE_WORKFLOW.md` before promoting, packaging or
   recovering a promotion; it holds the selection, resume and staging mechanics.

   Run `cargo xtask` commands bare from the repository root, with no `cd … &&`,
   `uptime`, subshell or output redirect around them; use the Bash tool's
   background mode for long runs. The project's permission rules match only
   the bare command.

4. Inspect and report each state separately.

       cargo xtask status --fetch
       cargo xtask status --json
       cargo xtask landings

   Without `--fetch`, refs are explicitly the locally recorded
   remote refs. A last successful restart is not proof that its process is
   still running. Report the dev landing name/range, Dev verification, main
   promotion with its selected landings, and production staging distinctly.

## Public source

This repository stays private: its history, `docs/TODO.md`, `docs/TODO-done.md`,
`docs/research/` and `docs/archive/` (apart from each retired feature's
`README.md`) hold the owner's notes. The public copy is
a separate repository with its own history, written by
`python3 scripts/export_public.py "<public checkout>"` (the paths listed in
that file, as committed; it stops on a personal address, the owner's home
folder path, a dead link in the front files or an unknown key-shaped string).
`README.md`, `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`
and `install.sh` here are the public ones, so write them for strangers, and
keep personal paths and names out of everything outside the private paths.
Committing and pushing the public copy needs the owner's go.
`python3 -m unittest discover -s scripts -p 'test_*.py'` checks the export
tool and the installer.

## Concurrent agents

Several agent sessions usually run at once over this repository, often in
worktrees under `/private/tmp`. Never revert, stash or reformat changes you did
not make. `cargo xtask worktree-prune` lists the worktrees it would remove;
`--apply` removes only those whose branch is merged into `origin/dev`, whose
tree is clean, and that no running process is using; never the Dev, promote
driver or release checkouts, nor a checkout of `dev` or `main`.

The main checkout `~/Developer/Interface-Rust` stays detached at `origin/dev`,
so sessions started there read current rules: a SessionStart hook
(`scripts/follow-dev.sh`) moves it forward when it has no tracked edits. Do task
work in a worktree; edits left in the main checkout stop it following dev.

## Hands-free UI testing

Drive the real app without the owner's mouse, keyboard, focus, clipboard or data. Never point Computer Use, `screencapture`, `osascript` or `open` at Interface, and don't start the installed apps yourself; the Dev relaunch that `cargo xtask closeout` and `cargo xtask dev-restart` perform is the exception. Use the hands-free kit (`npm --prefix electron run handsfree`); read `docs/AGENT_HANDSFREE_TESTING.md` before driving it.

## Known traps

Environment facts that have each cost a release run:

- The repository and the Dev checkout live outside iCloud Drive, in
  `~/Developer/Interface-Rust` and `~/Developer/.interface-dev`;
  `~/Desktop/Projects/Interface-Rust` and `~/Desktop/Projects/.interface-dev`
  are symlinks to them. Keep checkouts, worktrees and build output out of
  `~/Desktop/Projects`: iCloud evicts files there, and git, npm and cargo then
  stall or crash (`mmap failed: Operation canceled`, `rev-list died of signal
  10`) until the file provider downloads them again. The pre-move repository
  is `~/Desktop/Projects/Interface-Rust-icloud-archive`; worktrees made before
  2026-09-23, including `~/.interface-release`, still use its `.git`, and 24
  branches whose commits never reached GitHub exist only there.
- A symlinked `electron/node_modules` breaks vite's postcss resolution. Install
  into the checkout that will run the build.
- A detached process inherits the environment of whoever started it, and an
  agent's environment often lacks `$HOME/.cargo/bin` on `PATH`. `npm run dev`
  falls back to the absolute `$HOME/.cargo/bin/cargo` (and says so when neither
  path exists); every other command that spawns `cargo` still needs `PATH`
  exported first.
