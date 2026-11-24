# Pane pass-off tool — design

Date: 2026-09-05. Branch: `feat/pane-pass-off-tool` (worktree
`/private/tmp/interface-pane-passoff-20260905`, off `origin/dev` `1f59aeb`).
Status: approved in chat by the owner; implemented on the branch (see the
"As built" notes at the end).

## Goal

An agent running in an Interface terminal pane can hand a small job to another
model with one tool call and a prompt it writes itself, instead of the human
clicking Pass off and the whole rendered transcript travelling. The new pane
opens beside the caller, does the job, and types its answer back into the
caller's pane so the answer lands in the big chat as a new message.

Example: Claude in a pane calls `pass_off({profile: "codex", prompt: "Rename
FooBar to Baz across crates/interface-terminal and run its tests"})`. A Codex
pane appears beside it with only that brief, and when done calls
`send_to_pane` on the Claude pane with a two-line report.

## Non-goals

- Structured results, streaming, or a reply channel other than typing into the
  caller's pane.
- Tools for the Chat surface's engine sessions (they already have their own
  MCP paths).
- Manual `claude` / `codex` started from a shell pane: they get the pane
  environment (so a manual `grok` / `agy` does get the tool through its static
  config entry), but the inline argv for Claude and Codex is only added to the
  host-launched profiles. Follow-up: add it to `claude_manual_wrapper.rs`.
- Qwen and Shell profiles (no prompt binding; `bind_handoff_prompt` refuses
  them today and keeps refusing).

## What already exists, reused as-is

- **Pass Off (UI).** `IntegratedTerminalCenter.passOff` reads the source
  xterm's lines, sends `createTerminal` with `handoff {version, sourceTerminalId,
  transcript, fromTitle}`; the terminal service (`service.rs::pass_off_launch`)
  writes the transcript to `$TMPDIR/Interface/handoffs/<id>.md` (0700 dir,
  0600 file, unclosable fence), binds a host-templated positional prompt with
  `launch.rs::bind_handoff_prompt`, and starts the pane at the source agent's
  live cwd (`pass_off_directory`). Spec: `2026-08-31-terminal-pass-off-design.md`.
- **`iface_voice` MCP server** (`apps/interface-host/src/voice/mcp.rs`): Unix
  socket, newline JSON-RPC, `initialize` / `ping` / `tools/list` / `tools/call`,
  reached by the CLI through `/usr/bin/nc -U <socket>`. Tool helpers in
  `voice/tools.rs`: `start_pane` (dispatches `CHANNELS.create_terminal` through
  `TerminalHostDispatcher`), `read_pane` (`read_tail`), `send_to_pane`
  (`write_input` + `\r`), `find_pane`, `pane_json`, `parse_profile`.
- **Per-launch provider config already in argv** (`bootstrap.rs`): Claude gets
  `--settings <inline json>` with the SessionStart capture hook; Codex gets
  `-c` overrides including `hooks.SessionStart`. Per-pane environment
  injection exists for Codex (`CODEX_SESSION_CAPTURE_TOKEN_ENV`).
- **Live process facts by shelling out** (`proc_cwd.rs` via `/usr/sbin/lsof`,
  `embedded.rs` via `/bin/ps`). The workspace forbids `unsafe`.

## Architecture

```text
pane (claude|codex|grok --no-leader|agy)
  └─ nc -U <socket>  ── one fixed socket per host ──▶ interface-host: pane MCP server
                                                       ├─ peer pid (tokio UCred::pid, LOCAL_PEEREPID)
                                                       ├─ /bin/ps ancestry → which pane called
                                                       └─ tools: pass_off, read_pane, send_to_pane
                                                            pass_off → CreateTerminalRequest{handoff{kind: brief}}
                                                                        → existing pass-off path
                                                                        → lifecycle push carries passedOffFrom
renderer: acceptLifecycle sees a host-created terminal with passedOffFrom
          → split beside the caller's pane (fallback: balanced attach), file it into the workspace
