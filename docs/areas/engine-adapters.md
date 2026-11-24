# Engine adapters — engine-codex, engine-claude, engine-acp

Rules for editing here: `crates/AGENTS.md`. Navigation: `docs/CODEMAP.md`.

Three crates behind the shared `EngineAdapter`/`SessionHandle` traits. None of
them read credentials: the provider executable is the authentication boundary,
stderr is drained but never mirrored, and MCP descriptors carry an empty env.

Signed in or out (protocol 41) comes from each CLI's own answer only:
`EngineInfo.auth` from `claude auth status --json` (engine-claude `auth.rs`,
asked after the probe's control session started) and from Codex's
`account/read` (an account, or none where `requiresOpenaiAuth`); engine-codex
`auth.rs` runs `codex login status` for `iface doctor`. ACP agents report
nothing. A turn refused for that reason carries `cause: signedOut` on its
`error` event (Claude's `"error": "authentication_failed"` assistant message,
Codex's `codexErrorInfo: "unauthorized"`, ACP's "Authentication required").
Each adapter launches through a shared `interface_engine::ExecutablePath`
the host repoints on Models & providers → Refresh; every launch reads it once.

**crates/engine-codex** — OpenAI Codex `app-server`.

Protocol v22 permits an empty launch prompt to create an idle thread for durable
controllers. Its identity is available before the first explicit send; no model
turn is dispatched during this launch. Ordinary empty sends remain refused.

- `adapter.rs` — `CodexAdapter` and its `EngineAdapter` impl (probe, listing,
  stored-session operations, history and usage reads, create/resume), the agent
  modes, and thread-page parsing. Each part of a session is a submodule of
  `adapter/`; `adapter_tests.rs` reaches their private helpers through
  `super::*`, so the root re-exports those under `#[cfg(test)]`.
  - `lanes.rs` — the kept listing child, spare history lanes and isolated
    control connections. Listing and stored history reuse one supervised idle
    control child per adapter, with three extra history lanes created only on
    contention (a spare lane's child exits after two unused minutes), without
    caching provider results; `listing_reuse` (60 s) lets the runtime skip
    asking again after a complete listing. Codex 0.157.1 answers at most 100
    threads a page whatever the limit. Cancellation, failed/incomplete reads
    and child exit discard the affected child; mutations retain isolated
    connections and are never replayed.
  - `launch.rs` — launch/resume admission. `open.rs` — `make_handle`,
    `open_session`, the collaboration fence and source, thread start/resume,
    and verifying the opened thread. `gate.rs` — agent approval modes, their
    `config.toml` overrides and the response check.
  - `session.rs` — `SessionShared` (its lock order), the `SessionHandle` impl,
    closing, `session_inbound_loop`. `turns.rs` — send, steer, turn start.
    `config.rs` — mid-session model/effort/approval/Fast changes.
  - `commands.rs` — the built-in and skill command catalogue and its refresh.
    `dispatch.rs` — running a typed provider command. `approvals.rs` — server
    requests, approval facts (file-change paths, shell-wrapper unwrapping) and
    answers. `models.rs` — the model catalogue and Fast tiers.
- `protocol.rs` — method-name constants, typed request params with frozen
  camelCase wire shapes, RPC-outcome → `DeliveryOutcome` translation,
  `CodexSessionState`'s turn/thread bookkeeping.
- `transport.rs` — `CodexTransport`: the shared
  `interface_engine::supervised::SupervisedTransport` with Codex's
  `CodexTransportEvent` classification and nouns.
- `mapper.rs` — live notifications and stored thread items → `EngineEvent`s,
  unified-diff reconstruction, tail-bounded history paging, usage mapping; a
  reached rate limit carries its window's reset as `RateLimitNotice.resetsAt`
  (protocol 66).
- `executable.rs` — codex binary resolution, PATH augmentation, the exact
  `app-server` argv including the optional collaboration MCP `-c` argument.

**crates/engine-claude** — the user's own `claude` executable.

- `store/cache_file.rs` — the checked read both read-only Claude transcript
  caches use: the no-follow open with its identity/version check, the
  complete-prefix fingerprint (`PrefixHash`: 1 MiB blocks at fixed offsets, so
  a grown transcript re-verifies its whole old prefix on up to four threads),
  the record reader with its one 192 MiB record cap, and the post-read version
  check (each cache keeps its own message). A transcript Claude appends to
  mid-read fails as changed; both caches keep what they verified and read
  again, up to `READ_ATTEMPTS` in all. A listing whose transcript is still
  moving after that shows the row it last read and stays complete; only a
  transcript never read yet leaves it incomplete.

- `store/history_cache.rs` — bounded read-only parsed history tails. Every read
  checks file identity/version; appends verify the old byte prefix, then parse
  new complete lines. Partial final records, truncation and replacement retain
  the full reader's semantics. The explicit all-history path remains uncached.
  A tail whose records, at 4× their bytes, exceed `CACHE_BYTES_PER_FILE` keeps
  only a tail index (offset and length of each kept record, no parsed values)
  and re-parses just those records; a page of another size starts from the
  same transcript's tail and reads back line by line for the older records it
  needs. An index that no longer matches its file falls back to the full read.

- `adapter.rs` — trait impls: session task, command handling, approvals,
  advisory refreshes, model catalogue, usage mapping, store error translation.
- `transport.rs` — supervised child: writer/reader/supervisor tasks, control
  request routing, host-request bookkeeping, delivery certainty.
- `launch.rs` — permission modes, setting sources, the exact argv/`LaunchSpec`
  builder and its input validation.
- `executable.rs` — claude binary resolution order (override, login-shell
  `PATH`, known install paths), shared by the desktop host and `iface`.
- `protocol.rs` — streaming-JSON frame classification, user-message shape, and a
  **crate-local** NDJSON decoder.
- `mapper.rs` — stream/system/result messages → `EngineEvent`s, `tool_summary`,
  SDK context-usage normalization; a rejected `rate_limit_event` keeps
  `rate_limit_info.resetsAt` (Unix seconds) as `RateLimitNotice.resetsAt`
  (protocol 66).
- `store/listing_cache.rs` — incremental Claude summary fields with complete-prefix
  verification, file identity checks and retry of partial EOF records.
- `store.rs` — the `~/.claude/projects` store: listing, history replay,
  rename/tag/fork/delete transactions, safe-path helpers. `session_exists`
  (the reopen history check) refuses a linked project entry, except a link to
  another project directory directly in the same store: a moved project folder
  leaves one, and it adds no transcript the scan does not already reach.
  `session_id_from_transcript_path` accepts that same sibling alias for captured
  Claude receipts, checking the alias identity/target and the real directory
  and transcript metadata. This lets startup retire the old capture generation
  and resume its exact conversation after a project move. Store-root links,
  transcript links, outside targets and unsafe metadata remain refused.
- `title.rs` — Threads' one-shot generated titles: `claude -p --output-format
  json --model claude-haiku-5-5 --no-session-persistence` (Claude Haiku 5.5
  by its exact id, not the moving `haiku` alias; Write with AI shares it
  through `one_shot_arguments`) with `--tools ""`, no setting
  sources or MCP servers and `--safe-mode`, the excerpt on stdin (2 KiB each
  for the first prompt and, when there is one, its answer; the instruction
  works from the prompt alone), 30 s, process group killed on timeout,
  the same scrubbed environment as every Claude spawn
  (`transport::configure_claude_environment`).

**crates/engine-acp** — ACP v1 over stdio, three flavors.

- `adapter.rs` — flavor gating, probe, session open/inbound loops, permission
  brokering, prompt send, launch/resume validation, model catalogue.
- `protocol.rs` — ACP v1 DTOs, method names, `classify_route` (routed frames →
  `AcpInbound`), the `AcpSessionState` lifecycle machine.
- `transport.rs` — `AcpTransport`: the shared
  `interface_engine::supervised::SupervisedTransport` with ACP's
  `AcpTransportEvent` classification and nouns.
- `mapper.rs` — ACP session updates → `EngineEvent`s (tools, diffs, plans,
  commands, usage, subagents).
- `profile.rs` — strict profile validation, built-in Grok/Qwen argv, executable
  resolution and PATH augmentation.
- `qwen.rs` — owner-managed Qwen MLX backend: frozen system settings file, health
  probe, spawn/stop lifecycle, process environment overlay.
- `grok_store.rs` — read-only Grok session store plus billing/usage mapping.
- `opencode_usage.rs` — serialized, bounded OpenCode Go account allowance reads.
  A private `opencode models opencode-go` process loads the transport hook from
  `opencode_usage_transport.mjs`; OpenCode supplies auth to that hook for one
  read-only `GET https://opencode.ai/zen/go/v1/usage`. Credentials never cross
  into Rust or the renderer, no credential files are inspected, no model is
  called, and no user/project plugin or instruction is loaded. Rust validates
  the private v1 frames and projects rolling, weekly and monthly percentages
  with provider reset times into the unchanged `EngineUsage` contract. Missing
  or malformed windows remain unknown; local spend is never used as quota.
  Both this probe and `opencode_worker.rs` use the same private runtime
  directory (`opencode_worker::RuntimeDirectory`, built on
  `interface_engine::worker::RuntimeDirectory`) and the shared
  `interface_engine::worker::{ProcessGroup, cancelled, drain}` helpers, and
  set the one `OPENCODE_DISABLED` list. Cross-language fixtures live in
  `electron/src/fixtures/opencode-go-usage-*-v1.json`; the ignored live reader
  test requires `INTERFACE_TEST_OPENCODE` and performs no model call.

**Agent approval gate and approval facts** — for [agent teammates](agents.md).
`LaunchOptions`/`ResumeOptions` carry `approvalGate` and `additionalDirectories`.
An adapter that cannot guarantee that every policy-relevant act raises
`approval.requested`, or cannot grant the folders natively, refuses
definitely-unsent. `EngineAdapter::approval_facts(tool, input)` turns the
provider's own approval payload into provider-neutral `ApprovalFacts`; the
trait default is `Unknown`, and any missing or malformed field stays `Unknown`
so the policy asks the owner.

- engine-claude. Gate: permission mode `default`; the flag-layer `--settings`
  JSON gains a `PreToolUse` hook (matcher `*`) printing
  `permissionDecision: "ask"` and `"disableAllHooks": false`; `forced_ask`
  includes the gate, so the host never answers `can_use_tool` itself. Resume
  applies the same gate. A gated Chat is refused, and a gated session refuses
  any permission-mode change other than `default` before the CLI is contacted.
  An Auto or Bypass agent (`agentAccess`) launches in Claude's `auto` or
  `bypassPermissions` mode without the hook; `forced_ask` includes it too,
  because what still reaches `can_use_tool` is Claude escalating past its own
  mode (verified on Claude Code 2.1.282: Sonnet in auto sent none for a shell
  command, a write and a GET; Haiku fell back to `default` and asked for the
  write and the curl). It refuses a change to any other mode.
  Folders become `--add-dir` pairs ahead of `--permission-prompt-tool`; relative
  paths are refused. Facts: `Bash` → Shell; `Read` → Read; `Grep`/`Glob` →
  Search; `Edit`/`MultiEdit`/`Write`/`NotebookEdit` → Write; `WebFetch` →
  WebFetch GET; `WebSearch`; `TodoWrite`, `Task`, `Agent`, `BashOutput`,
  `KillShell`, `KillBash`, `ListMcpResourcesTool` → Internal;
  `mcp__<server>__<tool>` → Mcp (split at the first `__`); everything else,
  including `AskUserQuestion` and `ExitPlanMode`, → Unknown.
- engine-codex. Gate: `thread/start` and `thread/resume` send
  `approvalPolicy: untrusted`, `approvalsReviewer: user`, sandbox
  `workspace-write` and `config.sandbox_workspace_write` with `writable_roots`
  (cwd plus folders), `network_access: true` and `exclude_slash_tmp: true`.
  `verify_gate` requires the response to echo the policy, reviewer,
  `workspaceWrite`, `networkAccess`, `excludeSlashTmp` and every requested root
  (lexical path equality; extra roots are accepted), or the open fails
  outcome-unknown before the launch prompt. The gate refuses a read-only or
  full-access sandbox and any permission mode but `untrusted`; folders without
  the gate are refused; a gated session refuses approval-policy changes. A
  gated launch also spawns its own app-server with `-c features.X=false` for
  every default-on acting feature Codex runs without an approval (`apps`,
  `browser_use`, `computer_use`, `in_app_browser`, `multi_agent`,
  `unified_exec`, `unified_exec_tty`, from the set `worker.rs` disables;
  `code_mode_host` stays on because 0.154.0 runs commands through it), and
  `gate_config`'s `config/read` both lists the MCP servers to switch off and,
  when it reports a `features` table, fails definitely-unsent if any of those
  is still enabled (0.154.0 omits the table, so the spawn argv is the
  authoritative disable). Ungated launches are unchanged.
  Ungated params keep their frozen wire shape. Facts: `command` → Shell with the
  input `cwd` (an argv array is shell-quoted per element); a `kind` other than
  `command` (such as `writeStdin`) or a `networkApprovalContext` → Unknown.
  Codex reports every command inside its login-shell wrapper
  (`/bin/zsh -lc '<script>'`), so exactly one wrapper is unwrapped to its
  script: POSIX-unquoted to three words `zsh`/`bash`/`sh` (bare or in `/bin`,
  `/usr/bin`, `/usr/local/bin`, `/opt/homebrew/bin`), `-c` or `-lc`, and a
  script without NUL; the argv form needs the same three elements. Any other
  shape, unquoted metacharacter, expansion inside double quotes or unbalanced
  quote keeps the command as sent, and the policy asks about the wrapper.
  `fileChange` → Write over `interfacePaths`, which Interface owns: paths are
  stashed per item from `item/started` (including move targets) and
  `item/fileChange/patchUpdated` (256 items; an unreadable change poisons its
  item), and a provider-supplied `interfacePaths` is stripped. A `grantRoot` →
  Unknown.
- engine-acp. Gate: `approvalGate: true` or any `additionalDirectories` is
  refused for Grok, Qwen and custom profiles on launch and resume, because no
  way to force ask mode over the owner's Grok Build config was found (probe
  P3). Facts, by ACP tool kind: `read` → Read, `edit`/`move` → Write, `delete` →
  Delete, `search` → Search, `execute` → Shell, `fetch` → WebFetch GET, `think`
  → Internal. Paths come from `rawInput` `path`, `file_path`, `abs_path` and
  `paths`.

**Entry points** — `CodexAdapter::new` / `with_collaboration_bindings`, its
`EngineAdapter` and `SessionHandle` impls, `open_session`,
`session_inbound_loop`, `CodexTransport::spawn`, `CodexEventMapper::map_into`
(`map` is the allocating test wrapper over it);
`AcpAdapter::new`/`probe`/`create_session`/`resume_session`/`dispose`,
`validate_profile`/`builtin_profile`/`resolve_builtin_executable` (called from
`apps/interface-host/src/bootstrap.rs` and `apps/iface/src/app.rs`),
`AcpTransport::spawn{,_with_environment}`;
`ClaudeAdapter::new_with_projects_root`, `run_session{,_with_binding}`,
`handle_command`, `ClaudeSession::launch_with_initialization`.

**Invariants**

- Every send maps to Confirmed / DefinitelyRejected / DefinitelyUnsent /
  OutcomeUnknown, and an unknown is never widened into a retryable-unsent one.
- Codex turn bookkeeping is fail-closed: `turn_start_unknown` gates both new
  prompts and Stop, and `last_completed_turn_id` stops a late response from
  resurrecting a completed turn. Never clear either on an unknown outcome, and
  never synthesize an interrupt without both exact provider ids.
- Codex manual compaction has no turn id: `start_compact_command` notes an
  unidentified control before the request; a definite refusal clears the fence,
  an unknown outcome deliberately does not.
- Codex steering: only shapes in `steer_fallback_safe` may fall back to
  `turn/start`. `activeTurnNotSteerable` must stay a `DefinitelyUnsent` refusal.
- An ambiguous ACP prompt quarantines the session in
  `AcpPhase::PromptOutcomeUnknown`; only a `session/update` carrying the same
  session id proves delivery. engine-claude does the same with an
  `outcome_unknown` flag cleared only by an inbound provider frame.
- `ClaudeSession::respond_to_host_request` removes the pending host request on
  `Ok` *and* on `OutcomeUnknown`, and resets `responding` only on
  `DefinitelyUnsent`. That is what prevents a replayed approval.
- A cancelled ACP permission is answered `{"outcome":{"outcome":"cancelled"}}`,
  never with a deny option; a permission response only selects an option the
  agent actually offered.
- ACP's `session/prompt` answers only when the turn ends, so its
  `send_reporting_dispatch` fires once the prompt frame is queued
  (`request_with_timeout_reporting_dispatch`): the runtime then lets an
  approval or Stop for that turn through. Every other adapter keeps the
  default, which fires when `send` returns.
- `bypassPermissions` is spawn-time only in both engines, and resume never
  recovers a volatile permission policy from the stream. It applies only a
  standing choice: an agent's access, or (7 Oct 2026) the mode the owner last
  chose for any other session, which the host keeps across restarts in
  `session-access.v1.json` and passes as `ResumeOptions.permission_mode`.
- A gated launch never loses its gate mid-session: Claude and Codex refuse the
  permission or approval-policy changes that would remove it, and a Codex gate
  mismatch never sends the launch prompt.
- Launch is argv-only in all three: no shell, no interpolation. ACP
  `validate_profile` rejects `sh`/`bash`/`zsh`/`dash`/`fish`/`env`, relative
  paths, control bytes and credential-shaped arguments before any spawn.
- Wire shapes are frozen, including every `skip_serializing_if` omission: an
  omitted `serviceTier` means "inherit", `Some(None)` means "restore Standard".
- Emitted `EngineEvent` JSON is asserted against oracles, including the
  cross-language fixture
  `electron/src/fixtures/provider-history-user-image-events.json`.
- Codex skill paths are private: only the derived command name and a
  path-scrubbed description may reach a `ProviderCommand`.
- Collaboration leases must be disposed on every exit path. Depth-1 sessions
  enter the depth fence and never register a source.
- Team tools (`iface_team`, protocol 57) go only to an ordinary code session
  at collaboration depth 0 (`team_tools_eligible`), through
  `interface_engine::team_tools`: Claude in `--mcp-config`, confirmed before
  launch; Codex in the thread config (`required: false`, a 660 s tool
  timeout), confirmed when `thread/start` answers; every ACP flavor in
  `session/new`/`session/load`, confirmed when the agent names the session.
  A binding that cannot be made is a notice, never a failed launch; it is
  disposed with the session.
- Several accounts per provider (protocol 60, T3 §6): an adapter is one
  account. An added account's adapter carries an environment overlay set on
  every CLI it starts, after Interface's own entries (Claude
  `ClaudeAdapter::with_environment`, `ClaudeLaunchOptions.environment`, the
  title and `auth status` calls; Codex `CodexAdapterConfig.environment` on
  every app-server, so its own sessions and control lanes; Grok
  `AcpAdapterConfig.environment`), and its own store root (Claude projects,
  Grok sessions). An empty overlay (the Default account) sets nothing. The
  host's `AccountRoutedAdapter` (`interface-runtime/src/accounts.rs`) picks the
  account; the adapters never see `LaunchOptions.account`.
