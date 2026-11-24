# apps/interface-host

Module map, entry points and mechanism for the Rust process Electron speaks to.
Rules for editing here: `apps/interface-host/AGENTS.md`. Navigation:
`docs/CODEMAP.md`.

**Modules**

- `agents.rs` (tests in `agents_tests.rs`) — the [agent teammates](agents.md)
  adapter. `AgentHost` serves `iface:agentControl` ahead of the desktop owner,
  starts gated agent threads (`startAgentThread`), runs the approval driver
  (facts → `agent_policy::classify` → decision log → one automatic answer or a
  pending owner approval), and implements `ResumeRequirements` for the runtime
  dispatcher. `AgentRuntimeGuard` wraps the runtime and session-management
  bridges every other caller uses (router, voice, away, `sendLive`).
  Its agent-store calls, and those of a routine or task launch, run on the
  blocking pool, since a commit holds the store across fsync; a session's
  binding lookup stays inline while the store is free (`try_agent_binding`).
  `AgentEventTap` forwards session events to the store and approvals to the
  driver. It retires a session only when its agent task's projection failed or,
  with the store unreadable, when the agent host launched or resumed it; an
  ordinary chat is never closed by agent storage. `AgentRunner` is the routine scheduler: it sleeps until the next due
  routine or queued run (at least two seconds between claims, at most a
  minute), is woken by agent commands and committed run changes, and
  launches runs gated. `LateDispatcher` breaks the host↔runtime construction cycle.
- `project_away.rs` — desktop owner and native provider-event adapter for the
  [one-button project loop](../PROJECT_AWAY.md). Scheduling and stores live in
  `interface-collaboration`; this adapter binds exact managed-session identities.

- `main.rs` — process entry: builds the collaboration broker, orchestration host,
  runtime and every owner dispatcher in order, runs `HostServer` on stdio, then
  tears the graph down in reverse.
- `native_checkpoint_setup.rs` adds Antigravity's, Muse's and Kimi Code's
  inert-outside-Interface checkpoint plugins (Kimi has no install command, so
  the host writes its managed copy and `installed.json` record itself, only
  over an unchanged registry; Muse's run-scoped `--config`
  hook merge remains only as the fallback when the plugin is unusable: a
  fresh in-process session drops `--config`, so its stops would never be
  observed). `provider_capture/native.rs` validates hook ancestry and
  private generation receipts; Muse plugin hooks carry no capture token
  (their environment is filtered), so they resolve their generation by
  that ancestry alone (`native_checkpoint::LINEAGE_TOKEN`).
- `bootstrap.rs` — opens the production service graph (desktop stores → runtime
  adapters → managed-worktree ledger) and resolves the eight provider executables
  through one cached login-shell PATH lookup. `main.rs` starts that lookup
  (`LoginShellPathProbe`) on its own thread before the runtime exists, so the
  user's shell startup overlaps the journal, broker and store opens.
- `protocol.rs` — the wire envelope: `BridgeRequest`/`BridgeResponse`/
  `BridgeError`/`PushFrame`/`HostReady`, `PROTOCOL_VERSION`, token and channel
  validation and the push channels listed in [checked protocol facts](../PROTOCOL_FACTS.md).
- `server.rs` — the newline-JSON bridge loop: ready frame, bounded concurrent
  request admission, a single writer shared by pushes and responses, the
  push-before-response barrier, oversize-response demotion.
- `router.rs` — `OperationOwnerRouter` (disjointness proof + exhaustive dispatch)
  and `DesktopHostDispatcher` (bounded blocking pool, per-dispatch session-root
  authority, workspace-close release hook, replay-safety classification).
- `dispatcher.rs` — the 13 frozen runtime-bridge channels: payload decode, the
  collaboration fence, resume/send/model/effort/fast-mode/permission/interrupt/
  close transactions, worktree-isolated create, per-session event pumps.
- `host_log.rs` — `host.log` in the data root: the WARN and ERROR half of
  the tracing writer (`HostLogSink`, attached once the data root is known),
  owner-only, rotating at 5 MB with one predecessor
  (`interface_storage::RotatingLogFile`), and its tail for diagnostics.
- `diagnostics.rs` — `DiagnosticsHost` serves `iface:diagnostics`
  (protocol 39): the Copy Diagnostics report (facts gathered here, written
  and credential-filtered by `interface_review::diagnostics`), this run's
  `StoreNotices` (a store that `main.rs` found set aside at open: the
  session cache, the threads list, the permission modes) and Finder reveals
  of the log, the data folder and a set-aside file. A reveal is never replayed.
