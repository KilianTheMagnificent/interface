# Code map

The architecture navigation map: the process and data flow, every crate in one
line, a "where does X live" lookup, and where to read next.
`docs/ARCHITECTURE.md` states the responsibility boundary. The per-area docs
under `docs/areas/` carry the module maps, entry points, invariants and
gotchas; the `AGENTS.md` beside the code carries the rules for editing it.

For a small task, use `cargo xtask navigate '<terms or path>'` (`--json` for a
structured packet). [AGENT_TASKS.md](AGENT_TASKS.md) explains context selection
and acceptance. [agent-map.json](agent-map.json) contains checked owners, source
paths, scoped rules and focused tests; use it directly if Cargo is unavailable.

[TODO.md](TODO.md) records unfinished features, remaining acceptance checks and
the exact next step for resuming work. Update it before an incomplete handoff.

## Workspace shape

```text
electron/src/renderer      React, presentational; App.tsx is the only window.iface owner
        │ preload contextBridge (sandboxed, no runtime imports)
electron/src/main          window, CSP, host supervision, positional args -> named payloads
        │ versioned NDJSON protocol over stdio  +  fd3/fd4 terminal data v2
apps/interface-host        one owner per frozen operation; adapters only, no domain logic
        ├── crates/interface-runtime ──── crates/engine-{codex,claude,acp} ─→ provider CLIs
        ├── crates/interface-orchestration, interface-collaboration
        ├── crates/interface-{terminal,desktop-services,updater}
        └── crates/interface-{storage,registry,settings,review,workspace,prompt-context}

apps/iface                 standalone Ratatui TUI + CLI over the same Rust services
crates/interface-contracts frozen DTOs / channel table / pure validators (leaf crate)
```