- No std `Mutex` guard is held across an `.await`.
- Fork at a turn (`fork_session_at`, Threads) cuts after the owner prompt an
  `interface_engine::ForkBoundary` names by identity: its `prompt_digest`
  (`interface_engine::prompt_digest`, the store's digest — the host pins the
  two together) and `later_same_digest`, how many later recorded turns repeat
  it. Each adapter walks its own prompts from the newest and takes the
  (`later_same_digest` + 1)-th match, so history from before a thread, a
  terminal's prompt or a counted steer cannot shift the cut; no match refuses
  definitely-unsent (`FORK_BOUNDARY_NOT_FOUND`). Codex knows each
  `thread/read` turn holding an owner prompt by its first prompt
  (`mapper::fork_turn_boundary`; a steered prompt belongs to its turn, a
  host-context-only turn is skipped) and sends `thread/fork` with the
  inclusive `lastTurnId`; an unloaded turn before the match or a running
  boundary turn refuse. Claude walks the prompts the history page shows, less
  the user messages Claude Code writes itself (`[Request interrupted by
  user]`, its `for tool use` form, `!` bash echoes — the page still shows
  them), and copies the transcript through the last assistant entry before
  the next prompt (`store::fork_at`), the entry `--resume-session-at` would
  cut at: the CLI writes a `--fork-session` copy only once a first prompt is
  sent, so a CLI fork would name a session that does not exist yet. ACP keeps
  the trait's refusal.

