# Port parity status

This is the acceptance ledger for the strict Rust port of Interface `0.1.169`.
It records the `0.2.0` code boundary; it does not authorize publication,
installation, or new BridgeMind-inspired product behavior.

| Surface | State | Acceptance evidence |
| --- | --- | --- |
| Shared DTOs and 105 baseline desktop channels | verified | Historical strict-port baseline: Rust golden/strict-decoder tests; Electron and host independently freeze 98 request plus 7 push channels; private native operations are excluded from that count |
| Owner-private storage and registry | verified | atomic/CAS/lock/quarantine/adversarial suites; current Interface data opened from a disposable copy without changing canonical bytes |
| Plans, orchestration, collaboration, review, checkpoints, settings, and workspace | verified | every public operation has one exact Rust owner; cold-start workspace authority, archive/forget narrowing, project-root projection, and all durable stores are exercised |
| Claude, Codex, Grok, Qwen, and configured ACP providers | verified | deterministic wire/lifecycle/history suites plus isolated live sentinel sessions for all five configured provider paths |
| Rust host protocol and runtime | verified | all 98 requests route or fail by their frozen contract; all 7 push families, readiness, bounded admission, response fairness, event ordering, terminal eviction, crash visibility, and shutdown drain are covered |
| Retained Electron/React frontend | verified | 96 Vitest files / 2,120 tests, 14 release-policy tests, both TypeScript checks, production Vite build, and packaged renderer smoke |
| Electron compatibility boundary | verified | only lifecycle, BrowserWindow/preload transport, native dialogs/theme observation, and updater effects remain in TypeScript; canonical policy/state stays in Rust |
| `iface` commands, TUI, PTY, and control socket | verified | 101 command/TUI tests and 4 real-PTY tests cover text/JSON/exit codes, normal/narrow rendering, signals, socket ownership, cleanup, and provider control |
| Packaging and signing | verified | fresh release Rust binaries, signed app, DMG, ZIP, manifest, fuses, embedded-binary identity, archive integrity, and ZIP-extracted CDP smoke all passed in `/private/tmp` |
| Installed deployment/update exercise | not performed | real updater feed, `/Applications` install, running-byte replacement, restart, and rollback are deployment effects rather than missing port code |

## Final acceptance snapshot — 2026-08-27

- `cargo xtask check` passed: formatting, strict workspace Clippy, 665 Rust
  tests, doc tests, and the complete Electron check.
- The one explicitly ignored live migration test was run separately against a
  stopped legacy Interface data root and passed, bringing the exercised Rust
  test count to 666.
- Claude, Codex, Grok, local Qwen, and the configured Gemini ACP shim each
  produced the exact sentinel, reached idle, emitted no approval/tool/error,
  closed exactly once, exited cleanly, and left no child process alive.
- A fresh signed `0.2.0` package passed Rust verification and the packaged CDP
  renderer/host smoke. It was not published, staged, or installed; the private
  package output was moved to Trash afterward.
- A final read-only boundary audit found no P0 or P1 strict-port defect and no
  operational/domain TypeScript outside the retained Electron shell.

## Deliberate boundary and known representation limits

- This migration adds no product feature. BridgeMind and Claude App research
  informed the process boundary only; feature work starts after this port.
- `electron/src/engines` contains no provider implementation. Electron main
  retains native effects and transport only, while Rust owns providers,
  canonical state, workspace policy, updates, worktrees, and the CLI.
- Rust strings cannot represent lone UTF-16 surrogates. A preview boundary that
  bisects one is transported as U+FFFD. Rust Unicode lowercase is also not an
  implementation of JavaScript host-locale `toLocaleLowerCase`. Both fringes
  are covered and documented; ordinary valid UTF-8 session text is unchanged.

## Post-parity phase

On 2026-08-27 the user explicitly authorized the first product expansion: a
Rust-owned integrated multi-terminal workspace inspired by BridgeMind Code
Mode. This work versions the desktop protocol rather than changing the accepted
0.2.0 parity claim retroactively.

### Integrated-terminal acceptance snapshot — 2026-08-27

- Desktop protocol v2 froze 105 request operations and 8 push families. PTY
  lifecycle, process-tree teardown, generation fencing, bounded output, and
  worktree leases are owned by Rust; terminal bytes use a dedicated bounded
  Electron message channel rather than the JSON control stream.
- The retained React renderer now provides persistent shells, tabs, same-pane
  and cross-pane drag ordering, horizontal and vertical splits, adjustable
  ratios, overview/tidy and maximize modes, pane cycling, rename, duplicate,
  restart, confirmed kill, dismiss, search, clipboard and file drop, unread
  state, and VoiceOver-aware xterm rendering.
- The final canonical `cargo xtask check` passed strict workspace Clippy, all
  Rust tests and doc tests, 2,209 Vitest tests across 100 files, 14 Electron
  release-policy tests, both TypeScript checks, and the production renderer
  build.
