# Agent task controls — protocol 29

The owner-approved September 2026 audit packages add Rust-owned task controls
without changing the persisted Agent roster v2 shape or its settled appearance.
The canonical test gate remains `cargo xtask check`. The timezone dependency is
`chrono-tz = 0.10.4`, explicitly approved by the owner.

## Admission and recovery

`agent-starts.v1.json` reserves each first-message request before provider dispatch.
The request ID is bound to the agent and input hash. Bound starts return their
original session; ambiguous starts are never reissued. Only definitely-unsent
failures permit the identical intent to retry. At 4096 retained identities a
new start replaces the oldest bound or definitely-unsent intent (the journal
stores no age, so intents from an earlier run go first, unsent before bound);
the same happens at its 4 MiB cap. It never discards an unresolved effect, and
refuses new starts only when every retained intent is unresolved. A real
subprocess-exit test covers reservation, synthetic dispatch and binding boundaries.

Admission locks are per agent. Folder changes close old provider processes before
committing grants. Close failures retain the previous scope. Exact canonical
Codex roots, including a valid empty array, must match the requested extras.
Codex resume explicitly clears persisted runtime workspace roots. A CLI may echo
the exact inherited private TMPDIR only when its metadata explicitly permits it;
this exception never accepts `/`, shared `/tmp`, ancestors or missing metadata.
Late approval events cannot revive closed sessions. Only explicit resume starts
a new approval generation. Unresolved owner approvals are retained durably (4096
maximum); reaching capacity stops the affected provider instead of dropping a
live request. Session closure/restart expires old decisions without replay.
The UI notifies for owner-pending agent decisions and preserves ordinary session
notifications while agent starts are binding.

## Task lifecycle and evidence

`agent-work.v1.json` is an owner-private, leased, atomic sidecar (32 MiB maximum).
It contains task settings, up to 500 task records, tool intents, memory history,
consultation receipts and calendar metadata. Corruption or uncertain writes close
admission. Restart converts active tasks to Unknown and keeps their folder leases.
The owner must inspect and acknowledge them; acknowledgement closes any surviving
provider before releasing its lease. Stop persists before process closure.
A failed task write retires only that task's session. Session events seen while a
task is still starting are buffered (4096 maximum); overflow makes the starting
task Unknown and stop-requested when it binds, never failing another session.
Events that change nothing (a repeated turn start, an unchanged token count) are
not written.

Every new turn snapshots expected output, input freshness, artifact checks,
canonical roots and budgets. Overlapping writable roots, including the agent home,
are serialized. Routine and interactive work share the same leases. Independent
selected-context consultations do not gain a second filesystem writer.

Execution and verification are separate. Idle means the provider turn ended.
After completion, the host checks declared files for freshness, nonempty content,
UTF-8 or valid JSON, optional required text, and unchanged input hashes. A receipt
binds exact canonical paths, bytes and SHA-256 hashes. Later changed bytes mark it
Stale; polling never upgrades it. No declared checks means Unverified. These checks
do not establish semantic correctness, research accuracy or overall model quality.
Files are bounded, credential-filtered and must remain in scope. Outputs and child
reports are delivered locally in Runs / Task results, with links to conversations.

Defaults: 30-minute elapsed deadline, 10-minute no-progress timeout and 200 observed
actions. Wall and no-progress timers also use monotonic process clocks. The host
checks budgets every second while a task runs; with none running it sleeps until
the next admission (or a minute). Reported
parent/child tokens can be capped; missing usage and cost are not inferred.
Provider-native actions may complete before observation and interruption. These
are execution controls, not an OS sandbox or a precise billing limit. A successful
tool result with a different content hash counts as progress; the absolute and
action bounds remain in force even when progress signals are weak.

Opaque scripts, package runners and commands whose effects the classifier cannot
establish require an owner decision by default. Task settings offer an explicit
opt-in to the former broader command policy. Even the stricter policy is an
approval classifier, not containment: provider-native operations can bypass approval,
network access remains enabled and executable identity is not cryptographically
attested. Unknown shell flags/effects should be reviewed; no universal prevention
of exfiltration or out-of-scope writes is claimed.

## Narrow host tools and consultation