**Gotchas**

- `drain_control_requests` swaps `connection.inbound` for a dummy channel and
  moves the real receiver into a spawned task — anything you later do to
  `connection.inbound` affects the dummy.
- `interface_engine::provider_search_path` augments the spawn PATH for both the
  Codex app-server and engine-claude. engine-acp's `builtin_search_path`
  deliberately keeps its own copy — see the comment on it in
  `interface-engine/src/executable.rs`.
- `mapper::text` treats `""` as absent. That is parity behavior; do not simplify
  it into a plain `as_str()`.
- `advertised_names` is a permanent refusal fence — names are never removed, so a
  withdrawn skill yields "no longer available" rather than a plain prompt.
- `dispatch_provider_command` returns `Confirmed(())` even when the command
  failed; failures surface as Notice/StatusChanged events.
- Codex collaboration `process_enabled`/`startup_unavailable` are latched for the
  adapter's lifetime: one initialize failure disables it for every later session.
- engine-claude has its **own** `NdjsonDecoder`/`ProtocolError`; engine-acp uses
  the shared one from `interface-engine`. They differ: the claude one fails the
  whole chunk on a bad line, the shared one returns per-line results and
  resynchronises after an oversized line.
- One ACP resume-capability predicate: `resume_method_for`, used directly by
  probe and through `AcpSessionState::resume_method` at open time. Grok is
  additionally hard-coded resume-capable even when the handshake omits the bit.
