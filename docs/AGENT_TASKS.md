# Bounded agent tasks

This guide is for work handed to another agent or a smaller model; a lead agent
that already knows where its change lives does not need it. It provides context
selection and checks, not permission to expand a task.
Root and scoped `AGENTS.md` files retain authority. Model quality is evaluated
per task; repository structure alone cannot establish that a model is reliable.

## Find the change

1. From the exact checkout, run `pwd` and `git status --short --branch`.
   Preserve concurrent changes; use a task branch/worktree for implementation.
   Read the root rules and [CODEMAP.md](CODEMAP.md).
2. Run `cargo xtask navigate` to list areas, then query a route id, short terms
   or a repository path. Exact ids come first; other queries require every
   whitespace-separated term. Matching is case-insensitive substring search,
   not semantic search. Unknown JSON queries return an empty `matches` array.
3. Read the returned `rules`, the relevant `guides`, the first entry point and
   the relevant existing test. All paths and check commands use repository-root
   coordinates; commands requiring another working directory say so explicitly.
4. Find the relevant symbol, inspect its callers and tests, and expand only where
   the behavior crosses a boundary. Stop reading when the owner, observable
   behavior, edit scope and acceptance check are clear.

```sh
cargo xtask navigate 'slash help'
cargo xtask navigate 'sidebar pinning' --json
cargo xtask navigate crates/interface-remote/src/http/direct.rs
rg -n 'parse_argv|CLI_USAGE' apps/iface/src/argv.rs
rg -n 'projectPinned|pinnedProject' electron/src/renderer/viewmodels.ts
rg --files crates/interface-terminal electron/src/renderer | rg 'title|launch|test'
```

If Cargo is missing from PATH, use `export PATH="$HOME/.cargo/bin:$PATH"`.
If the tool cannot build, read [agent-map.json](agent-map.json) directly and search
its `id`/`keywords`. No embedding service, network request or new dependency is
needed. Navigation prints suggested checks; it does not execute them, start a
provider, alter settings or grant filesystem authority. The initial Cargo build
writes normal local build output.

Use `rg` over the selected source directories. Avoid broad searches through
generated output, `.worktrees`, `target`, `node_modules`, runtime data or archived
research. Never read secrets. A whole-file read of `App.tsx` or `tui.rs` is usually
unnecessary: search the symbol, read a bounded range and follow imports.
Zero callers do not prove a retained protocol surface is removable.

## Choose a small task

| Suitable bounded task | Starting route | Acceptance |
| --- | --- | --- |
| Documentation/path repair | `pipeline` | Real path/link checks and navigation gate |
| Display label or pure grouping fix | `sidebar`, `conversation` | Existing helper test with a concrete before/after example |
| CLI help or slash-parser fix | `cli` | Grammar tests; preserve exit codes; PTY test when lifecycle changes |
| Pure timestamp/sentence helper fix | `interface-common`, `interface-voice` | Boundary cases and the owning crate's tests |
| Add coverage for already specified behavior | Owning route | Test fails on the known defect and passes on the intended behavior |

Keep persistence migrations, provider delivery/retry, authorization, shared wire
contracts, PTY lifecycle, broad `App.tsx`/`tui.rs` refactors and release coordination
with an experienced integrator. A small diff in one of these areas can still have
large consequences. UI work must respect [SETTLED_LOOKS.md](SETTLED_LOOKS.md).

When handing a task to another agent or model, give it a short task packet:

```text
Outcome: one observable behavior, with a concrete input and expected result.
Checkout/base: exact path, branch and revision.
Edit scope: exact files or one owning area; follow imports read-only as needed.
Context: applicable rules, route, existing implementation and nearby test.
Preserve: the specific API, persisted shape, visual decision or effect boundary.
Acceptance: focused command, regression/example, required final gate.
Return: changed files, evidence, remaining uncertainty and any scope blocker.
```

If the required fix crosses the assigned scope, report the additional owner/files
and why they are needed; do not quietly widen authority. For an unfamiliar helper,
first reproduce the input/output defect. Tests should exercise behavior, not merely
assert that the implementation contains a chosen string.

## Verify and finish

For unfinished work, update [TODO.md](TODO.md) before the final response or
handoff, following the root rules. Include the verified stopping point, remaining
work, blocker and exact next safe action; retain unrun acceptance checks as open.
Read that entry when resuming, then verify current source/runtime state.

Run the route's focused check during iteration. Renderer interactions need the
real DOM lane: `npm --prefix electron run test:renderer`. A pure Vitest or static
markup assertion alone does not establish focus, geometry or xterm lifecycle.
`electron/scripts/renderer-lanes.cjs` lists the isolated lanes and their flags.
Native phone testing requires the iPhone README's Xcode/Simulator steps.

Run the gate required by root rules: `cargo xtask check --changed` for eligible
single-sided work; `cargo xtask check` for shared contracts, xtask or both sides.
Navigation validation always runs, including docs-only changes. It checks map
structure, existing non-symlink paths, scoped rules and Cargo-member coverage;
it does not prove prose, test-command semantics or model behavior.

After required checks pass, commit only the task's files on its branch and use
the root `AGENTS.md` closeout procedure. Dev landing and running Dev verification
are distinct. Production promotion, staging, installation and restart keep their
separate owner authorities. Report exact checks and omissions.

## Keep navigation current

`docs/agent-map.json` is the one machine-readable routing source (schema v1).
Each route has an id, short summary/keywords, owning roots, rules, guides,
entrypoints, tests, focused checks and constraints. Add a route when adding a
Cargo member or a separate UI/platform surface; update it when a named file moves.
Use real paths, not line numbers, globs or symbol fragments. Tests may live inline
in Rust source. Keep focused check commands explicit and runnable from the root.
Run `cargo xtask navigate --check --json` to check drift without full compilation
of the product. [The area docs](CODEMAP.md#read-next) retain detailed invariants;
the map selects them rather than duplicating them wholesale.

Start a fresh harness session and verify it has read the root and scoped rules
before judging instruction behavior. An `AGENTS.md` reference is not proof of
automatic loading. OpenCode documents native AGENTS.md support, CLAUDE.md fallback
and explicit loading of references. Prefer the shared rule files over a second
model-specific policy. [OpenCode rules](https://opencode.ai/docs/rules/).