Only `iface_agent` is attached to gated sessions. It is an owner-private Unix
endpoint bound by the host to one active task, not an agent ID supplied by a model.
Failure to bind it refuses launch. A retained connection can follow subsequent
turns of that same live session only with identical roots; Stop, recovery and
scope changes close admission. An explicit resume gets a fresh endpoint generation
which stays inactive until new work is admitted; older connections remain revoked.
External MCP names and annotations never grant
automatic permission. Claude's bounded native `ToolSearch` discovers definitions;
it does not authorize invoking them. First-party tools advertise the documented
`anthropic/alwaysLoad` metadata to avoid unnecessary discovery on supported CLIs.

- `task_status`: goal, contract, grants, limits, current evidence, bounded pending
  approvals and child result/review metadata; recovery never replays effects.
- `artifact_register`: records exact bytes; cannot verify success by itself.
- `memory_propose`: compare-and-save with private revision history; never owner authorization.
- `notification_request`: owner-directed note in local Task results; no third-party delivery.
- `routine_draft`: paused draft only; cannot enable it.
- `consult_start`: tool-free, selected-context Claude/Codex child.
- `consult_review`: accepted/rejected review bound to the exact child result hash.
- `consult_apply`: compare-and-save one reviewed existing file; cannot delete or create files.

Every mutation has a durable request ID and input hash. Interrupted outcomes are
non-replayable. The owner explicitly configures the child provider/model, selected
files, writable subset and maximum children (1–4). Default is no consultation.
The packet is at most 32 KiB of selected, hashed, non-sensitive text. There is no
provider fallback, child tool access, recursive delegation or authority expansion.
The child uses the existing content-only adapters: strict read-only Codex exec
configuration or restricted tool-free Claude print mode. Unexpected tools invalidate
the response. Claude built-in instruction/telemetry plugins are explicitly disabled
in addition to safe mode, empty MCP and tool restrictions. Codex enables only this
private host tool server through its native approval configuration; the backend
still checks every task, generation, grant and effect. Deadline is at most five minutes and the parent's remaining time;
parent cancellation propagates. Child reported usage and elapsed time are retained.

The report carries exact input evidence and optional replacement proposals. Parent
review is mandatory, and applying rechecks all source hashes plus the target's CAS
revision. Each applied file has its own receipt; this is not a multi-file atomic
transaction. A child result never completes or verifies the parent. No broad
quality/latency advantage is assumed from this wiring; measure it on relevant tasks.

## Memory

Local pre/post-turn observations and owner/tool saves retain 20 revisions per agent,
up to 256 KiB per revision. Restoring requires idle work and a current hash. Facts
carry a source, observed time, optional expiry/supersession and proposed versus
owner-confirmed state. Models cannot confirm facts or rewrite owner authorization.
Task status is the recovery context; memory is separate. Memory history is not
automatically inserted into consultation packets or task prompts.

## Calendar routines

A first routine can select Agent home before the first conversation. Rust prepares
that home on save. Existing elapsed intervals remain supported. Calendar schedules
use an IANA zone, local time and ISO weekdays; the UI previews future occurrences
without applying them. Editing the draft clears its stale preview.
Spring gaps are skipped; fall repeats run at the first instant only. Missed runs
are either skipped after a 60-second lateness allowance or coalesced to one run.
Saving a calendar pauses it. Enabling requires a successful real test with the same
profile, prompt, folder and task contract; declared artifact checks must pass.
Changing that contract invalidates the test. Failure can pause the schedule. Fresh
inputs are checked again at every admission; old or missing inputs fail explicitly.
The Mac and Interface must be awake/running. Hosted execution, phone approvals and
unrestricted collaboration remain separate projects.

## Evidence and remaining measurements

See `docs/AGENT_MODE_VALIDATION.md` for exact checks and measured results. Unit and
synthetic boundary tests establish lifecycle behavior, not production model quality.
The audit's suggested 30 tasks × two providers × three trials remains a separate
180-attempt model-quality evaluation; no such success rate is claimed here.

Primary protocol references checked during implementation:
[Claude tool discovery](https://code.claude.com/docs/en/mcp),
[Claude sandbox controls](https://code.claude.com/docs/en/sandboxing),
[Codex app server](https://learn.chatgpt.com/docs/app-server),
[MCP tool contracts](https://modelcontextprotocol.io/specification/2026-07-28/server/tools),
[chrono-tz](https://docs.rs/chrono-tz/0.10.4/chrono_tz/).