- The civil-date → ISO conversion is one copy in `interface_common::time`
  (`days_from_civil`/`civil_from_days` plus the `IsoFraction` renderer). The
  three trailing-fraction formats the callers need — `None`, `Zeros`, `Millis` —
  are frozen bytes, not formatting taste.
- `store::in_project` retries other project directories **only** on NotFound, so
  a mutation is never blindly retried against a second candidate file.
- An ACP session id is provisional (`pending-acp-N`) until `session/new` answers;
  updates arriving before then are buffered (max 100) and replayed only on an id
  match.
- `ClaudeSession::close()` early-returns when the transport already flipped
  `closed`; final teardown then depends on `Drop` and `kill_on_drop`.
- A stop the provider acknowledged while a turn ran goes idle at once; the
  stopped turn's own tail up to its `result` is its end, not new activity:
  `end_stopped_turn` drops its inactive thinking signal and the error of an
  `error_during_execution` result (Claude Code 2.1.325 sends one with an
  `[ede_diagnostic]` error). Any other result stays as mapped, and a new send
  closes the window.
- Qwen's backend is a real local server on `127.0.0.1:8080`, and SIGTERM is
  delivered by spawning `/bin/kill` because the crate has no libc dependency.
- Tests in engine-acp and engine-claude run real fake CLIs written to tempdirs
  over the actual transport, so a framing or argv change surfaces as an opaque
  shell-test hang, not a compile error.
