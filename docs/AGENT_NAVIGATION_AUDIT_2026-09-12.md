# Agent navigability audit — September 12, 2026

## Verdict and scope

The repository has strong ownership boundaries and extensive tests, but an agent
still has to connect prose architecture, exact files, scoped instructions and
test commands manually. The highest-value improvement is a checked task router
and a short working procedure. Broad source refactoring is a poor first step:
large central files contain contract and lifecycle coupling that cannot safely
be removed just to make them shorter.

The audit covered repository layout, all 23 Cargo workspace members, the Electron
main/shared/renderer/remote surfaces, both native Swift shells, release tooling,
instruction entry points, area documentation, test discovery and common task
paths. It examined module/API boundaries and representative source/test pairs.
It is a full repository navigation audit, not a line-by-line correctness,
security or live-provider audit of every source file.

Source baseline: fetched `origin/dev` at `d200d74`, isolated branch
`docs/agent-navigation-20260912`. The canonical checkout remained on `main`
at `e594661` with pre-existing changes. Its instruction reductions, CLAUDE imports,
reference files, settings and duplicate files were not swept into this landing.
The reference docs in that checkout are not automatically present on Dev.

## Measured baseline

Counts come from tracked paths at the source baseline, excluding build output.
Source line counts cover `.rs`, `.ts`, `.tsx`, `.cts`, `.mjs`, `.cjs`, `.swift`,
with sensitive-shaped paths excluded. They are approximate context-size proxies,
not model token measurements.

| Measure | Before this change |
| --- | ---: |
| Tracked files | 918 |
| Source files in the above extensions | 736 |
| Source lines | 321,840 |
| Cargo workspace members | 23 |
| TypeScript/TSX test files | 143 |
| Swift source files | 20 |
| Tracked scoped/root AGENTS.md files | 5 |
| Root AGENTS.md size | 10,481 bytes |
| Renderer area guide | 428 lines |
| Persistence/services area guide | 424 lines |

Large navigation choke points include `electron/src/renderer/App.tsx` (7,035
lines), `apps/iface/src/tui.rs` (6,510), `components/Conversation.tsx` (4,527),
host `collaboration.rs` (4,417), collaboration `state.rs` (4,111), and host
`terminal.rs` (3,850). The 7,516-line terminal integration test file also requires
symbol-based lookup. These sizes do not by themselves establish defective design.

## Findings and decisions

| Priority | Finding and evidence | Implemented response |
| --- | --- | --- |
| High | No machine-readable task → owner → rules → test route; `CODEMAP.md` mostly names areas | `docs/agent-map.json` plus Rust `cargo xtask navigate`; queries return exact paths, focused checks and constraints |
| High | Root map omitted `interface-remote`, native iPhone and Apple voice helper; agent routines and Away had weak front-door discovery | Added map entries and a remote/native area guide; every Cargo member has checked coverage |
| High | Ordinary docs changes were classified as no gate work in `xtask/src/gate.rs` | Navigation validation runs before receipt reuse/suite selection, including docs-only `--changed` |
| High | README said host protocol v5; current checked facts/source say v19. Contracts guide said 117 channels; source says 119. Desktop services prose said 22 operations; table has 23 | Replaced mutable copies with source/checked-facts references; removed stale numeric descriptions |
| Medium | Host instructions/guide named nonexistent `tests/session_management.rs` | Corrected to `tests/suite/session_management.rs`; router validates actual test paths |
| Medium | No tracked area rules for native shells or xtask | Added short scoped instructions with ownership and exact relevant checks |
| Medium | Large central files and 400+ line area guides encourage whole-file context loading | Task routes start at focused helpers/tests; guide teaches symbol lookup and caller-driven expansion |
| Medium | Background delegation guide had a disabled banner followed by present-tense workflow instructions | Explicitly labelled the following workflow as retained, opt-in documentation rather than current audit instructions |
| Medium | File references can be mistaken for automatically loaded instructions | Root task entry and guide explicitly require reading returned rules; validator catches omitted ancestor AGENTS.md files |
| Medium | Unit tests, real renderer tests, native tests and release/runtime evidence are easy to conflate | Task guide separates focused iteration, required final gate, UI/native evidence and release authority |
| Follow-up | Committed root/scoped instruction bulk overlaps a pending local consolidation | Preserved that work; added an early navigation entry without replacing its policy or landing its uncommitted changes |

## Coverage by area

| Area | Navigation assessment and action |
| --- | --- |
| contracts | Clear DTO ownership; corrected stale counts; route names full cross-language constraint |
| common | Good small helper surface; explicit entry/test route for time/path work |
| engine/runtime/orchestration | Useful area guide; routes keep certainty, live authority and event replay boundaries visible |
| Codex/Claude/ACP adapters | Clear provider owners but large adapter files; routes start at lib/mapper and paired adapter tests; protocol differences retained |
| collaboration | Journal/coordinator separation is useful; Away and retained delegation now have distinct task routes |
| storage/registry | Strong atomicity and golden-format constraints; routes expose them before seemingly small edits |
| settings | Presentation and persistence can be confused; route sends saved pins/appearance to Rust and display grouping to renderer |
| review/workspace/prompt context | Focused modules already suitable for bounded tasks; routes state path authority and compare-and-save boundaries |
| terminal | High-coupling lifecycle implementation with large tests; separate Rust lifecycle/title and mounted xterm presentation routes |
| desktop services/updater | Stale operation count removed; routes keep typed effect errors and production authority visible |
| host | Adapter-only rule is clear; corrected relocated test reference and indexed actual owner files |
| iface | Pure argv and slash parsing provide small entry points before the large TUI controller; preserve real PTY checks |
| Electron main/shared | Frozen channel mirrors and sandboxed preload require coupled checks; route lists concrete encoders/tests |
| renderer | Existing pure helper modules and colocated tests are a strength; separate sidebar, conversation, composer and terminal routes |
| agent routines | Existing guide was disconnected from front-door map; route ties Rust store/host and panel/tests together |
| phone/remote/relay | Previously missing top-level coverage; added end-to-end area map and native instructions; no deployment performed |
| voice/native helper | Added Swift adapter instructions and separate Rust/helper routes; actual speech remains a runtime gate |
| xtask/scripts/CI | Existing deterministic gate and discovery checks are strong; added always-run navigation drift validation; manual CI trigger retained |

