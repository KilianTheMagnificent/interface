# Cheap Agents MCP

The MCP server is named **`cheap_agents`** in both native Interface panes and
the standalone Claude/Codex launchers. Ask your lead:

> Use the cheap_agents MCP to delegate this review to Muse Spark 1.3, then check its results.

Two models are admitted: `opencode/muse-spark-1.3-contributor-free` (Muse
Spark 1.3, free contributor tier) and `opencode-go/grok-4.6` (Grok 4.6 through
the owner's OpenCode Go subscription). Both run through the same read-only
OpenCode report runner; a request names one exactly, with no fallback.

The start tool is `worker_start` (`mcp__cheap_agents__worker_start` in Claude).
The other tools are `worker_models`, `worker_collect`, `worker_followup`,
`worker_cancel`, and `worker_review`.

The same Rust MCP now serves Claude and Codex leads. A lead starts a bounded
report, continues independent work, collects it, and accepts or rejects the exact
result. The broker owns one queue and separate lead inboxes. Legacy mandatory
delegation was retired on 2026-10-14.

Native Claude and Codex panes advertise the six report tools through
`cheap_agents`, at `<host-data>/panes/cheap_agents.sock`. The Rust host checks the live provider process and its
private, corroborated session receipt on each call. Inbox ownership follows the
provider and conversation, so reopening that conversation recovers its results;
a requested resume ID alone does not grant access. Separate conversations have
separate inboxes, with two running slots shared across projects in this host.

The general `iface` MCP still provides pane controls. Its old worker tool names
remain callable for existing clients but are omitted from discovery to avoid
duplicate tools. Both endpoints use the same broker, grants, inboxes, task IDs
and retry receipts; renaming does not create or replay jobs. Notice hooks keep
using the existing pane socket. The standalone launcher formerly registered
`interface_workers`; new launches register `cheap_agents`. Existing launcher
processes and manually named MCP configurations continue to reach the same
`iface workers mcp` command. Request schemas and stored records are unchanged.

The feature is available in hosts built with this change. It does not hot-patch
an already running production host or restart an existing Fable job. Promotion,
installation and restarting production remain separate actions.

## Native pane source grants

Sharing is denied by default. The owner can configure an exact project and file
list without restarting the host, using its actual runtime data directory:

```sh
iface workers grant \
  --root '<host-data>/panes/report-workers' \
  --project /absolute/project \
  --allow-contributor-data \
  --allow-file src/example.rs

iface workers revoke \
  --root '<host-data>/panes/report-workers' \
  --project /absolute/project
```

Grant/revoke only updates private local configuration; it does not read source
files or contact a model. The host rereads the grant for every new task. Revocation
blocks future admissions; cancel already admitted tasks explicitly. No grant
operation is exposed to models. The source is the caller's current approved
checkout, and the grant must match that exact directory. Collection remains
available after a grant is revoked. A graphical grant editor is not included.

## Start a broker and a lead

Use the built `iface` binary. Keep the broker root short and outside the source
tree/iCloud; it must be a real owner-private directory (0700) or not yet exist.
Parents must already exist. The broker acquires its writer lease before binding
the private socket. Only one broker can open that root.

```sh
iface workers serve \
  --root /private/tmp/interface-workers \
  --project /absolute/project \
  --opencode /absolute/path/to/opencode \
  --allow-contributor-data \
  --allow-file src/example.rs
```

The owner supplies `--allow-contributor-data` only after approving transmission
to `opencode/muse-spark-1.3-contributor-free`. Submitted prompts and completions
can contribute to future Meta training. Omitting this flag makes task starts
fail definitely unsent. The same grant authorizes sending the packet to
`opencode-go/grok-4.6`; there is no separate per-model grant. `--allow-file` is repeatable and names exact files;
additional paths remain denied. A single broker scope is available to both lead
clients, without sharing their conversations or results. The command itself is
the explicit grant for this pilot; no source-sharing dialog is implemented yet.

In another terminal, start either lead:

```sh
iface workers launch --root /private/tmp/interface-workers \
  --client claude --executable /absolute/path/to/claude --model claude-opus-5

iface workers launch --root /private/tmp/interface-workers \
  --client codex --executable /absolute/path/to/codex --model gpt-5.6-sol
```

`--prompt <text>` runs one isolated, noninteractive lead turn for testing. These
test leads have ordinary command tools disabled and no inherited MCP servers.
Interactive launchers preserve the CLI's existing model and effort unless
`--model` or `--effort` is supplied. Claude retains other MCP servers. Test turns
use low effort by default and isolated tools/configuration. Muse uses medium effort. Credentials remain owned
by each CLI; Interface does not read or copy them. No global client settings are
written. Codex receives exact-definition trust for the three first-party notice
hooks, following Interface's existing launcher mechanism; it never receives a
global hook-trust bypass. Its six report tools receive scoped process-local
approval, while broker sharing/file checks still apply. Ordinary external tool
permissions are unchanged.

The launcher registers its kernel-reported PID and start identity with the
broker. MCP bridges and hooks must descend from that registered launcher, and
every request proves it again against a process-table read that began after
the request arrived; requests that arrive while one read runs share the next.
Models cannot pass an owner ID to tools. Reconnecting the MCP while the launcher
is alive retains ownership. A new launcher cannot claim a former lead by title
or guessed task ID. These standalone pilot launchers still use process-local ownership; a new
launcher cannot reclaim their old inbox. Durable conversation reconnection uses
the native Interface pane path above. Old pilot inboxes are not guessed or
automatically reassigned. Never replay an interrupted task to obtain it.

## Implemented v1 tools

The precise schemas are returned by `tools/list`, from
`crates/interface-collaboration/src/workers/tools.rs`.

| Tool | Behavior |
| --- | --- |
| `worker_models` | Exact route, grant state, queue limits and delivery mode; availability is explicitly unprobed |
| `worker_start` | Flat v1 request: requestId, kind, title, brief, criteria, readFiles, exact model and bounds |
| `worker_collect` | Task IDs/cursor, up to 30 seconds waiting; `detail` includes report and bounded original output |
| `worker_followup` | A new linked task after an exact completed report; preserves the former task and rechecks context scope |
| `worker_cancel` | Durable cancellation request; collection distinguishes request from settlement |
| `worker_review` | Receipt or accepted/rejected report, bound to its exact result hash; never writes source |

Example start:

```json
{
  "schemaVersion": 1,
  "requestId": "review-example-1",
  "kind": "review",
  "title": "Check stable ordering",
  "brief": "Review the supplied function for first-occurrence ordering. Cite the captured file and line. Do not edit or claim tests ran.",
  "acceptanceCriteria": ["One concrete, source-supported finding or noFindings"],
  "readFiles": ["src/example.rs"],
  "model": "opencode/muse-spark-1.3-contributor-free",
  "wallSeconds": 180,
  "maxSteps": 16
}
```

Kinds are research, analysis and review. Sources must be explicitly supplied;
workers cannot browse or fetch missing context. `readFiles` must be a subset of
the broker's approved list, and inherited sensitive-path/symlink exclusions
apply before reading. Snapshot inputs are rechecked, hashed and durably frozen
before dispatch. The total inline packet is limited to 32 KiB. A read-only
packet has no writable files. Worker tools are all denied, and an observed tool
attempt invalidates its run.

There are two running slots and eight queued tasks across the broker, with
admission rotated across lead queues. Default task limits are 180 seconds and
16 steps; accepted ranges are 1–600 seconds and 1–16 steps. There is no model
fallback, worker recursion, or automatic retry of ambiguous outcomes. Follow-up
while a provider turn is still running is refused in this slice, rather than
implicitly starting concurrent turns. At 128 hot records, the broker archives up
to 32 oldest acknowledged terminal results before admitting more work. Unread and
active tasks are never discarded. Exact task IDs still collect archived reports;
old start/review/cancel request IDs retain replay protection. Mutation receipts
spill to individual private files, so a full receipt index does not block Stop.
The archive persists on disk without automatic deletion. If all 128 records are
active or unacknowledged, admission still refuses until results are acknowledged.

The store uses schema version 2 at the retained `workers.v1.json` rendezvous
path. Opening a v1 store atomically upgrades it before admission. An older
broker sees the unsupported version and refuses, so it cannot ignore archived
retry receipts and duplicate effects. Tool requests remain version 1. It uses the existing
descriptor-bound writer lease, expected-snapshot check, atomic replacement and
fsync primitives. A frozen result, status, and inbox cursor commit together.
Any failed write poisons the broker: collection and acceptance report
`storageUncertain` instead of exposing an uncommitted completion. Recovery reads
only durable state and never replays an ambiguous effect. Archiving commits
complete evidence and retry receipts before removing records from the hot index;
the explicit store-version fence prevents unsafe downgrades.
Malformed reports keep bounded original output without being repaired into
valid reports. Failure/cancellation can be acknowledged using `receiptSha256`
as the review hash when no result hash exists. Only a completed valid report
can be accepted. Review acceptance remains a lead judgment, not test evidence.

## Delivery and limits

Standalone launchers configure SessionStart, UserPromptSubmit and PostToolUse
notice hooks. Native panes add notice-only UserPromptSubmit and PostToolUse hooks
alongside existing provider-session capture. Legacy delegation hooks were retired on 2026-10-14. Hooks query locally and add only broker-authored task IDs. While
no running pane is bound to a conversation with an unreceived result, a notice
answers empty before the caller and conversation identity checks, so ordinary
tool calls spawn no `ps`; an unreviewed result for one conversation leaves every
other pane on that path. After an in-pane switch such as `/resume`, a notice for
the new conversation waits for the pane's binding to refresh (the provider hook
starts that at once). Reports remain
untrusted tool data. They do not deny edits, veto Stop, type into a terminal,
or start a new idle model turn. Explicit collection and subsequent review are
the verified receipt path. Do not infer idle wakeup or hook-triggered attention
from a successful tool call.