- The compiled Rust host terminal smoke passed three consecutive runs. Each run
  created two isolated real PTYs and verified concurrent output, resize,
  rename, natural exit, forced process-tree close, restart, lifecycle ordering,
  dismiss, and cleanup.
- A live development Electron window mounted and toggled the integrated
  terminal workspace through its real preload/Rust-host boundary. The terminal
  expansion was not repackaged, published, staged, installed, or committed.

### Terminal launcher acceptance snapshot — 2026-08-28

- Protocol v3 adds `iface:listTerminalLaunchProfiles` and versioned terminal
  launch intent without changing the dedicated PTY byte channel. Rust owns the
  availability and executable mapping for Shell, Claude Code, Codex, Grok,
  Qwen, and Antigravity. Its legacy `gemini` profile id stays frozen for saved
  workspaces; Electron transports only the frozen profile DTOs.
- The retained renderer presents logo-and-label profile buttons, exact count
  buttons from 1 through 10, the approved workspace root, and a reviewed launch
  summary. The workspace limit is now 10 panes.
- The final `cargo xtask check` passed strict workspace Clippy, all Rust tests
  and doc tests, 2,225 Vitest tests across 101 files, 14 Electron release-policy
  tests, both TypeScript checks, and the production renderer build.
- The compiled host smoke crossed protocol v3, read the six-profile public
  catalog, exercised two full lifecycle PTYs, then launched ten independently
  identified Shell-profile PTYs concurrently. All ten produced their unique
  output, exited, and were dismissed; the run observed 40 lifecycle events.
  Separate real-PTY coverage proves an agent profile executes the host-fixed
  canonical program, argv, and environment directly and retains it on restart.
- A private-root development Electron window displayed all five agent logos,
  the secondary Shell option, disabled-provider reasoning, exact buttons 1–10,
  approved working-folder copy, and the dynamic launch action at version 0.3.0.
- Fresh Apple Development-signed 0.3.0 app, DMG, and ZIP artifacts passed deep
  signature, fuse, embedded-Rust-binary, archive-integrity, and packaged
  renderer/Rust-host smoke. The ZIP-extracted app invoked
  `iface:listTerminalLaunchProfiles` through the real preload, opened the
  launcher, and read back the complete logo roster, exact 1–10 controls, and
  approved working directory.
- Updater feed staging, `/Applications` installation, running-byte replacement,
  restart, rollback, and installed-runtime readback were not performed.

The ten-terminal statements above remain historical 0.3.0 acceptance evidence.

### Adversarial-review acceptance snapshot — 2026-08-28

- Protocol v4 freezes 109 renderer request operations and 8 push families. The
  launcher retains exact independent-terminal count buttons from 1 through 6
  and replaces the old preset cards with a separate adversarial-review mode.
- Adversarial review accepts only a cross-provider Claude Code/Codex pair with
  explicit builder and reviewer roles. Rust validates the canonical working
  directory, objective, provider availability, and Claude's native review
  command before either session launches.
- Rust durably owns request idempotency, pair identity, launch outcomes,
  provider-native review delivery, duplicate-idle suppression, one feedback
  handoff, one correction pass, and the session-registry event projection used
  by the ordinary workspace UI.
- The renderer atomically reserves two pane slots before the provider effect,
  prevents concurrent new panes from consuming that capacity, and releases the
  reservation on failure. Both review sessions are categorized under the
  selected project and use the selected approved working directory.
- The final source `cargo xtask check` passed formatting, strict workspace
  Clippy, all Rust unit/integration/doc tests, 2,319 Vitest tests across 106
  files, 14 Electron release-policy tests, both TypeScript checks, and the
  production renderer build.
- The packaged smoke is designed to cross the real preload/Rust-host boundary,
  prove fail-closed adversarial-pair validation without launching a provider,
  and then launch six deterministic Claude-profile PTYs while verifying exact
  equal 3-by-2 layout geometry and bidirectional terminal I/O.

### Review and reduction pass — 2026-08-29

- An agent team reviewed the repository end to end and removed roughly 6,400
  lines: production-dead modules (`shared/fserror.ts`, `shared/projectroot.ts`,
  `shared/retention.ts`, the contracts `model_catalog` and `project_root`),
  duplicated logic, dead renderer props, unreferenced crate dependencies, and
  the tests that existed only to exercise them. Verified defects found on the
  way were fixed rather than deleted around.
- The frozen desktop surface was not touched: 109 renderer request operations
  and 117 total channels, with every channel id, registration, and serde DTO
  field intact.
- The canonical `cargo xtask check` passed: formatting, strict workspace
  Clippy, 743 Rust tests and doc tests with the one live migration test still
  ignored, 2,120 Vitest tests across 96 files, 14 Electron release-policy
  tests, both TypeScript checks, and the production renderer build.
- No packaging, signing, updater, installation, or live-provider gate was run.

### Bare-CLI transcript voice — 2026-08-29

