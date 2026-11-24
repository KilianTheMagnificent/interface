# Agent workspace implementation plan

Started September 7, 2026. Baseline: dev `62ed4ba` (Interface 0.3.66).

## Product outcome

A persistent agent can use the owner's chosen compatible model and authorized tools,
carry out real work, report results and requests for input, and recover without
replaying uncertain effects. Grok Bot supplies the persistent-agent interaction
reference; Hermes supplies model/tool extensibility patterns. Interface retains its
Rust ownership and established themes. See the companion research report under
`docs/research/agent-workspace-2026-09-07.html`.

## Milestone 1 - usable Agent workspace (current build)

- Keep conversation and composer visible; put progress, files and capability evidence
  in an optional inspector. Preserve approval/stop behavior and explicit navigation.
- Populate agent model selection from the same provider catalog used by launch.
  Preserve custom IDs, allow choosing at launch, reset model/effort on provider
  change, and use model-specific effort choices.
- Show tool calls observed in the selected task and provider-advertised commands;
  do not call them a complete tool inventory. Unknown discovery remains unknown.
- Keep existing agent profile bytes and session assignments intact. This milestone
  adds no new canonical renderer state, tool permissions, model endpoints or secrets.
- Verify with interaction tests, existing source pins, real Electron fixtures and
  required canonical checks. Close out to dev and refresh Interface Dev.

Acceptance: select a provider/catalog model or custom ID, save/reopen profile,
launch through the existing provider path; conversation stays visible while tool
activity updates; inspector does not change tasks; unsupported or unknown
capabilities are described honestly; narrow and dark/light layouts remain usable.

## Milestone 2 - durable Rust agent identity

Owner: interface-settings or a dedicated domain module using interface-storage;
contracts: interface-contracts and Electron shared DTOs; adapter: interface-host.

- Add versioned AgentProfile, AgentRun, tool-policy and profile-assignment contracts.
- Replace the legacy localStorage profile preference envelope with a Rust-owned,
  bounded single-writer store. Import `interface.agentProfiles.v1` exactly once,
  acknowledge success before retiring legacy writes, preserve a recovery copy.
- Add expected revision/CAS semantics, typed failures and events. Agent identity,
  active run and foreground UI selection remain distinct.
- Version protocol changes explicitly and add Rust/TypeScript golden fixtures.

Acceptance: repeat import without duplication; interrupted writes recover;
concurrent edits refuse stale revisions; deleting a profile does not erase provider
history; changing selection cannot move running work or update another profile.

## Milestone 3 - direct model connections and Rust tool loop

Owner: new provider adapter and Rust agent service, driven by existing EngineAdapter
and SessionHandle interfaces. This is required for model independence beyond the
currently installed provider CLIs; it is not implemented by milestone 1's picker.

- Separate backend/provider, endpoint and model ID. Support OpenAI-compatible HTTPS
  first, then provider-native API formats when required; support configured local
  endpoints with explicit transport constraints. Do not silently substitute models.
- Use OS-backed secret references; never serialize credentials into renderer state,
  prompts or argv. Inspect each provider's current official auth terms before adopting
  subscription authentication. API-key and subscription paths are distinct.
- Add bounded streaming parser, tool-call argument assembly/validation, context and
  usage limits, actual model attribution, cancellation and three-state certainty.
- Start with scoped read/list/search tools through the existing Rust workspace rules.
- Before adding an HTTP dependency, present its exact version/features and impact for
  the owner's dependency approval. No new dependency is added by milestone 1.

Acceptance: deterministic local HTTP fixture proves text -> tool call -> tool result
-> final response, fragmented streaming, malformed arguments, refusal, timeout,
unknown delivery, context exhaustion and cancellation. A configured live-provider
run is a distinct gate, with actual tool execution and persisted model identity.

## Milestone 4 - shared tools, MCP, skills and memory

- One Rust tool descriptor drives the model schema and UI: stable name/version,
  input schema, origin, executor, availability reason and effect classification.