- engine-acp and engine-codex share one transport,
  `interface_engine::supervised::SupervisedTransport` (spawn, request family,
  reader, writer, stderr and supervisor tasks), and one session core,
  `interface_engine::session_core::SessionCore` (readiness, emit, and the
  single report of a transport's death). Each crate keeps its event enum and
  its `Classify` impl, which supplies the nouns in the frozen prose, plus
  `drain_control_requests`, `Opening` and the inbound loop's handling of each
  event. The inbound queue is bounded by event count and by the stdout bytes
  actually queued, and when it is full the stdout reader waits (backpressure
  onto the child's pipe) instead of failing the transport. Only a single frame
  over the frame limit is refused.

## Provider replay (test-only)

`crates/provider-replay` checks the adapters against recorded real provider
sessions instead of only hand-written fake CLIs (design and contract:
`docs/superpowers/specs/2026-11-14-t3-systems-design.md` §4).

- `fixtures/<engine>/<scenario>.json` are real recordings (`simple`,
  `tool-approval`, `interrupt`, `resume` for Claude and Codex), scrubbed by
  the recorder. Never write or edit one by hand: re-record with
  `apps/interface-host/tests/suite/provider_record_live.rs` (ignored, live,
  `INTERFACE_LIVE_PROBES=1 INTERFACE_RECORD_DIR=…`, from a plain shell), or,
  after a scrubber fix, re-scrub a kept journal (`refinish_replay_fixtures`).
- `provider_replay.rs` (host suite) runs each scenario as the renderer does:
  `createSession`, thread bind, `send`/`respond`/`interrupt`/`closeSession`/
  `resumeSession` through a `RuntimeHostDispatcher` with its event pump, a
  `ThreadHost` with its run tap, and the real native adapter pointed at a
  wrapper script that sets `INTERFACE_REPLAY_TRANSCRIPT` and runs
  `target/<profile>/provider-replay` (built by the crate's own integration
  test, so `cargo test --workspace` and the gate build it; a lone
  `cargo test -p interface-host` needs `cargo build -p provider-replay`).
- An adapter that changes what it sends (a new field, a renamed method, a
  missing request) fails the replay with the expected and received frame in
  the test's message. A change in what the provider sends needs a new
  recording.
- The recorder and the replay suite share the scenario code, so the frames a
  replay expects are the ones the host sent when recording. Launch options are
  fixed per engine (`Engine::launch`): Claude `haiku` with settings isolation,
  Codex `gpt-6-luna` at low effort with `untrusted` approvals.
