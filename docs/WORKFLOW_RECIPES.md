# Project setup and workflow recipes

The new-thread card's **Setup & recipes** disclosure saves project configuration
in the host-owned `workflows.v1.json` sidecar. Old `threads.v3.json` and settings
formats are unchanged. There is no inferred setup command and no default package
installation.

Save setup explicitly: prerequisite commands run first, followed by setup commands,
only when Interface creates a new worktree. Each line is an owner-authored shell
command. Commands run in that worktree, with no stdin, under one total deadline
(1–600 seconds, 120 by default; at most 12 commands). Known provider and token
variables are removed from inherited environment; no secret files are copied.
Output is bounded and credential-filtered. Nonzero exit, unavailable executable,
output overflow, timeout, or a changed worktree identity stops the sequence and
prevents a prepared launch token. A successful setup is checkpointed before the
provider starts, so setup files are not attributed to its first turn.

A failed worktree stays on disk for inspection. Its result and logs survive an app
restart. Selecting it as an existing worktree remains blocked until the owner
uses **Run saved setup again in this worktree** and the saved configuration passes.
Only a recorded failed/interrupted worktree can be retried, and a session using it
blocks retry. An interrupted host records interruption on reopen and never replays
the commands. Local checkouts and ordinary existing worktrees are not set up just
by selecting them. Ready means the configured commands passed; it does not assert
that unconfigured dependencies exist.

Recipes store a prompt, provider/model, project-relative file/folder or HTTPS
context descriptors, Local/New worktree choice and base branch, a setup snapshot,
expected outputs and required checks. **Use recipe & setup** explicitly applies
that recipe's setup to this project and fills the draft; **Start** remains the
launch action. Expected outputs/checks become clearly labelled prompt instructions,
not claims that verification has run. The ordinary context backend still validates
and authorizes every file read at launch. Temporary pasted images, diff snapshots
and prior-chat attachments are not reusable recipe context. No cached file bytes
or provider credentials are stored in a recipe.

Storage is leased to the Rust host. A corrupt workflow file fails closed rather
than quietly pretending no setup was configured. There are at most 128 projects,
32 recipes per project, and 64 retained setup results per project; completed setup
results can age out, unresolved results never do. The complete document is bounded
at 8 MiB. Saves are atomic and an uncertain write blocks further effects until
restart.

Protocol 49 adds nested `workflow` commands to `threadControl`, plus optional
`workflow` and `setup` response fields. Rust and TypeScript share
`electron/src/fixtures/workflows-v49.json`. Rules/persistence live in
`interface-settings/src/workflows.rs`, execution in
`interface-review/src/workspace_setup.rs`, and the host adapter in
`apps/interface-host/src/threads/workflows.rs`. The renderer's `WorkflowControls`
only draws these DTOs and submits explicit commands.

Checks:

- `cargo test -p interface-settings workflows`
- `cargo test -p interface-review workspace_setup`
- `cargo test -p interface-host setup_`
- `cargo test -p interface-contracts shared_v49_workflows`
- `cd electron && npx --no-install vitest run src/shared/workflows.test.ts src/shared/threads.test.ts src/renderer/thread-card.test.ts`
- `INTERFACE_RENDERER_LANE=workflows,threads npm --prefix electron run test:renderer`
- `cargo xtask check` on the integrated change before landing.

Recipe file and folder attachments resolve to the original project files, including local uncommitted documents, just like explicitly selected prompt attachments. They are reauthorized at launch. Setup commands execute in the prepared worktree; selecting a base branch does not change the attached files.