Port of the TypeScript delta's Claude Code transcript vocabulary into the
retained renderer (item `tw-cli-transcript-voice`). Welcome / working / done /
meter / session-line copy live in `session-status.ts`; `groupFinishedTools`
never folds a running or failed call; Escape stops only while a turn is
running; CLI chrome paints glyph-led transcript rows and lights `❯` only on
the focused pane. `engineVersion` / `planName` stay optional Conversation
props; the surfaces renderer now supplies them from engine health and usage.
Tile tokens, toolbar, AgentTile, and App.tsx were out of this item's scope.

### Owner-ruled divergences from the 0.1.169 oracle — 2026-08-29

Deliberate behavior changes directed by the owner, each tested; the oracle is
no longer authoritative for these:

- `iface exec --timeout` is now a real deadline on both paths (it was inert,
  matching the oracle). Expiry after provider contact reports "outcome-unknown"
  and never cancels the session.
- The Codex create path now delivers the collaboration source instruction on
  the first turn; the oracle deliberately declined to inject it on create.
- Settling a worktree escrow stamps the transferred lease only on the task run
  bound to that lease; the oracle stamps every task run of the target agent.
- Collaboration records stuck in `Unknown` are reconciled to `Failed` at host
  startup, unlocking their source session ("failed equals safe").
- The local update feed retries `accept()` failures with backoff and trips off
  after 10 consecutive failures, reporting the feed unavailable.
- Terminal input is per-terminal: one pane's parked PTY write no longer stalls
  typing in other panes, and a saturated pane's excess input is dropped for
  that pane alone instead of failing the shared transport.
- ACP chat is refused. Grok, Qwen, and custom ACP agents cannot run a tool-less
  chat because ACP has no way to remove an agent's tools; the TypeScript ACP
  adapter would silently launch a chat with tools. Date 2026-08-29.

### Chat · Code · Agent session kind — protocol v5 — 2026-08-29

**Owner-ruled.** Additive optional `kind` (`"chat"` | `"code"` | `"agent"`) on
`LaunchOptions`, `ResumeOptions`, and `SessionSummary`. Absent kind means
`"code"`. Host wire protocol is v5; request operations stay 109 and pushes
stay 8. No registry, runtime, host chat-home, engine-preset, or renderer
surface semantics in this snapshot.

- Crash-safe local stores (Owner-ruled, 2026-08-29): an unreadable, directory,
  or linked `appearance.json` no longer fails host boot — `getAppearance` serves
  System and `setAppearance` returns `appearance-unavailable` without writing.
  Registry open quarantines only that cache's stale atomic temps (other stores'
  temps in the same directory are left alone). Atomic fsync writes were already
  stricter than the TypeScript helper and were not reimplemented. `cargo xtask
  check` was not run for this item.
- Team fieldset, Canvas toolbar button, palette row, and AgentFeed mount:
  Shelved 2026-08-27, code kept.
- Pane chrome (`tw-pane-chrome`): agent and terminal tiles share `--tile-radius`
  / `--tile-gap` / `--tile-head-h` in both chromes; the header carries a
  `data-status` dot, always-visible split/maximize/close, and the pane's own
  permission picker; split opens the seeded composer and appends via the
  reservation helper rather than replacing the focused pane. Terminal panes
  restated the same ring/gap/head idiom only. Evidence: AgentTile.test.ts,
  workspace-ui.test.ts, chrome-mode.test.ts, theme-accessibility.test.ts,
  TerminalPrimitives.test.ts. Live provider, packaged-app, and installed-runtime
  gates were not run.

### Chat · Code · Agent renderer surfaces — 2026-08-29

- The renderer persists the selected `SessionKind`, keeps independent Chat,
  Code, and Agent workspace slots, migrates the legacy workspace into Code,
  and filters the sidebar registry to the active kind. Pane capacity,
  reservations, split panes, terminals, project plans, and adversarial review
  remain Code-only.
- A centred Chat / Code / Agent switch is the primary toolbar navigation. Its
  icon-and-text buttons retain stable accessible names and titles when narrow
  CSS hides the text. Rust's Plans and Terminal controls remain ordinary
  buttons on the right; the command palette mirrors the three surfaces.
- The Chat surface launches `kind: "chat"` through the shared composer without
  requiring a project directory. The composer supplies the host-defined chat
  permission/worktree semantics, while unsupported-engine refusals remain
  visible launch errors. Code launch preferences are not overwritten by Chat.
- The Agent surface mounts `AgentTask` with the session's merged conversation,
  live state, plan, touched-file statistics, stop action, and Code diff handoff.
  CLI conversation chrome receives the selected engine version and usage plan.
- Focused evidence passed: 80 Vitest assertions across `workspace-ui`,
  `orchestration-app`, `project-plans-app`, and `status-waiting-allowlist`; both
  TypeScript checks; and the production renderer build. The build retained its
  existing large-chunk warning. Full `cargo xtask check`, live-provider,
  packaged-app, updater, installed-runtime, and restart gates were not run for
  this renderer item.