```

### 1. One socket per host

`<data_root>/panes/iface.sock` (data root is the profile's Application Support
directory, so Dev and production hosts have different sockets). Bound once at
boot right after the terminal dispatcher exists, unlinked on shutdown next to
`VoiceService` teardown. Directory 0700, socket 0600, path checked against the
103-byte Unix limit like the voice server. A helper `pane_mcp_socket_path(data_root)`
is the single source of that path for the server, the launch catalog, and the
pane environment.

Why not one socket per pane: Grok and Antigravity only take MCP servers from
their user config files, so their entry has to be static.

### 2. Which pane is calling

`serve_connection` reads `stream.peer_cred()?.pid()` (tokio 1.53 supports it on
macOS) and hands it to the backend with every `tools/call`. The terminal crate
gains `proc_ancestry.rs`: one `/bin/ps -axo pid=,ppid=` (bounded like
`proc_cwd.rs`), parsed into a pid→ppid map, walked upward from the peer pid at
most 32 hops. `TerminalService::terminal_for_process_ancestor(pid) ->
Option<(TerminalId, TerminalGeneration)>` matches against the live entries'
`control.process_id()`. No match → the tool call fails with
`"Interface could not tell which pane called; run the agent inside an Interface terminal pane."`
Only `Running` entries match.

Grok panes launch with `--no-leader` (verified accepted by grok 1.0.13) so the
agent and its MCP servers run in the pane's own process instead of the shared
leader daemon; otherwise `nc`'s ancestor would be the leader and the walk would
find nothing.

### 3. Per-provider wiring

Every pane launch (all profiles, shells included) gets
`INTERFACE_PANE_MCP_SOCKET=<socket path>` in its environment.

| Profile | How the pane learns about the server |
| --- | --- |
| Claude | `--mcp-config '{"mcpServers":{"iface":{"type":"stdio","command":"/usr/bin/nc","args":["-U","<socket>"],"env":{}}}}'` appended to the catalog argv. The existing inline `--settings` JSON gains `"permissions":{"allow":["mcp__iface__pass_off","mcp__iface__read_pane","mcp__iface__send_to_pane"]}` so the three tools never prompt. Do not use `--allowedTools` (variadic; would swallow the trailing handoff prompt). |
| Codex | Two more `-c` overrides beside the existing ones: `mcp_servers.iface.command="/usr/bin/nc"` and `mcp_servers.iface.args=["-U","<socket>"]`. |
| Grok | Catalog argv `--no-leader`. Static user-config entry written once (below). |
| Antigravity (`gemini` wire id, `agy`) | Static user-config entry written once (below). |

Static entry for Grok and Antigravity, identical for both and independent of
which host is running:

```text
command: /bin/sh
args:    -c  exec /usr/bin/nc -U "$INTERFACE_PANE_MCP_SOCKET"
```

Written at host boot in a background task, never blocking boot, only when the
marker file `<data_root>/panes/iface-mcp-config.v1` is absent, and only for
executables the catalog actually resolved:

```sh
grok mcp add --scope user iface /bin/sh -- -c 'exec /usr/bin/nc -U "$INTERFACE_PANE_MCP_SOCKET"'
agy  mcp add iface -- /bin/sh -c 'exec /usr/bin/nc -U "$INTERFACE_PANE_MCP_SOCKET"'
```

Each spawn is bounded (10 s), stdin null, output captured through the existing
capped reader, failure logged at warn and retried next boot (the marker is written only
once every command for a resolved executable has succeeded). Bump the
marker suffix if the entry ever changes. This is the one out-of-repo side
effect of the feature; the owner approved it.

### 4. Tools on the `iface` server

Server wire name `iface`, title "Interface", instructions: "Tools for agents
running inside Interface terminal panes. pass_off opens a new agent pane beside
yours holding only the prompt you give it; the new agent reports back by typing
into your pane."

| Tool | Input | Effect |
| --- | --- | --- |
| `pass_off` | `profile` enum `claude\|codex\|grok\|antigravity`, `prompt` string (1 byte .. `TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES`), `title?` string | Builds `CreateTerminalRequest{ id: random, cwd: caller.cwd, size 120×40, title: title or profile label, launch: profile, launch_session: None, handoff: { version 1, source_terminal_id: caller, transcript: prompt, from_title: caller title if it passes the host's title rules else omitted, kind: brief } }` and dispatches `CHANNELS.create_terminal` exactly as voice `start_pane` does. Returns JSON `{pane_id, title, cwd}`. |
| `read_pane` | `pane_id`, `lines?` (1..400, default 60) | Last lines of that pane's rendered text (`read_tail`). |
| `send_to_pane` | `pane_id`, `text`, `enter?` (default true) | Types into that pane (`write_input`), only when it is on the other end of a pass-off with the caller: the delegate reporting back to its source, or the source nudging its delegate. Any other target, or an unidentifiable caller, is refused. |

`read_pane` and `send_to_pane` are the voice implementations, shared, not
copied. The voice server keeps its own tool table; the transport (`bind`,
`accept_loop`, `serve_connection`, `run_request`, framing helpers) moves to a
shared module parameterised by a small server spec (wire name, title,
instructions, tool catalogue, allowed names). `ToolBackend::call` gains the
peer pid argument; the voice backend ignores it.

### 5. Brief kind on the handoff

`TerminalHandoffV1` gains `kind: Option<TerminalHandoffKind>` (`transcript` |
`brief`, serde default `None` = transcript, `skip_serializing_if` none). TS
mirror in `electron/src/shared/ipc.ts`: `kind?: "transcript" | "brief"`. The
renderer never sends it; only the tool does. Contract round-trip test extended;
an unknown kind is refused.

Brief file (`write_handoff_file`) header replaces the transcript header:

```text
# Pass-off brief

From: <from title or "another agent"> (pane <caller id>)
Working directory: <cwd>

When the task is done, report the result to the agent that sent it: call the
send_to_pane tool with pane_id <caller id> and a short message, then stop.

<fence>text
<prompt>
<fence>
```

Brief launch prompt (`handoff_prompt`): "Another agent<from> passed you a
task. Read <file>, do it in this directory, and report back the way the file
says." The transcript wording is unchanged.

### 6. Placement beside the caller

`TerminalSummary` gains `passed_off_from: Option<TerminalId>` (serde default,
skip if none), set from `handoff.source_terminal_id` when the entry is created
and kept across restarts of that entry. TS mirror: `passedOffFrom?: TerminalId`.
`electron/src/main/ipc-payload.ts` does not validate summary fields, so nothing
else on the wire changes.

Renderer (`IntegratedTerminalCenter`): terminals created by this renderer are
remembered in a ref set of minted ids before `api.createTerminal` is called
(both `createTerminal` and the batch reopen path). In `acceptLifecycle`, an
upserted terminal that is not in that set, not in the current layout, not in
any workspace record, and has `passedOffFrom`:

1. If `terminalPaneForTerminal(layout, passedOffFrom)` exists → `splitTerminalPane`
   beside it with the same orientation the `+` menu's default pass-off uses.
2. Else → `attachTerminalsBalanced` into the active layout.
3. Then `file(terminal, terminal.launchSession, activeWorkspaceId)` so it is
   saved into the workspace and reopens after a restart like any other pane.

The pure decision lives in `terminal-workspace.ts` (`placePassedOffTerminal`)
with a unit test; the component only wires it. Voice `start_pane` panes stay
as they are today (untiled); out of scope.

## Files touched

- `crates/interface-contracts/src/terminal.rs` — `TerminalHandoffKind`,
  `TerminalHandoffV1.kind`, `TerminalSummary.passed_off_from`, tests.
- `electron/src/shared/ipc.ts` — the two mirrors.
- `crates/interface-terminal/src/{launch.rs,service.rs,proc_ancestry.rs,lib.rs}`
  — `--no-leader` is catalog data (bootstrap), but the env var injection, brief
  wording, `passed_off_from` on the entry, and `terminal_for_process_ancestor`
  live here. `pass_off_launch` chooses header and prompt by `kind`.
- `apps/interface-host/src/bootstrap.rs` — socket path helper, Claude
  `--mcp-config` + `permissions.allow`, Codex `-c mcp_servers.*`, Grok
  `--no-leader`, server start, config-writer task.
- `apps/interface-host/src/pane_mcp.rs` (new) — server spec, `PaneTools`
  backend, `pass_off`; `apps/interface-host/src/mcp_transport.rs` (new, moved
  out of `voice/mcp.rs`) — shared transport.
- `apps/interface-host/src/voice/{mcp.rs,tools.rs}` — use the shared transport;
  expose `read_pane` / `send_to_pane` helpers for reuse.
- `electron/src/renderer/terminal-workspace.ts` + test,
  `components/IntegratedTerminalCenter.tsx`.
- Docs: `docs/areas/interface-host.md` (module map), `docs/areas/persistence-and-services.md`
  (pass-off bullets), `docs/areas/renderer.md` (placement), `docs/VOICE_ASSISTANT.md`
  (transport now shared).

## Security notes

- The prompt is agent-authored text that only ever lands in the 0600 brief
  file; argv still carries only the host-templated prompt naming the file.
- Caller identity comes from the kernel's peer pid plus process ancestry, never
  from tool input. A caller that cannot be matched to a live pane is refused.
- The socket is owner-only (0700 dir, 0600 file). Peer uid must equal the
  host's uid; anything else is dropped before parsing.
- No secrets in any config entry; the env var holds a path under Application
  Support.

## Tests

- contracts: handoff `kind` round-trip and refusal; summary `passedOffFrom`
  round-trip.
- terminal crate: brief header and prompt wording; `--no-leader`, env var on
  every profile including shells; ancestry walk against a fake pid→ppid map
  (hit, miss, cycle, hop cap).
- host: Claude argv carries `--mcp-config` before the trailing prompt and
  `permissions.allow` in the settings JSON; Codex `-c mcp_servers.*`; `pass_off`
  builds the expected `CreateTerminalRequest`; unknown peer refused; config
  writer runs the two commands once and honours the marker (fake executables).
- renderer: `placePassedOffTerminal` (beside caller, fallback, already placed,
  renderer-minted skipped); existing CSS guard tests untouched.
- Gate: full `cargo xtask check` (touches `electron/src/shared/` and
  `crates/interface-contracts/`).

Live verification after the Dev app restarts on the pushed code: from a Claude
pane, `pass_off` to Codex; confirm the pane opens beside it, does the job, and
the report appears in the Claude pane; then the same from Grok and Antigravity
panes.

## As built (2026-09-05)

Deviations from the sections above, all small:

- **Manual Codex in a shell pane gets the tools too.** The manual Codex
  wrapper shares `codex_terminal_capture_arguments`; it now passes the socket
  from the pane environment, so a bare `codex` in a shell pane carries the
  same `-c mcp_servers.iface.*` as a curated pane. The manual Claude wrapper
  is still the follow-up the non-goals name.
- **Placement orientation is always horizontal** (beside, left/right). The
  menu's pass-off measures the pane to choose; a lifecycle event has no pane
  size, and beside is what "beside the caller" means.
- **The peer uid check compares against the socket file's owner**, read once
  at bind, rather than `getuid()` (an `unsafe` extern the workspace forbids).
  The 0700 directory already keeps other users out; this is the second lock.
- **`send_to_pane` is exposed to every pane but confined to the pass-off edge.**
  The caller is resolved from the peer pid exactly as `pass_off` does, and the
  write is allowed only when `target.id == caller.passedOffFrom` (the delegate
  reporting back) or `target.passedOffFrom == caller.id` (the source nudging
  its delegate). Anything else, shells included, is refused with one sentence
  naming the rule (`may_send_to_pane`, unit-tested). Without this, any pane's
  agent could type commands into any other pane, pre-allowed in Claude panes.
- **The static-entry marker is once per data root.** A CLI installed after
  that first boot gets no `iface` entry until
  `<data root>/panes/iface-mcp-config.v1` is deleted.
- **Pre-allow lands in `permissions.allow` of the inline settings JSON** the
  manual Claude wrapper also receives; allowing tools a session does not have
  is inert there.
