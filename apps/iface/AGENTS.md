# apps/iface

The standalone Ratatui TUI and the `iface` CLI. Same Rust services as the
desktop host, no Electron. `src/lib.rs` holds the crate's modules so
`src/main.rs` alone expands `interface_common::product_version!()` and the
library stays cached across release version bumps; no external crate links it,
so `pub` confers no external API and is not evidence that an item is
load-bearing.

Module map, entry points and the mechanism behind these rules:
`docs/areas/iface.md`. Read it before editing here.

## Owns

Argv grammar and the one-shot commands (`exec`, `orchestrate`, `doctor`,
`ps`, `logs`, `kill`, `theme convert`), the interactive TUI, the agent control socket on both
ends, and the CLI-owned durable stores (`config.json`, prompt history, steer
history, `agents.json`, the mailbox, `bg-sessions`, diagnostics). It owns no
domain rules — session lifecycle, orchestration and collaboration live in
`crates/`; this crate drives them and renders results.

Two structural facts worth knowing before you start: `src/main.rs` is the only
place process output and the exit code are emitted, and `src/tui.rs`'s
`TuiController` is simultaneously the owner of in-memory session state and the
`AgentController` served over the socket, so every mutation must satisfy both
the human UI and the socket API.

## Do not break

- Untrusted text reaching a terminal goes through `sanitize::for_display` first.
  Double-sanitizing is safe and common here.
- Exit codes come only from `errors::CLI_EXIT`. Definitely-unsent (3) and
  outcome-unknown (4) must stay distinct — scripts branch on the difference.
- Steering is fail-closed: `SteerHistoryStore::begin` writes a Pending row
  durably **before** provider contact, and `settle()` is idempotent.
- `AgentMailbox` writes a `Requested` receipt before the provider call and
  settles after. Never merge begin/settle into one write.
- `agent.send` idempotency is keyed on `messageId` + identical intent; a reused
  id with different content is a hard error, never a second send.
- Durable writes are create_new temp + fchmod 0600 + fsync + rename, temp removed
  on failure. Owner-private modes are 0700 dirs / 0600 files, re-asserted on read
  rather than trusted.
- The agent socket takes exactly one newline-terminated frame per connection,
  1 MiB cap, same uid only. Inbox pages and transcript tails stop at
  `AGENT_RESULT_BUDGET_BYTES`, so a reply always fits; one that still cannot
  be framed is answered with an error under the request's id, never silence.
- One process opens the stores: the instance lock runs before `build_runtime`.
- `paths.history` is the frozen 0.1.169 prompt file. The engine history is a
  process-local projection that writes nothing; a durable journal would need
  its own name, never `history`.
- `orchestration::run_inner` fails closed (exit 3) for provider-bound verbs when
  `live_provider_host` is false. New provider-touching verbs join that list.

## Gotchas

- `agent_server::valid_engine` is deliberately weaker than `argv::is_engine_id`.
  Wire compatibility: tightening it would refuse frames the server accepts
  today. Do not "consolidate" them.
- `CliPaths::review_checkpoints`, `worktrees` and `registry` are constructed and
  read nowhere — `interface-host` owns those stores. They describe the frozen
  on-disk layout; leave them.
- `SteerHistoryStore::flush` and `PromptHistory::flush` look like no-ops. They
  are lock barriers proving no append is mid-flight at shutdown.
- `config::read_config` re-asserts the parent directory mode as a side effect of
  a read, and deliberately goes no further: creating or refusing the parent
  would route a symlinked data directory into `quarantine_corrupt`.
- `exec.rs`'s `--timeout` is a real deadline bounding the whole run on both
  paths. Expiry after provider contact reports "outcome-unknown"
  and never cancels the session; only the pre-dispatch resume refusal is
  "definitely-unsent" and closes it. The JSON envelope deliberately echoes the
  caller's raw `--resume` prefix, not the resolved key.
- `send_route` in `tui/agent_control.rs` (behind `AgentController::send_agent`)
  has an `unreachable!` that holds only because every writer setting status
  `NotLive` also sets `live = false`.

## Verify

```
cargo test -p iface                # unit + tests/tui_pty.rs (real PTY)
cargo clippy -p iface --all-targets -- -D warnings
cargo xtask check                  # canonical gate
```

`tests/tui_pty.rs` drives a real PTY and asserts SIGTERM=143 / SIGINT=130 and
control-file teardown. It is the only coverage of the terminal lifecycle — do
not skip it when touching `tui.rs`, `tui/controller.rs`, `tui/terminal.rs` or
`app.rs`.