| Path | Purpose |
| --- | --- |
| `apps/interface-host` | The Rust process Electron speaks to. Owns the [versioned bridge](PROTOCOL_FACTS.md) and every desktop service owner. Adapters only. |
| `apps/iface` | Standalone Ratatui TUI and `iface` CLI over the same Rust services. No Electron. |
| `apps/interface-ios` | SwiftUI iPhone shell, protected identity and native/WebKit transport; Mac Rust owns session and trust decisions. |
| `apps/interface-voice` | Apple speech/audio framework adapter driven by the Rust host. |
| `crates/interface-contracts` | Frozen wire vocabulary: DTOs, channel table, `validate()` gates. No I/O, no async, no provider knowledge. |
| `crates/interface-engine` | Provider-neutral adapter/session traits, three-state delivery certainty, NDJSON + JSON-RPC framing, argv-only process specs. |
| `crates/interface-runtime` | Host-facing session service: adapter registry, open/resume transaction, live-attachment map, event pump, management ops, the typed bridge command schema. |
| `crates/interface-orchestration` | Event-sourced multi-agent state: pure reducer, byte-budgeted projections, one-writer CAS store. |
| `crates/interface-collaboration` | Durable cross-model consult/handoff: record journal, coordinator, broker authority, owner-private MCP server. |
| `crates/engine-codex` | OpenAI Codex `app-server` adapter. |
| `crates/engine-claude` | `claude` CLI streaming-JSON adapter plus the `~/.claude/projects` transcript store. |
| `crates/engine-acp` | ACP v1 adapter: built-in Grok, built-in Qwen (owner-managed MLX backend), user Custom profiles. |
| `crates/provider-replay` | Test-only, never packaged: `provider-record` records a real provider CLI session (scrubbed) into `fixtures/<engine>/<scenario>.json`; `provider-replay` stands at the CLI's path and serves it to the real adapter by what arrives. Driven by `apps/interface-host/tests/suite/provider_replay.rs` (replay) and `provider_record_live.rs` (live, opt-in). |
| `crates/interface-storage` | Persistence primitives: owner-private dirs, descriptor-bound writer leases, atomic writes, bounded JSON, quarantine. |
| `crates/interface-registry` | Session cache, managed git worktrees, review/recovery/turn checkpoints. |
| `crates/interface-settings` | Appearance, ACP startup roster, saved terminal workspaces, default directory resolution, the Projects folder New project creates in (`projects_folder.rs`), the agent store (`agents.rs`), agent home folders (`agent_home.rs`), decisions (`agent_decisions.rs`), durable start intents, task/evidence ledgers (`agent_work.rs`) and calendar recurrence (`agent_calendar.rs`). |
| `crates/interface-review` | Bounded git/npm subprocesses producing a credential-filtered diff and check results. |
| `crates/interface-workspace` | Shared credential/binary path classifier, Quick Open, project search, editor compare-and-save, customization inventory. |
| `crates/interface-common` | Leaf helper crate: the civil-date/ISO timestamp renderer, `paths::lexically_normalized`/`lexically_absolute`, `process::poll_backoff`, capped child-output reads, the shared credential/binary path tables and the tooling's `paths::is_sensitive_name`, the poison-tolerant `sync::lock`, `unix::effective_uid`/`same_stable_metadata`, `fsid::DirectorySnapshot`, `bounded_read` (the deadline-bounded project read behind Threads' index copy, Review's untracked reads, Quick Open and output preview: an iCloud-evicted file fails as still downloading instead of hanging), `product_version!` (expanded only in iface and interface-host's `main.rs`: the release-stamped version the binaries report), and `git_hardening` (the overrides that keep Interface's own git calls from running programs a repository's settings name). No I/O policy of its own. |
| `crates/interface-prompt-context` | Path authorization, the four prompt-context operations, pasted-image store. |
| `crates/interface-terminal` | Integrated PTY registry, host-owned launch profiles, OSC title scanning, approved-root allowlist. |
| `crates/interface-remote` | Opt-in loopback HTTP/WebSocket service, phone pairing, input leases and bounded output replay. |
| `crates/interface-desktop-services` | The desktop operation dispatcher plus workspace grant authority and worktree cleanup leases; agent commands (`agent_control.rs`) and the pure agent approval policy with its shell tokenizer (`agent_policy/`, `agent_shell.rs`). |
| `crates/interface-updater` | Updater policy: feed admission, enablement, state machine, macOS artifact verification, loopback private feed, and the public update channel (signed manifest, download, staging). |
| `crates/interface-voice` | Voice-helper protocol, sentence chunking, speech text cleanup, and bounded terminal-tail projection. |
| `crates/interface-remote` | Phone access/trust, encrypted connection, replay controls, HTTP/WebSocket service and relay. |
| `electron/` | The retained shell: main process, sandboxed preload, React renderer, packaging/signing/smoke scripts. |
| `xtask/` | `cargo xtask check` (the gate), `cargo xtask prove` (a pinned test fails on the base and passes here), macOS packaging, journalled release staging, the closeout commands. |
| `docs/` | Architecture, area docs, parity status, migration and packaging acceptance notes. `docs/archive/` holds completed migration records. |
| `release/` | Local updater feed and staged artifacts. Build output, gitignored. |
| `deploy/phone-relay` | Relay build/deployment recipe. Deployment requires separate authority. |
| `deploy/phone-relay-worker` | The same relay as a Cloudflare Worker (recommended host); its own wasm workspace, outside the gate. |

## Where does X live