- `provider_refresh.rs` — protocol 41's `{refresh: true}` on `probeEngines`
  and `listTerminalLaunchProfiles`: one host-wide `ProviderRefresh` runs
  `bootstrap::ProviderCatalogSource::rebuild` (a fresh login shell, the
  provider CLIs resolved again, a new launch catalog, each engine's shared
  `ExecutablePath` repointed) on the blocking pool and swaps the catalog
  into the terminal service. Refreshes never overlap; one asked while
  another runs takes its result, except `rebuild_after_setting_change`
  (protocol 71's pane tools switch, from `ThreadHost::set_pane_tools`), which
  always rebuilds because a running refresh may have read the old setting. A `{details: true}` launch-profile listing
  (protocol 43, Settings' Terminal tools) also reads each found CLI's
  `--version` in `terminal.rs`, cached per file.
- `theme_watch.rs` — creates `~/.config/interface/themes` at startup and
  polls its fingerprint on the timer task (every second while it holds a
  theme file, every five while it holds none), pushing
  `iface:appearanceChanged` (empty payload) after each change so a saved
  theme file repaints the window.
- `threads.rs` — `ThreadHost` serves `iface:threadControl` ahead of the
  desktop owner (`OperationOwnerRouter::with_threads`) and emits the
  `iface:threads` push (the whole `ThreadsView`) after every committed
  change. It holds the one `threads.v3.json` writer
  (`interface_settings::threads`, where every thread rule lives); store I/O
  and git run on the blocking pool, and the store lock is held across a
  commit and its push so pushes leave in revision order. Children:
  `threads/ports.rs` (the seams: registry and runtime, hidden Away and
  orchestration workers (the latter from the whole agent list; reconcile
  puts the hidden registry sessions on every view as `hiddenKeys`, protocol
  58), `listDir` folder authority, `thread_git`, `gh`,
  and `ThreadConsults`: the guarded runtime's resume and the collaboration
  service's submit),
  `threads/opinion.rs` (protocol 46, review B4: Second opinion submits an
  ordinary human consult, after refusing the thread's own provider, a
  session a Code pane holds and a running turn, resuming a paused session
  first; Continue with prepares the thread's folder or worktree again with a
  `handoff` lineage and answers the first prompt — not the coordinator's
  handoff mode, which drives its target outside the run tap; the words live
  in `interface_collaboration::thread_handoff`),
  `threads/workspace.rs` (prepare/discard/bind, branch and worktree lists,
  diffs, restore, fork and edit, release, pull request refresh),
  `threads/ship.rs` (protocol 37: the ship status read, commit of exactly
  the files the owner saw, a Local thread's new branch, push and pull
  request with a `gh pr list --head` look first, the failing-checks prompt,
  review comments and a confirmed merge that settles through
  settle-on-merge; one ship effect per thread at a time, each unclear end
  re-read and never retried; mechanics in `thread_git::ship`),
  `threads/run_tap.rs` (`ThreadRunTap` in the event observer chain, the
  `ThreadRegistrySignal` the shared registry coalescer rings each round, and
  the `SendHooks` the runtime dispatcher calls before a send: recreate a
  missing worktree or refuse definitely-unsent, checkpoint, record the turn
  and open its run generation in one commit (the worker drops run changes
  older than it) — except a Codex or Claude send into a turn running by both
  the tap and the registry, which the provider folds into that turn: it
  records nothing and becomes a turn only if the provider starts a new turn
  with it within 30 s of the running one ending; the `SendTicket` takes a
  turn back only for a `SendSettlement::Unsent` send or a `Rejected` one the
  provider never answered), `threads/titles.rs` (generated titles through
  `engine_claude::title`, on `regenerateTitle` or once after a project
  thread's first completed run; the `RenameHooks` that stamp an owner's
  `renameSession` as `titleSource: user` under the session's title gate)
  and `threads/worker.rs` (registry reconcile and seeding, run ends and end
  checkpoints, auto-settle every ten minutes, snooze wakes, pull request
  passes, stale prepared workspaces, worktree release on archive, and the
  `RemovalHooks` that release it on an owner's delete or forget; a record
  whose session stays missing for a day is dropped, its worktree kept).
  Reconcile and auto-settle ask `run_accounted`, not `run_tracked`: a run
  the tap closed whose end op the worker has not recorded yet (counted per
  generation in `RunTrack::ending`, cleared by `end_recorded` after
  `apply_run`) is never taken for a run cut off and filed as stopped.
  `threads/journal.rs` (protocol 56) is the thread journal's host side: the
  run tap hands each relevant event with the run change it made to one
  journal worker over an unbounded channel (no I/O in the pump); the send
  hook, a delivered steer, a take-back and `Bind` send prompts and
  retractions; the worker holds a session's entries until its launch prompt
  (at most 30 s or 2,000 entries), writes only sessions with a thread
  record, fsyncs at a turn's end and within 2 s, imports up to 200 threads'
  provider history once, serves `iface:threadJournal` (reads only) and the
  `HistoryFallback` that answers `readHistory` from the journal when the
  provider's history is unavailable. Its drain is the `thread journal`
  shutdown stage, after the event pumps close; it is never aborted.
  `threads/team.rs` (protocol 57) is team tools: what each `iface_team`
  call does for the thread its socket belongs to (start a child thread on the
  guarded runtime at collaboration depth 1, bound with its `child` lineage
  and its task sent through the send hook as the host's prompt; wait, status,
  list, read, send and stop), the ledger's host side, the waits' change
  signal, and one notice to an idle caller when a started thread finishes.
  A call from a new thread whose launch is still waiting for its bind (a
  prepared workspace for the session's folder not yet bound,
  `ThreadsDocument::awaits_bind`) waits for that bind on the store's change
  signal, at most 10 s (`BIND_WAIT`), before reading the caller's project.
  `team_tools.rs` is the server: one owner-only socket per session under
  `/private/tmp/iface-tt-<id>/`, the catalogue, and the factory the adapters
  ask (`DeferredTeamTools` until the thread host attaches). A socket answers
  only the provider process its adapter named (`provider_process_started`)
  and the processes under it, checked per call against a fresh
  `interface_terminal::ProcessListing` with both start times pinned; any
  other caller is refused. Started threads
  resume one hop down through `RuntimeHostDispatcher::with_team_children`.
  `threads/stash.rs` (protocol 63, T3 §9) serves `iface:draftStash` from
  the thread host (`THREAD_OPERATIONS`): a key resolves to its thread's head,
  a session with no thread record is refused, commits run on the blocking
  pool; the stash follows a switch's new head (`switch.rs`) and leaves with
  a deleted or forgotten thread (`RemovalHooks`). `threads/reference.rs`
  makes the thread host the prompt-context owner's `PromptContextThreads`
  (attached in `main.rs`): at every send it admits a `chat` chip by team
  tools' read rule, reads the target chain's journal and answers the block;
  `team_tools.rs` marks a session's team tools bound while its socket is
  confirmed, which is when the block mentions `read_thread`. Its stashed
  drafts' file paths keep pasted images from the prune.
  `outside_agents.rs` (protocol 65, T3 §11) is the outside socket `iface mcp`
  reaches: `<data root>/team/outside.sock` (or the short `/private/tmp`
  fallback `thread_team::outside_socket_path` names), 0600 in a 0700
  directory, each peer's uid checked against the host's; one connection is
  one outside session the host names; only `hello {client}` then
  `call {tool, input}` are served, and `catalogue()` is the tool list `iface
  mcp` shows. `threads/outside.rs` is what those calls do: the level (kept in
  the team tools ledger, read at every call), any-project reading through the
  team tools' code, and a start in an existing Interface project at the
  target's asking rung through `start_child` (no lineage, no notices, the
  default account). `setSettings` saves the level there and the Threads
  store never keeps it; views join it and `startedFromOutside`.
  Quick wins (protocol 67, T3 §12): `threads/ship.rs`'s `suggestShipText`
  reads the files Commit would stage (or the branch since its default
  branch) through `thread_git::diff`, bounds and filters them
  (`thread_git::ship_text`) and asks `ThreadTitles::write_ship_text` (one
  tool-less `claude -p`, Claude's default account); nothing is committed.
  `threads/secrets.rs` serves `iface:threadSecret` (`THREAD_OPERATIONS`)
  and team tools' `request_secret`: one open request per thread (its head),
  on the view as `secretRequest`, waiting at most ten minutes; Give writes
  the value to its 0600 file and the tool answers only the path. The
  channel's payload is never logged (a malformed one is refused without
  saying why). Expired secrets are swept at attach and hourly; a removed
  thread's go (`RemovalHooks`). `threads/side.rs` owns
  `thread-sides.v1.json` and side questions: a fork at the thread's latest
  turn, hidden (the reconcile's hidden set, never a record), resumed one hop
  down (`TeamChildren`), in plan mode where the provider has it, sent its
  question through the guarded runtime; closing or removing the thread
  deletes the side session and its journal.
  Folder trust (protocol 72): `folder_trust.rs`'s `HostFolderTrust` gives
  each Claude adapter (`bootstrap.rs`: the Default account with the host's
  `CLAUDE_CONFIG_DIR`, each added account with its own folder) an
  `engine_claude::FolderTrust` over `DesktopService::folder_trust_of`, and
  registers the Chat folder as Interface's own. `threads/folder_trust.rs`
  joins `folderTrust` on untrusted Claude threads' views, serves
  `folderTrust` and `trustFolder`, and after a trust ends the folder's idle
  Claude threads' processes (send gate held, busy checked again; a thread
  mid-turn is waited for, up to 12 h; Away-held, pane-held and side threads
  are left) so the next send resumes them with the folder's settings.
  `provider_accounts.rs` (protocol 60, T3 §6) serves
  `iface:providerAccounts`, owns `provider-accounts.v1.json` and
  `session-accounts.v1.json` (opened only by the store owner), is the
  `AccountDirectory` the Claude, Codex and Grok `AccountRoutedAdapter`s read
  (installed in `bootstrap.rs` `install_account_routers`), and the
  `TerminalAccounts` that puts the provider's default account (a sign-in's
  named one, a reopened chat's bound one) on a terminal launch;
  `provider_capture/accounts.rs` lets a Claude pane's capture find an added
  account's store after the Default store finds nothing. Threads join
  `ThreadView.account` at read and team tools start a child on the caller's
  account (`ThreadHost::attach_accounts`).
  `threads/switch.rs` (protocol 59, T3 §5) is `switchProvider`: refusals,
  the target (back to an earlier segment through the guarded runtime's
  resume, else a new session in the thread's folder at the thread's rung or
  lower), one store commit, closing the old session under its send gate, and
  the handoff sent as a host prompt (an ACP session launches with it). It
  also answers chain history (`HistoryFallback::chain_history`, asked before
  the provider), maps search hits to the head, and `worker.rs` makes the
  head's delete, forget, archive and restore take its segments along; the
  team ledger follows the head (`team_follow`).
  Visual replies (protocol 61, T3 §8): `threads/team.rs` `show_visual`
  checks the page (`interface_settings::thread_visuals`), hands it to the
  journal worker (`JournalOp::Visual`: the worker writes the page to
  `thread-visuals/` and journals `visual` in pump order, then answers), and
  pushes `visual.shown` into the pane (`ThreadHost::push_session_event`).
  `threads/journal.rs` answers `iface:threadJournal` `visual` from the head
  and its segments, removes a session's pages with its journal, and
  `HistoryFallback::with_visuals` places the visuals into a provider's own
  history page (`dispatcher.rs` `history_page`). `iface_team` reads request
  lines up to 4 MiB (`McpServerSpec.max_request_line_bytes`).
  `threads/schedules.rs` (protocol 62, T3 §7) is scheduled prompts and
  pull-request watch over `interface_settings::thread_schedules`: the four
  commands, each thread's `ThreadView.schedules`, the worker's due pass (the
  next due time joins its deadline, at least once a minute while anything is
  scheduled; a run's end or a registry round re-offers waiting firings), the
  delivery gate and send (attempt recorded before the guarded runtime's
  send, as a host prompt; unknown never retried), the watch on the
  pull-request pass (`Gh` checks and review threads, fingerprint, the
  failing-checks prompt `ship.rs` builds), the team tools `schedule_prompt`,
  `list_schedules` and `cancel_schedule` (calling thread only), following a
  switch (`schedules_follow`), removal and archive, and `iface trigger`
  (`thread.schedule.trigger` on `host.sock`, served through
  `orchestration_transport::ScheduleTrigger` when this host owns it).
  `threads/resume.rs` (protocol 66, T3 §10) is Resume at reset: the run tap
  (`observe_limit`) records each session's usage limit from a rate-limit
  notice and clears it when a run completes; a notice without a reset reads
  the usage windows (`readUsage` on the session management owner,
  `attach_usage`) of the thread's own account; `ThreadView.limit` joins it;
  `resumeAtReset` makes the thread's one `resume` schedule (prompt from the
  owner's queued follow-up, else the prompt-recovery receipt read through
  `attach_prompt_receipts`, else the default). Delivery is `schedules.rs`'s,
  unchanged but for the text (`resume_prompt`) and, for a receipt's prompt,
  the send's `recovery` Retry that consumes it.
  Bind files the prepared baseline under `refs/interface/threads/<h>/0/start`
  (`thread_git::move_ref`, one `update-ref` transaction); discard and the
  hourly stale sweep delete unbound pending baselines.
  See the [threads design](../superpowers/specs/2026-10-06-threads-design.md).
- `registry_push.rs` — the debounced desktop push coalescer every owner shares
  (`RegistryPushCoordinator`, the two-rate `SharedRegistryPushes`) and the
  frozen `iface:registry` frame.
- `payload.rs` — the shared request-payload rules: what counts as empty
  (`null` or `{}`) and the object-only decode that refuses a non-object before
  serde sees it. Each owner passes its own refusal, so wire codes and messages
  stay per owner.
- `registry_access.rs` — the one way owners reach the session registry: `read`,
  `write` (drops the lock, then schedules the cache save), `sorted_visible`
  (clones under the lock, sorts after it), `content_revision`, `apply_event`.
  A poisoned lock is always a definitely-unsent registry error; each owner
  keeps its own mapping of a refused write.
- `collaboration.rs` — the 9 frozen collaboration channels, their errors and
  the traits the rest of the host implements or calls (session hooks, desktop
  control, runtime port, agent sessions). Its children hold the rest of the
  host side: `collaboration/port.rs` (the `SessionRuntimeCollaborationPort`
  effect port with target/source leases and the target event pump),
  `collaboration/service.rs` (`DurableCollaborationService`: the drive loop,
  previews, the 30 s approval sweep (which skips its transition while nothing
  awaits approval), cleanup and restart reconciliation) and
  `collaboration/translation.rs` (payload validation, DTOs). The packet v2
  builder is `interface_collaboration::packet`. A source session's
  events re-drive its live collaborations, except parked ones: a drive that
  finds a record awaiting approval or peer review (or unknown) parks it, and
  every explicit action, terminal cleanup and admission un-parks. Delivery
  waiting on its source is never parked.
- `collaboration/projection.rs` — live transcript/approval/delivery projection,
  revision checks and bounded text. The parent retains provider binding and
  durable driving; public projection types keep their original import paths.
- `terminal/lifecycle.rs` — reopen-authority transaction transitions, rollback,
  lifecycle publication and input-admission mirroring. It shares the parent's
  single sink/state; lock order is lifecycle, input lanes, then admission
  (lifecycle, then the phone replay table). Publish holds the lifecycle lock
  across the durable sidecar write and the push send, so async dispatch code
  reaches this state and the desktop terminal-state gate only through the
  blocking pool (`lifecycle_blocking`, `desktop_blocking`).
- `orchestration.rs` — the 23 frozen orchestration operations (dispatch
  matches `CHANNELS`), epoch recovery and the coordination port. Its children
  hold `impl OrchestrationHostDispatcher` blocks and their helpers:
  `orchestration/runtime_port.rs` (the `OrchestrationRuntimePort` trait, its
  `SessionRuntime` impl, launch access and the
  durable error codes), `orchestration/effects.rs` (the fail-closed
  provider-effect protocol: Reserve → Start → Settle exactly once, and the one
  reservation shape), `orchestration/observer.rs` (agent lifecycle
  observation and settlement) and `orchestration/adversarial.rs` (the
  adversarial-review choreography and pair keys; `startAdversarialReview`
  itself stays in the parent, whose refusal text the packaged smoke pins).
  The transcript renderer is `interface_orchestration::transcript`.
- `orchestration_transport.rs` — owner/client election over `host.lock`, the
  owner-private `host.sock` line protocol (1 MiB cap), `LocalHost` method
  routing, and the channel adapters. `worker_session_keys` is how Threads
  hides orchestration workers: the owner reads its whole agent list, a client
  pages `agent.list` to the end (never the 64-agent control snapshot).
- `session_management.rs` — the frozen 15-operation adapter over
  `RuntimeBridgeCommand`: payload decode, collaboration guards, registry pushes,
  import-progress sink, supplemental usage merge, legacy result shapes, and the
  `RenameHooks` around an owner's `renameSession` (settled renamed unless the
  failure was definitely unsent), and the `RemovalHooks` told of a confirmed
  `deleteSession` or `forgetSession`.
- `native.rs` — `NativeRuntimeView` (registry/history projection used by every
  non-chat slice) plus the native desktop slice: external terminal launch,
  managed-worktree list/open/cleanup.
- `terminal.rs` — integrated-terminal control-plane owner: eight lifecycle
  operations, per-generation output pumps, the single input pump and the bounded
  per-terminal input lanes it routes into, the transport failure monitor, the
  desktop workspace lease provider, the lifecycle push sink. Pumps and lanes
  live as long as their terminal, so each runs on a named thread
  (`iface-pump-<id>`, `iface-input-<id>`), not the Tokio blocking pool; a
  lane is dropped when its terminal's Removed event is delivered. A phone
  replay stream lives while its generation is pumping or visible: delivered
  Upsert/Removed events and each pump's end release the rest, so no request
  polls lifecycle state to prune it. `terminal.rs` keeps the owner, the shared
  types and the short `dispatch`; its children hold `impl
  TerminalHostDispatcher` blocks: `terminal/create.rs` (exact replay, saved
  and fresh restart authority, managed-spawn verification),
  `terminal/restart.rs`, `terminal/dismiss.rs` (and its background
  rollback), `terminal/authority.rs` (reopen-authority verification,
  migration and the create/restart transaction), `terminal/spawn.rs` (the
  shared exposure tail and uncommitted-generation rollback),
  `terminal/shutdown.rs` (`close` and shutdown reconciliation),
  `terminal/handoff.rs` (protocol 50: the one worker that hands an agent CLI
  that exited cleanly by itself a plain shell in its folder, fed by every
  delivered clean-exit Upsert whatever the window shows; it holds the
  lifecycle dispatch gate across the check, the create and the agent's
  dismissal) and `terminal/io.rs` (the pumps, lanes and transport failure).
- `terminal_transport.rs` — the fd3/fd4 binary protocol: 44-byte frame
  encode/decode with per-stream sequencing and EOF rules, the fair per-terminal
  outbound queue, reader/writer tasks with a shared failure watch.
- `mcp_transport.rs` — the owner-only Unix-socket MCP transport the voice and
  pane servers share: 0700 dir / 0600 socket binding (only a stale socket
  of the same owner is unlinked, and close removes only the socket whose
  (dev, ino) it bound), peer-uid check, newline JSON-RPC framing capped at
  1 MiB per line with at most 16 requests in flight per connection,
  `initialize` / `ping` / `tools/list` / `tools/call`, and the kernel's peer
  pid handed to every backend call.
- `pane_mcp.rs` — the `iface` MCP server every terminal pane reaches through
  one fixed socket (`<data root>/panes/iface.sock`): `pass_off` (a brief
  handoff from the calling pane, resolved from the peer pid's process
  ancestry), `read_pane` and `send_to_pane`. Only `read_pane` is pre-approved
  in Claude and Codex panes; `pass_off` and `send_to_pane` ask the owner
  unless Settings › Threads › Pane tools without asking is on (protocol 71:
  `PaneToolApproval` in `bootstrap.rs`, Claude's `permissions.allow` and
  Codex's per-tool `approval_mode`, `prompt` or `approve`; the shell pane's
  manual Codex reads `INTERFACE_MANUAL_PANE_TOOLS_WITHOUT_ASKING`). The switch
  is read from `threads.pane-tools-without-asking` whenever the launch catalog
  is built, and changing it rebuilds the catalog, so panes opened after the
  change follow it. Both tools are confined to the pane on the other end of a pass-off (the
  delegate reporting back to its source, or the source nudging its delegate);
  `read_pane` may also read the caller's own pane. Anything else is refused. Also
  Settings' opt-in for Grok and Antigravity (protocol 48): `read_pane_tools_entry`
  and `set_pane_tools_entry` read, write and remove the static
  `/bin/sh -c 'exec nc -U "$INTERFACE_PANE_MCP_SOCKET"'` `iface` entry in their
  user config through each CLI's own `mcp list|add|remove` (argv and parsing in
  `interface-terminal/src/pane_tools.rs`). Nothing is written at start; off
  removes only an entry that is exactly Interface's, and an owner's own `iface`
  server reads `taken` and is never touched. The pre-v47 marker
  `<data root>/panes/iface-mcp-config.v1` is no longer read.
- `capture_generation.rs` — the on-disk layout of a provider capture
  generation (one private directory per terminal process generation under
  the receipts root): entry names and bounds, `classify` into
  `GenerationEntry`, the root and writer locks, the pending-writer marker
  codec and owner-only file creation. The Claude and Codex `SessionStart`
  hooks (`claude_terminal_hook.rs`, `codex_terminal_hook.rs`) and the lookup
  share it, and every directory walker matches on `classify`. The hooks'
  other shared pieces (the `SessionStart` source table, the hook process's
  identity and parent from one `ps` call) live in `capture_hook.rs`, so
  neither hook imports the other.
- `provider_capture.rs` — `ProviderCaptureLookup`, the provider-session
  lookup behind the terminal service for Claude, Codex and OpenCode panes:
  capture owner records and their transactions (one `verified_owner` check
  for every lookup) and the startup sweep. Its tests stay in
  `claude_terminal_session_tests.rs`, the path landings pin. Its children hold
  the locked-generation protocol (`generation.rs`), receipt scanning and
  verification (`receipts.rs`), Claude's per-process session pointer
  (`claude_registry.rs`) and the `ps` identity probes (`process_probe.rs`).
  Before waiting for supervised captures to settle, startup revalidates the
  private capture owner and asks `interface-terminal` to settle only an exact stopped orphan group.
  Receipt reconciliation/retirement then uses the existing durable-ack path;
  active, uncertain and leaderless groups retain their fail-closed captures.
- `bounded_process.rs` — the host's one bounded child-process runner: its
  own process group, a deadline covering both the exit and the captured
  pipe's close (so an inherited pipe cannot outlive it), group teardown on
  timeout. Once the child exits and its group is empty, a pipe still held
  open elsewhere (on macOS, by a child another thread spawned before the
  pipe became close-on-exec) ends the read after 100 ms of quiet instead of
  timing the run out (`interface_common::process::CapturedPipe`; the async
  readers use `interface_engine::child_output`). The pane-process `ps`
  probes and the pane MCP config commands use it.
- `report_workers.rs` — optional report-worker adapter and shared OpenCode runner.
  `pane_mcp.rs` exposes its six tools as `cheap_agents` through the adjacent
  `cheap_agents.sock`; old `iface` worker calls remain compatible but hidden
  from discovery. Both endpoints share the same service and inboxes.
  Native pane calls use a fresh host-confirmed provider identity, project grants,
  and one shared durable report pool; see [`WORKERS_MCP.md`](../WORKERS_MCP.md).
- `pane_delegates.rs` — `CliDelegate`, the Away CLI's worker runner:
  explicit-file snapshots and a content-only Claude or Codex worker, returning
  a proposal. Pane delegation (its durable task service, workflow tools,
  hooks and OpenCode/Antigravity workers) was retired on 2026-10-14; see
  [the archive](../archive/pane-delegation-20261014/README.md).
  `readTerminalDelegation` stays served and answers unavailable.
- `delegation_hook.rs` — the `--interface-worker-notice` hook that brings
  report-worker notices into a pane at turn boundaries. Its retired
  `--interface-delegation-hook` mode is still accepted and does nothing.
- `voice/` — the voice assistant owner: helper supervision, the turn loop, the
  per-session `iface_voice` MCP server (`voice/mcp.rs`, over `mcp_transport`),
  its tool implementations (`voice/tools.rs`, two of them shared with the
  pane server) and the renderer UI bridge.
- `updater.rs` — updater channel adapter (three public operations plus two
  private native effects) and the update-state push forwarder task.
- `worktree.rs` — `SystemWorktreeService` over the one-writer managed-worktree
  ledger, with a degraded no-ledger mode. A ledger write runs on the blocking
  pool holding the mutation gate's owned guard and swaps the in-memory records
  once it commits, so readers see the prior records meanwhile and a caller
  that stops waiting cannot let the next writer start from a stale mirror.
- `worktree_release.rs` — the automatic release path: per-session-key gates, the
  ordered safety checks (inactive → not open in editor → git-verified →
  removable: no uncommitted changes and no git-ignored files, which
  `git worktree remove` would delete; no repository-wide prune),
  kept/removed notices, drain-on-close. Ledger lookups
  canonicalize every record path, so they run on the blocking pool; a
  workspace release does its lookup inside the tracked task `close` drains.
- `prompt_context.rs` — prompt-context dispatcher, the base64-v1 pasted-image
  wire decode with per-item refusals, the host authority projection.
- `antigravity_usage.rs` — supplemental Gemini quota reader: version-gated
  (`agy --version` asked again only when the executable file changes),
  serialized, timeout-bounded, parsed fail-closed into an `EngineUsage` row.
  The child can never open a browser (its `PATH` leads with a no-op `open`,
  which is how a signed-out `agy --print` would otherwise start Google
  sign-in in Chrome on every poll), and a failed read backs off instead of
  respawning at the panel's cadence.
- `opencode_usage.rs` — implements the supplemental usage trait directly on the
  engine crate's OpenCode Go allowance reader; bootstrap supplies the resolved
  executable and private runtime root. Quota and authentication policies stay
  in the crate.
- `instance_lock.rs` — the exclusive CLI instance lock: `O_NOFOLLOW` create,
  owner JSON with pid + random token, inode/ctime identity checks, stale
  recovery.
- `lib.rs` — the public re-export surface `apps/iface` consumes.

Tests: one integration binary, `tests/suite/main.rs`. `child_protocol.rs`
spawns the real binary and drives the JSON protocol end to end;
`live_legacy_data.rs` (ignored by default) opens a disposable copy of real
legacy data; `session_management.rs`; `agent_gate_live.rs` holds the live agent
approval-gate probes (ignored, and inert unless `INTERFACE_LIVE_PROBES=1`).

**Entry points** — `main.rs::main`; `HostServer::run` (`server.rs`);
`OperationOwnerRouter::dispatch` (`router.rs`, the single operation→owner table);
`open_host_services_at_with_orchestration_and_collaboration`
(`bootstrap.rs`, the only bootstrap entry `main.rs` uses);
`RuntimeHostDispatcher::dispatch` (`dispatcher.rs`);
`CollaborationHostDispatcher::dispatch` (`collaboration.rs`);
`DurableCollaborationService::start` (`collaboration/service.rs`);
`OrchestrationHostDispatcher::dispatch` and `::recover`;
`orchestration_transport::prepare_interactive`/`prepare_one_shot` (must run
before any authoritative store is opened);
`TerminalDataPlane::from_process_pipes` (`terminal_transport.rs`, over the
inherited fd3 host→Electron and fd4 Electron→host PTY pipes);
`AgentHost::new`, `AgentRuntimeGuard::new`, `AgentRunner::start` and
`RuntimeHostDispatcher::with_resume_requirements` (`agents.rs`, wired in
`main.rs`); `ThreadHost::new`/`::start`, `RuntimeHostDispatcher::with_send_hooks`
and `SharedRegistryPushes::start_observed` (`threads.rs`, wired in `main.rs`).

**Invariants** — the rules an editor must not break are in
`apps/interface-host/AGENTS.md`; this is the mechanism behind them.

- Replay safety is classified in `desktop_replay_is_safe`,
  `runtime_dispatch_error`, `delivery_boolean` and the `response-too-large`
  demotion.
- `open_runtime_at_with_epoch` fails if another process holds the leases, and a
  client process must not open orchestration at all.
- One saturated terminal is never allowed shared fate. A PTY write parks until
  its child reads, so input is applied on a bounded per-terminal lane, not on
  the shared pump; output backpressure parks that terminal's own pump thread
  (and through `OutputBuffer`, its PTY reader) rather than dropping bytes. A
  pump whose own credit window is full parks on its stream's own condvar,
  which only that stream's acknowledgement (or close/failure) wakes; pumps
  parked on the shared limits wait on `capacity`.
- Every renderer push goes through `PushFrame::new` (or `PushFrame::encoded`,
  which serializes typed rows once, straight to JSON: the whole-registry
  frame), and both reject non-push channels. Read a payload as a `Value`
  through `PushPayload`'s deref, or `decode()` it into a type.
- In the desktop host the runtime dispatcher, collaboration port and
  orchestration dispatcher share one whole-registry coalescer
  (`SharedRegistryPushes`, built in `main.rs` before any owner starts), so at
  most one registry frame leaves per window whichever owner scheduled it.
  Only `main` closes it, after `close_event_pumps()`: the event pumps stop
  forwarding once `schedule()` (or the push itself) fails but keep draining
  to the terminal frame, so the close projection and worktree release still
  run at quit, and provider shutdown relies on its flush for the final
  terminal registry frame. The orchestration dispatcher
  closes only its own `orchestration_event` coalescer. A shared coalescer
  that stops (closed channel, unreadable registry) stops registry pushes for
  every owner, which all read the same registry. Session management, managed
  worktree cleanup and automatic worktree release flush through it too
  (`with_shared_registry_pushes`), so the frame still precedes their
  response and the next scheduled round knows it was sent. The self-starting
  constructors (`with_pushes`, `set_pushes`) remain for tests and `iface`.
- The whole-registry coalescer is two-rate. A round whose
  `SessionRegistry::content_revision` equals the one its last frame left with
  changed only `last_activity`; it waits until 5 s after that frame, then a
  trailing frame always leaves. Status, title, meta, archive, origin, row-set
  and order changes still leave within the 50 ms window, and flush/close send
  at once. This deliberately departs from the 0.1.169 bridge, which
  rebroadcast every 50 ms window; the wire and payload are unchanged. Visible
  effect: rows sorted by `lastActivity` within a sidebar group (and
  `selectAgent`'s newest pick, the agent roster) reorder up to 5 s late. A
  runtime with no registry revision (test fakes) keeps the 50 ms cadence.
- `recover()` closes Reserved orchestration effects as
  `HOST_STOPPED_BEFORE_DISPATCH` and quarantines Started ones as
  `HOST_STOPPED_AFTER_DISPATCH`.
- All canonical orchestration mutations are serialized by the dispatcher's
  `gate`, including the async observer path. Read-only operations skip it
  deliberately. `snapshot()` serves the memo a successful commit left without
  the store lock (a commit holds it across its fsync); a failed write or
  close forgets the memo, so reads take the lock again and fail closed.
- `observe_agent` must spawn `settle_observed_agent` rather than awaiting inline:
  its callers already hold the gate.
- Owner-private modes are re-verified after creation: root 0700, non-symlink,
  parent uid; socket 0600 and a real socket; close unlinks only a socket whose
  (dev, ino) still matches.
- `channel_method` and `method_channel` must stay exact inverses for every
  migrated channel.
- Agents: the approval driver answers on the unguarded runtime; every other
  caller reaches the runtime through `AgentRuntimeGuard`. An owner answer for an
  agent-bound session is accepted only for a matching pending entry and sent
  once; an unknown automatic answer is never re-sent.
- Agents: a session whose registry cwd is inside the agents root is read-only
  (resume, send, `sendLive`, model, effort, fast-mode, permission changes refused
  definitely-unsent) unless it is a current thread or an unsettled run. The check
  runs in the guard and in `ResumeRequirements`, so the dispatcher's implicit
  resume before a send or setting change is covered too. An unavailable store
  leaves such a session read-only. Every resume of a bound session is forced
  gated with the binding's folders.
- Agents: approvals arriving while a thread or run launch is binding its key are
  buffered and drained after the bind; `respond` for a buffered key is refused
  "still checking" (definitely-unsent). New thread and provider change close the
  old thread session and clear its pending approvals.
- Collaboration: provider effects stay fail-closed (`delivery_disposition` maps
  only Confirmed + Persisted to success); the projection treats an unknown draft
  as queue depth 1 and `delivery_unknown` is sticky; target leases are
  identity-checked via `exact_target`, never key-checked; projection revisions
  strictly increase under `JS_MAX_SAFE_INTEGER`.

**Gotchas**

- The host code `TerminalService` callbacks reach is
  `HostTerminalLifecycleSink::publish` (via `blocking_send`) and
  `DesktopTerminalLease::drop`; neither may assume a Tokio runtime context.
- `HostServer::run` takes the push receiver out of a `Mutex<Option<..>>`; calling
  `run` twice on the same server silently yields no pushes the second time.
- `desktop_replay_is_safe` maps `DesktopEffectCertainty`, never error prose.
  The existing wire codes, messages and certainty fields remain unchanged.
- `shutdown::timed` measures each drain and warns after five seconds while
  continuing the original future; it does not cancel durable/provider work.
- With no terminal data plane, `main.rs` warns and keeps the control plane while
  terminal creation fails as `terminal-data-unavailable`.
- `SystemWorktreeService::degraded()` deliberately keeps records in memory after
  the durable ledger fails to open, and `persist` keeps the in-memory record even
  when it returns an error. A test pins that.
- `EngineEventKind::SessionClosed` arriving on the adapter stream is deliberately
  swallowed; the canonical desktop close is emitted only by
  `project_desktop_terminal_close` after the terminal frame proves the live
  attachment is gone. On a desktop queue overflow the stream errors before the
  runtime has evicted the session it fails closed, so the pump first waits
  (bounded, `wait_for_detach`) for that eviction.
- `AgentNodeState::Waiting` is reserved for an adversarial reviewer idling before
  the `/review` packet has been sent; `recover()` also exempts Idle agents whose
  task run already has evidence. Those two states are not interchangeable.
- `send_adversarial_review_request` ignores its transcript argument — the body is
  a fixed provider-native `/review` command.
- The coordination port is always `UnavailableCoordination` in production;
  `OrchestrationCoordinationPort` exists for the frozen coordinated-worker
  channels and the test fake.
- Stop/wake effects reserved without a stable key get a random idempotency key;
  only steer supplies a caller-stable one, so stop is intentionally not
  replay-deduplicated.
- `legacy_response` is not the read-history path: `iface:readHistory` is
  intercepted in `decode_runtime_request` and served from
  `read_provider_history`. The `read_history` arm inside `legacy_response` is an
  unreachable leftover.
- `observe_event`, `source_state`, `live_transcript` and `hydrate_history` all
  mutate the projection map via `entry().or_default()`, including on nominally
  read-only calls. Rows are never removed.
- Collaboration target approvals are auto-denied by the host. Do not route them
  through the ordinary renderer approval path.
- `DispatchError`, `CollaborationServiceError` and `protocol::BridgeError` are
  three structurally identical `{code, message, retryable, replay_safe}` types
  related by field-for-field `From` impls. Conversion happens at module
  boundaries via `map_err(Into::into)`.
- `main.rs` orders the shutdown drains broker-stop → collaboration →
  orchestration → runtime → pumps → registry pushes → worktree release →
  session cache flush (a release can redirect a session home after provider
  shutdown already flushed the cache).
- Release-smoke roots (`INTERFACE_HOST_SMOKE_*`) must be paired, private and
  outside the live home; `INTERFACE_HOST_TEST_DATA_ROOT` exists only under
  `debug_assertions`.

Terminal shutdown fences pane admission and finishes pane cleanup before draining
agent scheduling, Away, voice and report workers. A slow service cannot delay the
start of PTY teardown until Electron's outer host-stop watchdog fires. Durable
provider/service settlement still runs to completion; no timeout cancels it.

Saved panes after a restart are pinned by the cold-start restore matrix
(`terminal_restore_matrix_tests.rs`, 2026-10-15): every terminal profile on
fake executables × how the pane got its chat (an unconfirmed launcher tile, a
chat the host filed, a Session Library chat) × its folder (as saved, through a
symlink, moved with a link left behind, gone) × front or parked workspace ×
clean shutdown or a host that died without one. Each pane must reopen, or fail
with a code and a sentence; a moved folder must reopen once followed; a known
chat must resume as itself. The outcomes live in
`electron/src/fixtures/terminal-restore-matrix-v1.json`, which
`electron/src/renderer/terminal-restore-matrix.test.ts` replays through the
renderer's reopen planning, retry and shell fallbacks (never a blank tab, and
never a request the host matrix did not answer). The gate runs 68 of the 272
rows (about 8 s); `INTERFACE_RESTORE_MATRIX=full` runs them all (about 30 s),
and `INTERFACE_RESTORE_MATRIX_WRITE=1` rewrites the fixture after a deliberate
change. Not covered: the startup sweep that settles a dead host's capture
receipts on real processes (`claude_terminal_session_tests.rs`).

`provider_canary.rs` (protocol 51) keeps `provider-canary.v1.json`: the
`--version` line the host last checked for each found provider CLI, the
canary expectations it was checked against and the last change found. A
details listing (Settings) and one background pass 45 s after start run
`interface_terminal::provider_canary` for any CLI whose version or
expectations changed (help, version, listing and schema output and the
installed files; never a model turn), one at a time, and fill each profile's
`changedSinceLastCheck`. Only production attaches it (`main.rs`); a store that
does not open leaves the canary off.