Existing strengths worth preserving: Rust-owned authority, frozen contracts with
cross-language fixtures, pure renderer helpers, nearby tests, real renderer/PTY
lanes, exact-source gate receipts, and explicit release-state separation. This
change does not relocate product logic, rename exported APIs or edit settled looks.

## Deliverables and verification contract

- `cargo xtask navigate` lists route ids and summaries.
- `cargo xtask navigate 'sidebar pinning' --json` returns a versioned task packet.
- `cargo xtask navigate --check --json` verifies every referenced path, required
  metadata, ancestor rules and workspace-member coverage.
- `docs/AGENT_TASKS.md` gives a small-task workflow, task packet and test selection.
- Root/README/CODEMAP links make these reachable in a fresh committed checkout.

The 33-route manifest is deliberately queried, not injected wholesale. The
`sidebar pinning` JSON packet measures 1,077 bytes in this revision (metadata only;
the agent must still read its rules and relevant source). This is not a measured
total-context or token reduction. The tool
reads only the fixed map and path metadata; the coverage check additionally runs
offline locked Cargo metadata. Suggested check commands are text, never executed
by navigation. It refuses malformed schemas, missing entries, secret-shaped paths,
path escapes, symlinks and special files. Normal lookups revalidate paths without
running Cargo metadata. This is repository tooling, not an authorization sandbox.

Regression cases cover ten representative symptom queries (sidebar pins, PTY
launch, phone pairing, slash help, speech chunks, atomic writes, release receipts,
Away stop, window CSP and streaming transcripts), route ids/paths, no-match behavior,
deleted files, missing Cargo coverage, omitted scoped rules, schema drift and unsafe
paths. Required final validation is the full `cargo xtask check` because xtask changed;
actual command results and Dev state are reported in the task closeout.

The validator proves navigation structure and path existence. It cannot prove
that prose remains accurate, a referenced test has adequate coverage, or a focused
command is sufficient for a new behavioral change. Updates still require review.

## DeepSeek and GLM: evidence limits and next evaluation

The named versions have official pages:
[DeepSeek V4.1 Flash](https://www.deepseek.com/en/news/deepseek-v4-1-flash/) and
[GLM 5.3](https://docs.z.ai/guides/llm/glm-5.3). Their advertised abilities are not
evidence of performance inside this repository. No comparative provider run or
measured token/cost improvement was performed for this audit.

Instruction discovery belongs to the harness. OpenCode documents AGENTS.md support,
CLAUDE.md fallback and explicit reference loading; the new entrypoint follows that
shared-file approach rather than adding separate model policies.
[OpenCode rules](https://opencode.ai/docs/rules/).

For a later model evaluation, pin the same checkout, harness/version, model id,
reasoning setting, tool budget and task prompt. Start fresh sessions on six tasks:
locate sidebar pin grouping; locate slash-command parsing; locate terminal title
precedence; locate sentence chunking; trace a phone approval; repair one known
pure-function defect in a disposable worktree. Use the same tasks with and without
the routing packet. Score correct owner, relevant test selection, forbidden-scope
changes, regressions, completion, tool calls, context bytes, elapsed time and
provider-reported tokens/cost when available. Preserve actual model identity when
reported; do not silently substitute a different model. Repeat tasks to estimate
variance and have the integrator review resulting patches.

Until that is run, use these models for individually bounded tasks with explicit
acceptance. Do not infer autonomous reliability in persistence, provider effects,
authorization or release work from a navigation test passing.

## Deferred work

Split `App.tsx`, `tui.rs`, adapter or coordinator files only during a scoped change
with an identified seam and behavior tests. A mass move would impose merge churn
and context relearning without demonstrated task success. Continue the pre-existing
instruction consolidation separately so its preserved compatibility references and
fresh-session loading proof can be reviewed together. Historical plans remain
historical; do not treat them as current implementation or release state.

## Validation performed

Implementation source `ab857d7` passed the full `cargo xtask check`: 1,510 Rust
tests passed with 18 intentionally ignored; 143 TypeScript test files / 2,684
tests passed; Swift buffer tests, real renderer fixture lanes, type checks and
production builds passed. The first attempt was blocked by sandbox access to the
Swift cache and localhost renderer listener; the complete rerun with the necessary
local access passed. The 79-test xtask suite and focused Clippy check also passed.

`navigate --check --json` confirmed 33 routes and 23 Cargo members. Changed
Markdown relative-link inspection found 34 links and no missing targets. All 18
pre-existing non-secret source/instruction paths hashed in the canonical checkout
remained unchanged. The pre-existing Claude settings edit was not read or changed.
No live DeepSeek/GLM sessions, packaged-app/signing/updater, native iPhone or
production installation/restart gates were run. Dev closeout/runtime state is
reported separately because a passing source gate does not establish it.