| Task | Go to |
| --- | --- |
| Commit file/hunk selection | `crates/interface-review/src/thread_commit.rs`, `apps/interface-host/src/threads/ship.rs`, `components/CommitSelectionForm.tsx` |
| Durable output drafts and prompt recovery | `crates/interface-settings/src/{output_reviews,prompt_recovery}.rs`, `components/{OutputPreviewPanel,PromptRecoveryPanel}.tsx`, [Output preview](OUTPUT_PREVIEW.md) |
| Provider customization and local background operation | [Provider customization](PROVIDER_CUSTOMIZATION.md), `inventory_providers.rs`, `inventory_probe.rs`, `service/background.rs`, `electron/src/main/main.ts` |
| Find exact files and focused tests for an agent task | `cargo xtask navigate '<terms or path>'`; [bounded task guide](AGENT_TASKS.md) |
| iPhone, remote trust, encrypted pairing, relay | [Remote and native map](areas/remote-and-native.md), `crates/interface-remote`, `apps/interface-host/src/remote.rs`, `apps/interface-ios`, `electron/src/remote` |
| Agent teammates: roster, threads, home folders, routines, approvals | [Agent teammates](areas/agents.md), `crates/interface-settings/src/{agents,agent_home,agent_decisions}.rs`, `crates/interface-desktop-services/src/{agent_control.rs,agent_policy/,agent_shell.rs}`, `apps/interface-host/src/agents.rs`, `electron/src/renderer/components/AgentWorkspacePanel.tsx` |
| Bounded Away demo and recovery | [Away pilot](AWAY_PILOT.md), `crates/interface-collaboration/src/away.rs`, `apps/iface/src/away.rs` |
| Thread journal (each thread's own record; the Threads search's "In messages"; history for providers that keep none) | [T3-style systems, section 1](superpowers/specs/2026-11-14-t3-systems-design.md), `interface-contracts/src/thread_journal.rs`, `interface-settings/src/thread_journal.rs` (+ `thread_journal/`: segments, bounds, replay, search, import), host `threads/journal.rs` (tap, prompts, worker, drain, `iface:threadJournal`, history fallback), `electron/src/shared/thread-journal.ts`, `electron/src/renderer/thread-inbox.ts` (`inMessages`) |
| Team tools (an agent in a thread starts, waits for, reads and messages other threads; started threads' "Started by" and the caller's "Started threads" line) | [T3-style systems, section 2](superpowers/specs/2026-11-14-t3-systems-design.md), `interface-settings/src/thread_team.rs` (ladder, limits, status, notices, `thread-team.v2.json`), `interface-contracts/src/engine.rs` (`TeamToolsFactory`), `interface-engine/src/team_tools.rs` + each engine adapter, host `team_tools.rs` (sockets) and `threads/team.rs` (the tools), renderer `started-threads.ts` + `components/StartedThreads.tsx` |
| Outside agents (a Claude Code or Codex outside Interface reads threads and, when allowed, starts and messages them; `iface mcp`) | [T3-style systems, section 11](superpowers/specs/2026-11-14-t3-systems-design.md), `interface-settings/src/thread_team.rs` (levels, limits, projects, socket path; `thread-team.v2.json` keeps the level and `startedBy`), host `outside_agents.rs` (the owner-only socket, `hello`/`call`, the catalogue) and `threads/outside.rs` (the tools), `apps/iface/src/mcp.rs` (the stdio MCP proxy), renderer `outside-agents.ts`, `components/SettingsPage.tsx` (Outside agents), `components/ThreadPaneHead.tsx` (Started from outside) |
| Switch provider inside a thread (one thread, a chain of provider sessions; handoff from the journal; switch back with only what was missed) | [T3-style systems, section 5](superpowers/specs/2026-11-14-t3-systems-design.md), `interface-settings/src/threads/chain.rs` (store v5), `thread_journal/read.rs` (`replay_chain`), `interface-collaboration/src/thread_handoff.rs` (`switch_prompt`), host `threads/switch.rs`, renderer `thread-opinion.ts`, `thread-pane.ts` (`switchThreadTo`), `use-thread-bridges.ts`; hands-free Grok `electron/scripts/handsfree-fake-acp.mjs` |
| Several accounts per provider (named Claude, Codex and Grok profile directories; the account a new thread, terminal pane or sign-in uses; usage per account) | [T3-style systems, section 6](superpowers/specs/2026-11-14-t3-systems-design.md), `interface-contracts/src/provider_accounts.rs`, `interface-settings/src/provider_accounts.rs` (`provider-accounts.v1.json`, `session-accounts.v1.json`), `interface-runtime/src/accounts.rs` (`AccountRoutedAdapter`), each engine's environment overlay, host `provider_accounts.rs` (`iface:providerAccounts`), `provider_capture/accounts.rs`, renderer `provider-accounts-view.ts`, `components/ProviderAccountsSection.tsx` |
| Composer references and stash (Quote a reply's selection, a terminal selection into a thread's draft, a thread chip whose block Rust writes at send, stashed drafts) | [T3-style systems, section 9](superpowers/specs/2026-11-14-t3-systems-design.md), `interface-contracts/src/draft_stash.rs`, `interface-settings/src/draft_stash.rs` (`draft-stash.v1.json`) and `thread_reference.rs` (who may refer to what, the block), `interface-prompt-context` (`PromptContextThreads`, `chat` validation), host `threads/stash.rs` (`iface:draftStash`) and `threads/reference.rs`, renderer `composer-quote.ts`, `thread-reference.ts`, `draft-stash-view.ts`, `use-draft-stash.ts`, `components/StashedDrafts.tsx`, `components/ComposerReferenceDialogs.tsx` |
| Quick wins: Write with AI (commit message and pull request text), the private secret card (`request_secret`, one owner-only file), side questions (`/btw`, a hidden read-only fork shown on the right) | [T3-style systems, section 12](superpowers/specs/2026-11-14-t3-systems-design.md), `engine-claude/src/ship_text.rs` (+ `title.rs`'s one-shot runner), `interface-review/src/thread_ship_text.rs` (material), `interface-contracts/src/thread_secrets.rs` (`iface:threadSecret`), `interface-settings/src/thread_secrets.rs` (files, expiry) and `thread_sides.rs` (`thread-sides.v1.json`), host `threads/ship.rs` (`suggestShipText`), `threads/secrets.rs`, `threads/side.rs`, renderer `components/WriteWithAi.tsx`, `thread-secrets.ts`, `components/ThreadSecretCard.tsx`, `components/ThreadSecretsSettings.tsx`, `use-thread-secrets.ts`, `side-question.ts`, `components/SideQuestionPanel.tsx` |
| Visual replies (an agent shows a rendered page above its reply; Output preview's Render) | [T3-style systems, section 8](superpowers/specs/2026-11-14-t3-systems-design.md), `interface-settings/src/thread_visuals.rs` (page store), `thread_journal/read.rs` (`merge_visuals`), host `threads/team.rs` (`show_visual`) and `threads/journal.rs` (store worker, `visual` read), `electron/src/shared/visual-frame.ts` (the fence), main `visual-frames.ts` (`iface-visual:`, navigation guard), renderer `visual-reply.ts` + `components/VisualReply.tsx` |
| Scheduled prompts and pull-request watch (a thread sent a prompt at a time, on a repeat or calendar, or when its pull request's checks fail or review comments arrive; `iface trigger`) | [T3-style systems, section 7](superpowers/specs/2026-11-14-t3-systems-design.md), `interface-settings/src/thread_schedules.rs` (due times, gate, fingerprints, prompts, `thread-schedules.v2.json`), host `threads/schedules.rs` (driver, delivery, watch pass, team tools `schedule_prompt`/`list_schedules`/`cancel_schedule`), `orchestration_transport.rs` (`thread.schedule.trigger`), iface `app.rs` (`trigger`), renderer `thread-schedules.ts`, `use-thread-schedules.ts`, `components/{ThreadScheduleDialog,ThreadScheduleLine,ThreadSchedulesSettings}.tsx` |
| Resume at reset (a thread whose provider hit its usage limit carries on by itself a minute after the reset, or snoozes until then) | [T3-style systems, section 10](superpowers/specs/2026-11-14-t3-systems-design.md), each engine's `mapper.rs` (`RateLimitNotice.resetsAt`), `interface-settings/src/thread_schedules.rs` (`resume_at`, `resume_prompt_choice`, `usage_windows_reset`, `set_resume`), host `threads/resume.rs` (the thread's limit, the usage-window fallback, `resumeAtReset`) and `threads/schedules.rs` (delivery), renderer `thread-opinion.ts` (`threadLimitReset`, `resumeQueuedPrompt`), `components/ThreadOpinionCards.tsx`, `ThreadScheduleLine.tsx`; hands-free `limit <seconds>` in `electron/scripts/handsfree-fake-claude.mjs` |
| Desktop Away loops (each a Threads thread in its own worktree) | [Project Away](PROJECT_AWAY.md), `crates/interface-collaboration/src/project_away.rs` (+ `project_away/store.rs`), host `project_away.rs` and `threads/away.rs`, renderer `components/AwayControl.tsx`, `away-view.ts`, `away-thread.ts`, `components/ThreadAwayParts.tsx` |
| Retired features (Clean GUI, pane delegation, Control assistant, Tailscale phone route) | `docs/archive/` (one README per feature: source branch, data left on disk, the channels that still answer unavailable) |
| Voice helper and text chunking | [Voice assistant](VOICE_ASSISTANT.md), `crates/interface-voice`, `apps/interface-host/src/voice`, `apps/interface-voice` |
| Add or rename an IPC channel | Deliberately version and test the frozen surface. See [checked protocol facts](PROTOCOL_FACTS.md) and "Frozen surfaces" below. |
| Provider or model behavior | `crates/engine-codex`, `crates/engine-claude`, `crates/engine-acp`. Never the host or the renderer. |
| Provider CLI drift canary (what Interface depends on in each CLI) | `crates/interface-terminal/src/provider_canary.rs` (expectations and probes), `interface-settings/src/provider_canary.rs` (`provider-canary.v1.json`), host `provider_canary.rs` (protocol 51), `iface doctor providers`, `cargo xtask provider-canary` |
| Provider replay tests (a recorded real CLI session through the real adapter, runtime and threads) | `crates/provider-replay` + `apps/interface-host/tests/suite/{provider_replay,provider_record_live}.rs`; [design §4](superpowers/specs/2026-11-14-t3-systems-design.md) |
| Prove a fix (fails on the base, passes here) | `cargo xtask prove --pin <path>::<test>` → `xtask/src/prove.rs` |
| Delivery certainty, replay safety, adapter traits | `crates/interface-engine/src/{outcome,adapter,jsonrpc}.rs` |
| Open/resume a session, session list/rename/fork/import | `crates/interface-runtime/src/{runtime.rs,management.rs,management/}` |
| Multi-agent state transitions | `crates/interface-orchestration/src/reducer.rs` (pure), driven by `apps/interface-host/src/orchestration.rs` |
| Cross-model consult/handoff | `crates/interface-collaboration`, driven by `apps/interface-host/src/collaboration.rs` |
| Terminal features | `crates/interface-terminal` + `apps/interface-host/src/{terminal,terminal_transport}.rs` + `electron/src/main/terminal-data-transport.ts` + `electron/src/renderer/components/IntegratedTerminalCenter.tsx` |
| iPhone remote access | `docs/IPHONE_REMOTE.md`, `crates/interface-remote`, host `remote.rs`, renderer `RemoteAccessPanel.tsx`, `electron/src/remote/` |
| Colour themes, theme files, looks (backdrops, glass, moving scenes), the Settings theme picker | [Theme files](THEMES.md), `electron/src/renderer/theme.css`, `electron/src/renderer/appearance.ts`, `electron/src/renderer/theme-look.ts` + `theme-look.css` (looks), `electron/src/renderer/theme-shader.ts` (shader backdrops), `docs/theme-examples` (and `tn-code`, TN Code's themes), `skills/interface-theme`, `crates/interface-settings/src/custom_themes.rs`, `crates/interface-settings/src/theme_import.rs` (`iface theme convert`), `apps/interface-host/src/theme_watch.rs` |
| Persistence, on-disk format, atomicity | `crates/interface-storage` (primitives), then `interface-registry` / `interface-settings` / `interface-collaboration/src/journal.rs` |
| Logs, Copy Diagnostics, set-aside store notices, the app menu | host `host_log.rs` and `diagnostics.rs`, `interface-review/src/diagnostics.rs` (the report and its credential filter), `interface-storage/src/log_file.rs`, `electron/src/main/app-menu.ts`; accelerator audit in [APP_MENU.md](APP_MENU.md) |
| Updater | `crates/interface-updater` + `electron/src/main/native-updater.ts` + `xtask/src/release.rs` |
| Updates for copies on other people's Macs (signed manifest, publishing a release) | [Public updates](PUBLIC_UPDATES.md), `crates/interface-updater/src/{channel,signed_manifest,remote}.rs` and `release-channel.json`, host `updater.rs`, `xtask/src/update_channel.rs` |
| The retired "Halt all and update" (removed 2026-10-21) | [Archive](archive/update-checkpoints-20261021/README.md); the retained `iface:updateSessions` answer is in host `updater.rs` |
| A DTO / wire shape / channel name | `crates/interface-contracts` and its mirror in `electron/src/shared/` |
| Threads (inbox sections, thread workspaces, per-turn diffs, fork at a turn) | [Threads design](superpowers/specs/2026-10-06-threads-design.md), `interface-contracts/src/threads.rs`, `interface-settings/src/threads.rs` (store and rules) and `threads/view.rs` (ordering and projection), `interface-review/src/thread_git.rs` and its `thread_ship.rs` (commit, push, `gh`), host `threads.rs` + `threads/`, `electron/src/shared/threads.ts`; the inbox sidebar `electron/src/renderer/{thread-inbox,use-thread-inbox}.ts` and `components/ThreadInbox.tsx`; the card, pane head and diff panel `electron/src/renderer/{thread-card,thread-pane,thread-turns,thread-diff}.ts` and `components/{ThreadDiffPanel,ThreadPaneHead,ThreadEditDialog}.tsx`; the Threads layout's turns (work line, changes card, controls) `electron/src/renderer/thread-transcript.ts` and `components/ThreadTurnParts.tsx` + `ThreadTranscript.css` |
| Project setup, readiness and saved workflow recipes | [Setup and recipes](WORKFLOW_RECIPES.md), `interface-settings/src/workflows.rs`, `interface-review/src/workspace_setup.rs`, host `threads/workflows.rs`, renderer `WorkflowControls.tsx` |
| Harness commands, Attention, results and shared drafts | [Harness implementation](HARNESS_IMPLEMENTATION.md), `interface-contracts/src/harness.rs`, `interface-settings/src/harness.rs`, `interface-desktop-services/src/harness.rs`, renderer `TaskInspector.tsx` |
| Bounded pairing prototype | `apps/iface/src/away_pair.rs`, `apps/iface/src/away_cycle.rs`, `interface-collaboration/src/away_cycle.rs` |
| Review diff and checks | `crates/interface-review`. Checkpoints: `crates/interface-registry/src/checkpoint.rs` + `crates/interface-desktop-services/src/checkpoint.rs` |
| Inspect outputs, PDF/image/Markdown previews, feedback | [Output preview workspace](OUTPUT_PREVIEW.md), `interface-workspace/src/output_preview.rs`, `OutputPreviewPanel.tsx` |
| Browser pane: page, width presets, point and fix (pick or region, note into a draft), Chrome-like tab (menu, new tabs, popups, downloads, find, zoom, permissions) | [Point and fix](superpowers/specs/2026-11-14-t3-systems-design.md), [workspace tools](WORKSPACE_TOOLS.md), `electron/src/renderer/{browser-page,browser-point,browser-point-page}.ts`, `components/BrowserPane.tsx`, main `browser-pane.ts` (guest guards and decisions), `browser-pane-native.ts`, `shared/browser-events.ts` |
| File editor, Quick Open, project search, inventory | `crates/interface-workspace` |
| Prompt context, @-mentions, pasted images | `crates/interface-prompt-context` + `electron/src/renderer/prompt-context.ts` + `components/PromptContextPicker.tsx` |
| Transcript content rules | `electron/src/renderer/conversation-reducer.ts` (reference reducer; each rule set has its own module, imported directly), `conversation-projection.ts` (incremental projection), `conversation-display.ts` (display grouping), `live-events.ts` (retention), `components/Conversation*.tsx` (drawing: shell, transcript, composer, rows) |
| Sidebar / usage projections | `electron/src/renderer/sidebar-view.ts`, `usage-view.ts` (behind the `viewmodels.ts` barrel) |
| A new renderer IPC call | `electron/src/renderer/App.tsx` only. Components take props and emit callbacks. |
| CLI grammar / TUI | `apps/iface/src/argv.rs` / `apps/iface/src/tui.rs` + `apps/iface/src/tui/controller.rs` (controller) / `apps/iface/src/tui/commands.rs` (slash grammar) / `apps/iface/src/tui/view.rs` (drawing) |
| Terminal output credits | `docs/TERMINAL_DATA_V2.md`, `terminal-output-window.ts`, host `terminal_transport.rs` |
| Fast renderer interaction gate | `electron/scripts/test-renderer-dom.mjs` (lanes in `renderer-lanes.cjs`) — isolated Electron, real preload/MessagePort/xterm, fixture screenshots |
| Packaging, signing, fuses, release staging | `xtask/src/{package,release}.rs` + `electron/scripts/{sign-mac.cjs,verify-fuses.cjs,smoke-packaged.mjs}` |
| The canonical gate | `cargo xtask check` → `xtask/src/gate.rs::run_full_gate` → navigation validation → `gate_cache.rs` → `check_run.rs` → `test_runner.rs` |

## Frozen surfaces

Break one of these and the failure is silent or on someone else's machine. Each
is stated in full where it is enforced; this is the index.

- **Channel counts.** See the [checked protocol facts](PROTOCOL_FACTS.md),
  asserted in `crates/interface-contracts/src/ipc.rs`,
  `apps/interface-host/src/{protocol,router}.rs` and
  `electron/src/main/{ipc-payload,rust-host-client}.ts`. Rules:
  `apps/interface-host/AGENTS.md`, `electron/AGENTS.md`.
- **Deliberately retained, uncalled channels.** `readFileSnapshot`, `saveFile`,
  `setWorkspaceOpen`, `searchProject`, `listReviewCheckpoints`,
  `advanceTerminalControls`, `discoverReviewChecks`, `runReviewChecks`,
  `forgetSession`, `getCollaborationStatus`, `voiceInterrupt`, `readFile`,
  `resumeSession` (the host's voice path uses it internally), `sendLive`
  (host-served, no preload method; the CLI's live-only send) and `listSessions`
  (only the packaged smoke calls it). The renderer calls none of them, but the
  Rust host serves each; they are wire protocol. Never remove or renumber.
- **Retired features' channels.** `voiceControl` (the Control assistant) and
  `readTerminalDelegation` (pane delegation), both retired on 2026-10-14, stay
  in the table: the host decodes each request and answers unavailable
  (`voice-control-unavailable`; a `TerminalDelegationView` with
  `available: false`), as `advanceTerminalControls` does for retired Clean
  terminal settings. Details: `docs/archive/`.
- **Provider effects are fail-closed everywhere.** Definitely-unsent is the only
  replay-safe outcome. The chain and its duplication list: `crates/AGENTS.md`.
- **Persisted schemas.** Registry, worktrees, checkpoints, collaboration
  records, orchestration snapshots and `config.json` all have
  byte-level format contracts with the 0.1.169 layout, pinned by golden-JSON
  tests. Detail: `docs/areas/persistence-and-services.md`.
- **`interface-orchestration`'s reducer is event-sourced.** Changing an
  observable state transition breaks replay of stores that already exist on
  disk. New behavior must be additive and versioned.

## Read next

One area doc per area, each with that area's module map, entry points,
invariants and gotchas. The `AGENTS.md` beside the code is the rules file.

| Area | Area doc | Rules |
| --- | --- | --- |
| `crates/interface-contracts` | `docs/areas/contracts.md` | `crates/AGENTS.md` |
| interface-engine / -runtime / -orchestration | `docs/areas/runtime-core.md` | `crates/AGENTS.md` |
| engine-codex / -claude / -acp, provider-replay | `docs/areas/engine-adapters.md` | `crates/AGENTS.md` |
| storage, registry, settings, review, workspace, prompt-context, terminal, desktop-services, updater | `docs/areas/persistence-and-services.md` | `crates/AGENTS.md` |
| `crates/interface-collaboration` | `docs/areas/collaboration.md` | `crates/AGENTS.md` |
| `apps/interface-host` | `docs/areas/interface-host.md` | `apps/interface-host/AGENTS.md` |
| `apps/iface` | `docs/areas/iface.md` | `apps/iface/AGENTS.md` |
| `electron/src/{main,shared}`, `electron/scripts` | `docs/areas/electron-main.md` | `electron/AGENTS.md` |
| `electron/src/renderer` | `docs/areas/renderer.md` | `electron/AGENTS.md` |
| `xtask/` | `docs/areas/xtask.md` | `xtask/AGENTS.md` |
| Phone access, native iPhone and remote React | `docs/areas/remote-and-native.md` | `crates/AGENTS.md`, `apps/interface-host/AGENTS.md`, `apps/interface-ios/AGENTS.md`, `electron/AGENTS.md` |
| Voice protocol and Apple helper | `docs/VOICE_ASSISTANT.md`, `apps/interface-voice/README.md` | `crates/AGENTS.md`, `apps/interface-host/AGENTS.md`, `apps/interface-voice/AGENTS.md` |
| Agent teammates, approval policy, roster and routines | `docs/areas/agents.md` | `crates/AGENTS.md`, `apps/interface-host/AGENTS.md`, `electron/AGENTS.md` |
