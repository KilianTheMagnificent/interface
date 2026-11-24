# apps/iface

Module map, entry points and mechanism for the standalone Ratatui TUI and the
`iface` CLI. Rules for editing here: `apps/iface/AGENTS.md`. Navigation:
`docs/CODEMAP.md`.

The explicit bounded Away demo is documented in [`AWAY_PILOT.md`](../AWAY_PILOT.md).

**Modules**

- `workers.rs` / `workers/`: opt-in shared report broker CLI, bound native
  Claude/Codex launchers, private MCP bridge, local notice hooks and explicit
  grant/revoke commands for the native pane broker. Domain state
  lives in `interface-collaboration::workers`; see [the worker pilot](../WORKERS_MCP.md).
  Both launchers register the MCP as `cheap_agents` (formerly `interface_workers`).

- `away.rs` — disposable-project pilot CLI, signal/stop adapter and protected evaluator;
  durable queue decisions belong to `interface-collaboration::away`.
- `away_cycle.rs` — shared Codex/Claude command, packet/reviewer and cancellable
  check adapters, and the `--name value` pair parser `away pair` also uses;
  `interface-collaboration::away_cycle` owns deadlines and acceptance.
- `theme.rs` — `iface theme convert <file> [--format …]`: reads a colour
  scheme within 1 MB, converts it with `interface_settings::convert_scheme` and
  prints the CSS (stdout) and a suggested file name (stderr). It never writes
  into the themes folder; the user redirects the output there.
- `lib.rs` — module declarations shared with `main.rs`, kept in the library
  target so release version stamps only recompile `main.rs`.
- `main.rs` — release version stamp (`interface_common::product_version!`),
  argv parse, environment construction, stdin read, and the only place process
  output and the exit code are emitted.
- `argv.rs` — the stable top-level grammar, `CLI_USAGE`, `TuiMode`/`LaunchScope`/
  `OutputFormat`, duration parsing, cross-flag validation. `agent`/`orchestrate`
  (and `mcp`) args are captured verbatim and not interpreted here.
- `app.rs` — CLI dispatch, `AppEnvironment`/`AppResult`, the interactive startup
  ownership transaction and teardown, one-shot orchestration host open/close,
  `SessionRuntime` construction, provider-executable resolution (inherited
  `PATH`, then the desktop host's bounded `bootstrap::LoginShellPathProbe`,
  started before the stores open when a provider would need it; Codex and
  Claude through their engine crates' resolvers, as the host does; the path
  found is kept, never canonicalized, so an updater's repointed link takes
  effect without a restart), per-command stdin limits.
- `tui.rs` — interactive control: the `TuiController` and `TuiApp` structs
  and their state, `TuiApp`'s composer, overlays, slash commands, pickers and
  keys, and `run`, the startup and shutdown drain.
- `tui/controller.rs` — `impl TuiController`: provider discovery, provider
  starts and resumes, the event pump, sends, steers, setting effects and
  shutdown.
- `tui/settings.rs` — the launch-settings commands on `TuiApp`: `/cwd`,
  `/engine`, `/model`, `/effort`, `/fast` and access, their pickers and
  setters, and the access cycle key.
- `tui/reducer.rs` — pure reduction of provider events and stored history into
  a session row, and the context-telemetry sample; no locks or awaits. A diff
  cell keeps at most 64 KiB of each side (`truncated`), and a transcript keeps
  4 MiB of diff text, releasing the oldest diffs' text first; only the details
  overlay reads it.
- `tui/approval.rs` — pending approvals: raw question validation, the approval
  keys and the answer sent back.
- `tui/collaboration.rs` — `/consult`, `/handoff`, `/advisor`, `/collab`,
  `/peer`, the input guard for sessions a collaboration owns, and their parsers.
- `tui/orchestration.rs` — `/agents`, `/orchestrate`, `/spawn`, `/send`,
  `/inbox`, `/monitor` over the shared orchestration host.