The MCP transport negotiates tested legacy tool protocols through 2025-11-25.
It does not claim the 2026 task-extension protocol. Inputs and concurrent
requests are bounded. Caller ancestry is checked with a one-second deadline,
not the former unbounded process probe.

Remaining product work: graphical grant controls, adjacent worker views and
graceful upgrades that preserve running worker processes. Native inboxes recover
from durable records after process restart; interrupted effects remain unknown
and are not retried. Standalone pilot launchers do not reclaim previous inboxes. Code-proposal validation/application are a separate later slice;
`worker_validate` and `worker_apply` are not advertised by this build.

## Local regression evidence for the follow-up

- Save failure injected at result settlement: no completed result or acceptance
  escaped; later mutations refused with `storageUncertain`.
- 130 simulated tasks exceeded the former history ceiling. Archived evidence,
  owner isolation and identical/different-intent retry handling were verified.
- A full mutation index still allowed cancellation and preserved older receipts.
- Real macOS PTYs verified unconfirmed-ID refusal, unrelated-process refusal,
  stale-generation refusal and confirmed conversation continuity after reopening.
- The actual native pane MCP served simulated Claude and Codex processes, each
  starting, collecting and accepting one simulated OpenCode report. Both panes
  reopened and collected the original report, rejected the other inbox, and
  produced exactly two total worker dispatches. Source bytes stayed unchanged.