- Add MCP stdio then remote HTTP/OAuth; implement discovery refresh, per-server/tool
  filters, least-privilege child environment and bounded result handling.
- Bind approvals to exact run, call ID, tool version and input digest. Enabling a tool
  is not a blanket approval. Denial, revoked access and unknown outcomes are durable.
- Add scoped terminal/file mutation tools, reviewable skill loading, provenance-bearing
  memory edits, rollback, and explicit artifact preview actions.
- Optional Hermes ACP remains a distinct external backend. Probe the installed
  version; its curated ACP tools exclude computer use/scheduling/media capabilities,
  and generic model switching/restore require tested adapter support.

Acceptance: injected/invalid MCP descriptions cannot grant authority; changed tool
schemas invalidate old approvals; concurrent same-name calls retain unique IDs;
revocation stops future effects; restart never replays started ambiguous work.

## Milestone 5 - unattended agents

- Persistent scheduler: time zones, missed-run policy, overlap control, next-run/readback,
  pause/test, run history and bounded retry for definitely-unsent work only.
- Existing attention tray/native notifications consume durable terminal run events.
- Computer preview/takeover is separate from local desktop access. Remote runtimes
  need an explicit per-user/profile isolation design and restart/restore evidence.
- Add inter-agent delegation only after the single-agent lifecycle works. Reuse the
  existing orchestration and collaboration domains rather than inventing a second one.

Acceptance: sleep/wake, missed schedule, process crash and duplicate event delivery
produce one attributable run; local-only mode never claims 24/7 execution; foreground
selection stays unchanged on background completion.

## Release boundaries

This project request authorizes research, planning and beginning implementation.
Completed milestones land on dev with canonical checks and a verified Dev refresh.
Main promotion, production feed staging and production installation remain separate
release actions for this new work.

## Progress

- Research and architecture comparison: complete, with source/runtime limitations
  recorded in the report.
- Milestone 1: implemented. Agent launch defaults no longer inherit another
  profile's model/effort, and creating a profile clears the previous task view.
  Modal picker menus sit above the dialog and are checked with pointer hit tests.
- Owner's September 7 visual refinement: Grok Bot-inspired coloured character
  silhouettes, searchable vertical agent rows with real latest-task previews,
  and task navigation beneath the roster. Existing profile colours select the
  characters, preserving the saved profile format.
- Validation: canonical renderer gate (`cargo xtask check --changed`), 2,559
  unit/source tests, typecheck, release selector checks, Electron interaction
  fixtures and production renderer build. The fixtures cover profile round trips,
  launch form seeding, catalog/custom choices, provider resets, visible approvals,
  stable conversation mounting, and wide/narrow light/dark screenshots. The HTML
  research report also passed wide/narrow overflow and visual checks.
- This milestone did not run a paid/live provider task or the packaged/signing/
  updater gates. Model acceptance and tool execution still use existing adapters;
  new direct model connections and a common Rust tool runtime are future work.
- Milestone 2: done. `interface-settings/src/agents.rs` is the Rust-owned
  single-writer store with a one-time browser import, revision CAS, interval
  routines and run history; the agent teammates work moved it to
  `agents.v2.json` with a one-time v1 migration.
- Agent teammates core (September 15, sub-project 1 of 7): implemented on
  `feat/agent-teammates-core-20260915` from the
  [spec](superpowers/specs/2026-09-15-agent-teammates-core-design.md) and
  [plan](superpowers/plans/2026-09-15-agent-teammates-core.md): home folders,
  one thread per agent, `MEMORY.md`, statuses, and risky-act approvals behind a
  provider approval gate. See [agents](areas/agents.md). The canonical gate,
  live probes and Dev closeout are recorded in [TODO.md](TODO.md).
- Milestones 3-5: remain. Interval routines exist; calendar schedules,
  notifications and the rest of Milestone 5 do not.