- `tui/terminal.rs` — terminal and signal lifecycle: `TerminalSession` setup
  and teardown, `ShutdownSignals`, the frame-capped `run_terminal_loop`, and
  terminal notifications.
- `tui/effects.rs` — provider starts, sends, stops, steers and resumes started
  from the keyboard. `TuiApp::key` queues them instead of awaiting; the
  terminal loop runs one at a time as a task and applies its result, so
  drawing, other sessions' output and Ctrl+C/SIGTERM stay live. Keys and
  pastes that arrive meanwhile wait and replay in order afterwards.
- `tui/agent_control.rs` — agent socket commands against the same `TuiController`
  state and delivery gates, independent of keyboard/overlay presentation.
- `tui/commands.rs` — pure slash-command grammar and `/spawn` / `/send` parsers.
- `tui/view.rs` — Ratatui drawing and view formatting. First paint uses cached
  sessions; independent provider discovery follows, fenced against changed,
  live or removed rows. Explicit resume/continue still await fresh discovery.
  Transcript layout is incremental and windowed: every transcript edit goes
  through `tui/transcript.rs`'s `Transcript`, which records the lowest absolute
  cell index changed per version, so a frame restyles only from the first
  changed unit (a cell, or a whole command run, which can grow) and copies just
  those cells under one short read lock; no `Arc` of the cells is retained, so
  edits never deep-clone them. Tool events find their cell through the
  transcript's call-id index, not a scan. Heights come from ratatui's
  `unstable-rendered-line-info` `Paragraph::line_count` (the same `WordWrapper`
  that renders), and a frame renders only the lines in view. The fullscreen
  view is a `TranscriptScroll`: following the newest rows, or held on a display
  unit's absolute cell index plus rows into it, so held rows stay put while
  output arrives or old cells drop, with no row cap. Up/PageUp clamp at the
  oldest row, Down onto the newest rows or End follows again (0.1.169).
- `composer.rs` — cursor-aware text buffer with a UTF-8 byte cursor kept on
  scalar boundaries.
- `history.rs` — append-only NDJSON prompt history at the frozen legacy path.
- `steer_history.rs` — durable steer-attempt ledger: validated begin/settle,
  256 global / 40 per-session retention, atomic fsync+rename snapshot writes,
  corrupt-file quarantine.
- `exec.rs` — one non-interactive provider turn through `SessionRuntime`:
  resume-prefix resolution, quiescence detection, fail-closed classification.
  A start carries its prompt at launch, so failing to take its event stream is
  outcome-unknown (`stream_failure`). A runtime-shutdown or lock-release
  failure after the turn is a stderr warning that keeps the turn's stdout and
  exit code (`app::with_teardown_warnings`).
- `agent_server.rs` — TUI-side control server: uid-checked listener with a
  5 s request read deadline, method dispatch, target resolution, the durable
  `agents.json` name store, and the durable mailbox with begin/settle
  receipts, byte-budgeted inbox paging, monitor and wait. `agent.list` puts
  live and named sessions first and stops at the result budget (`live: true`
  lists only live ones). Mutating verbs take a per-session-key lock (send,
  steer, stop, resume, focus) or the naming lock (start, rename), so a stop
  never waits for another session's start or resume.
- `agent_client.rs` — CLI-side agent grammar, cross-verb option validation,
  Unix-socket RPC (read-only verbs time out after 30 s; mutating verbs never
  do), text/JSON result rendering with delivery-aware exit codes. A send,
  steer or start the TUI never received (nothing listening, or the line not
  fully written) exits 3, retryable; one whose TUI went away after the whole
  line was written exits 4, `retryable: false`, with a hint to check before
  resending (a departure from 0.1.169, which said retry for both).
- `agent_protocol.rs` — shared wire types and the single-frame, 1 MiB,
  newline-delimited JSON codec.
- `agent.rs` — the frozen `iface agent` usage text and the local `--help` fast
  path that must never open the control socket.