- No additional real-model calls were made for these changes.

## Measured minimal live test, September 13, 2026

Only a disposable three-value ordering fixture was shared. No Clean UI source,
approved design spec, or active Fable conversation was exported. Existing jobs
and production were left running.

| Lead | Native client | Result | Muse worker time | Lead session wall time |
| --- | --- | --- | --- | --- |
| `claude-opus-5` | Claude Code 2.1.270 | Start → independent arithmetic → collect → accept | 5.059 s | 18.49 s |
| `gpt-5.6-sol` | Codex 0.154.0 | Start → independent arithmetic → collect → accept | 5.424 s | 30.92 s |

Each successful lead used three worker-tool calls and one Muse run. Both
findings identified the seeded ordering violation and cited the captured line.
The harness independently inspected the fixture and preserved its bytes.
OpenCode 1.18.30 reported zero cost for both workers. Opus's CLI reported
approximately $0.1643 total, including approximately $0.0012 of incidental
Haiku startup usage. Sol reported 59,421 input tokens (33,792 cached) and 724
output tokens for its successful test; dollar cost was not reported. These are
CLI usage reports, not verified billing charges or a broad performance ranking.
The adapter records the requested Muse route; its stream does not report an
independent model identity, which remains null rather than inferred.

Two preliminary Codex attempts are retained as failures: literal `gpt-5.6` was
unsupported by the account, and the first Sol attempt had no headless MCP tool
approval. Neither started a Muse worker. The owner selected Sol, and explicit
scoped tool approval fixed the latter launch configuration. Opus was not rerun.

Detailed local evidence: `/private/tmp/iw-mcp-live`,
`/private/tmp/iw-mcp-sol`, `/private/tmp/iw-mcp-sol-final`. These temporary paths
are evidence locations, not runtime dependencies.

## Verification and code map

Run `cargo xtask check --changed` for this Rust-only slice. It includes Rust
formatting/Clippy/tests, Swift helpers and the renderer pin check; this worktree
needs its existing Electron lockfile dependencies for the xterm pin. The first
gate attempts exposed sandboxed Swift-cache access and absent node_modules;
neither was a worker behavior failure. Live tests are opt-in and are not part
of the deterministic gate.

- Domain/store/queue/review: `crates/interface-collaboration/src/workers.rs`.
- MCP schemas: `crates/interface-collaboration/src/workers/tools.rs`.
- Snapshot reuse: `crates/interface-workspace/src/delegation.rs`.
- Tool-free report adapter: `crates/engine-acp/src/opencode_worker.rs`.
- Native pane adapter and shared runner: `apps/interface-host/src/report_workers.rs`.
- CLI, native launch/config, socket identity and MCP: `apps/iface/src/workers/`.

Official integration references: [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp),
[Codex hooks](https://learn.chatgpt.com/docs/hooks),
[Claude hooks](https://code.claude.com/docs/en/hooks),
[OpenCode Contributor privacy](https://opencode.ai/docs/zen/#privacy).