- `app.rs` `trigger_command` — `iface trigger <name or id>` (T3 §7): asks the
  Interface that owns `host.sock` to run a thread's scheduled prompt now
  (`interface_host::trigger_thread_schedule`, never becoming an owner); sent
  or waiting exit 0, not sent 3, unknown 4.
- `mcp.rs` — `iface mcp [--data-root <dir>]` (protocol 65, T3 §11): a stdio
  MCP server a Claude Code or Codex outside Interface runs. It answers
  `initialize` and `tools/list` itself (the host's catalogue) and proxies each
  call over one connection to the host's outside socket, saying `hello` with
  the client's declared name; it talks only to a socket and directory its own
  user owns. No Interface: every call answers `interface-not-running`. A call
  lost after it reached the host answers `outcome-unknown` for start, send
  and stop (never retried); the next call connects as a new session.
- `orchestration.rs` — the `iface orchestrate` verb surface over
  `OrchestrationHostControl`, including the fail-closed provider-host gate and
  its `--name value` / `--name=value` option grammar.
- `collaboration_runtime.rs` — the CLI-local collaboration lifetime: prepare
  journal + broker before adapters, attach the runtime port, quiesce and drain.
- `background.rs` — `--bg` child spawn with owner-private logs, the durable
  `bg-sessions` row store, liveness reconciliation, kill, bounded log tail.
  A row's pid counts as its child only while that process's `ps` start time
  is within 5 s of the row's `startedAt`; a reused pid lists `stale` and
  `kill` never signals it.
- `doctor.rs` — `iface doctor` snapshot assembly, text/JSON rendering,
  credential redaction, the context-telemetry reader. The text report ends
  with the provider CLI drift canary (`interface_terminal::provider_canary`;
  `app.rs`); `iface doctor providers` prints the canary alone (`--output
  json` too) and exits 1 when an installed CLI no longer matches what
  Interface depends on. The JSON of plain `doctor` keeps its legacy shape.
- `diagnostics.rs` — the process diagnostics sink: sanitized in-memory ring plus
  bounded append log, pruning, the context-telemetry writer.
- `config.rs` — `config.json` load with quarantine-on-corrupt, atomic
  owner-private save, remembered engine/model/effort.
- `paths.rs` — pure, filesystem-free path resolution. The 0700/0600 modes are
  `interface_storage::{OWNER_DIRECTORY_MODE, OWNER_FILE_MODE}`, applied through
  `private_write.rs`.
- `sanitize.rs` — `for_display`, the terminal-control stripper every untrusted
  string must pass through.
- `secure_tail.rs` — TOCTOU-hardened bounded UTF-16 tail read.
- `errors.rs` — the frozen `CLI_EXIT` table and the `DeliveryOutcome` → exit-code
  mapping.

Tests: `tests/tui_pty.rs` — real-PTY integration for fullscreen/inline startup,
resize, palettes, SIGTERM=143 / SIGINT=130, and teardown.

**Entry points** — `main.rs` `#[tokio::main] main`; `argv.rs` `parse_argv`;
`app::interactive_command` → `tui::run` (the only interactive path, and the
startup ownership transaction: diagnostics sink → signal handlers → tty check →
instance lock → orchestration host lease → collaboration coordinator →
`SessionRuntime`); `tui::run` → `TuiController::new` (`tui/controller.rs`) → `start_agent_server`
(`agent_server.rs`, called from `tui.rs`) → `TerminalSession::start` →
`run_terminal_loop` (`tui/terminal.rs`); `impl AgentController for TuiController`; `TuiApp::key` /
`TuiApp::submit`; `exec::run` (`runtime: None` is the proven local refusal path);
`agent_client::run_agent_command`; `DiagnosticsSink::install` (before any
provider can be constructed); `collaboration_runtime::prepare`.

**Invariants** — the rules an editor must not break are in
`apps/iface/AGENTS.md`; this is the mechanism behind them.

- Both ends sanitize: the agent server calls `sanitize::for_display` on the way
  out and the client again on the way in, covering provider output, mailbox
  bodies, telemetry and error messages.
- `AgentDeliveryKind {AdapterAccepted, DefinitelyUnsent, OutcomeUnknown}` maps
  1:1 onto `DeliveryOutcome` and `ProjectedSendOutcome`. OutcomeUnknown must
  always reach the user and must never be auto-retried.
- Mutating an existing session requires `collaboration.guard_session_mutation`
  first when a collaboration runtime is present.
- `RwLock` order in `TuiController` is state → launch → launch_commands/probes,
  and no guard may be held across an `.await`.
- Keyboard-driven provider starts, sends, stops, steers and resumes go
  through `TuiApp::queue_effect`, never an inline await (approval answers and
  setting changes still await inline). One runs at a time, later input waits
  for its result (so a second Enter finds the composer already taken),
  queued-send failures wait too, and a quit or signal drops the handle rather
  than aborting it; runtime shutdown fences what is left.
- A prompt refused before any provider was called goes back to the composer
  (`restore_prompt`): collaboration and provider-command refusals, a send's
  NotFound/NotLive/Blocked, and a start proven unsent or rejected. A start
  whose outcome is unknown shows the r/x banner instead (`UnknownDelivery`
  with no key), and its `r` starts again.
- `acquire_cli_instance_lock` runs before `build_runtime`, and the orchestration
  host lease carries its own `CliInstanceLock` released on `Drop`.
- `Composer::cursor` is a UTF-8 byte offset always on a scalar boundary.
- A failed mailbox settle downgrades the response to outcome-unknown in memory
  only; `load` re-marks surviving `Requested` rows as unknown on restart.
- `load` and `begin` prune settled mailbox messages settled over 14 days ago or
  beyond the newest 2,000. `Requested` and outcome-unknown rows stay (a retry
  of their id must not send twice), as does every parent a kept reply names;
  `next_cursor` never moves back. A pruned id can be sent again as new.

**Gotchas**

- `agents.json` outlives the TUI process but `TuiController`'s session map does
  not. Any code unioning durable names with live sessions must tolerate a missing
  session.
- `scroll` means two different things: `TuiApp::scroll` is the transcript's
  `TranscriptScroll` (follow, or held on an absolute cell; a hold on another
  transcript, after a session switch or clear, falls back to follow), while
  the details overlay's `scroll` is a plain top-anchored row offset.
- Ctrl+Y (fullscreen only, as in 0.1.169) queues `scrollback_dump`; the
  terminal loop writes it before the next frame through
  `TerminalSession::dump_to_scrollback`: leave the alternate screen, print with
  CRLF (raw mode translates nothing), return, then a same-size
  `Terminal::resize` to repaint everything. Not `Terminal::clear`: it queries
  the cursor position, which blocks on a terminal that never answers.
- `--continue` picks the newest `last_activity` in the launch directory
  (RFC 3339 instants, else text; a tie keeps the lower key), not the first key.
- `Paragraph::line_count` is an unstable ratatui API (the manifest enables
  `unstable-rendered-line-info`); a ratatui bump must keep it equal to what
  renders. The differential tests in `tui_tests.rs` compare the windowed render
  against a full `Paragraph` at every scroll offset.
- `refresh_launch_commands` deliberately clears the catalogue to `known: false`
  before awaiting, so `provider_command_problem` fails closed during a refresh.
- `parsed_app_command` lowercases and rewrites `_` to `-`, and
  `is_handled_app_command` accepts aliases not in `APP_COMMANDS`, so the palette
  list and the accepted grammar deliberately differ.
- `for_display` consumes whole CSI/OSC sequences, which is why double-sanitizing
  is safe.
- `TuiController::shutdown` and `app::shutdown_interactive_runtime` both run on
  the normal exit path; the double quiesce/drain is deliberate and relies on
  idempotence.
