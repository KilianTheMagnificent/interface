# Agent mode audit implementation: validation

September 24, 2026. Branch `feat/agent-mode-audit-20260924`, base
`0b53abb7a78e88934dc852a8749c9962fe4400e5`. Implements the owner-approved audit
packages A–E; protocol 29, Agent roster v2 retained. Concurrent moved-folder
recovery (v27) and performance/controller work (v28) were integrated from Dev
`d65e056` without dropping their contract additions. `chrono-tz` 0.10.4 was
explicitly approved. Design and operating limits: [task controls](AGENT_TASK_CONTROLS.md).

## Deterministic acceptance

The canonical acceptance command is `cargo xtask check` (both Rust and Electron).
The final integrated gate passed: **1,957 Rust tests** (24 intentionally ignored),
**2,801 JavaScript tests** in 157 files, three Swift tests, formatting, linting, wire
fixtures, navigation checks, renderer DOM fixtures and builds. The final source
includes the checkbox alignment fix found during screenshot inspection. Exact-byte
receipts were reused by closeout. Gate log: `/private/tmp/agent-integrated-gate8.log`.

Landed **Reliable Agent tasks and calendar routines**, `d65e056..5677105`. Dev
renderer → preload → Rust-host readiness passed on `5677105`; fetched status
confirmed that verified revision is running and both source check receipts match.
Logs: `/private/tmp/agent-dev-closeout.log`, `/private/tmp/agent-dev-status.log`.
The main ref and the existing 0.3.132 production feed were not changed by this task.

Added regressions exercise:

- Exact Codex roots; missing/malformed/extra roots; cwd canonicalization; private
  inherited TMPDIR metadata; clearing persisted workspace roots on gated resume.
- Actual subprocess exit after first-start reservation, dispatch and binding;
  ambiguous intents never replay; identical definitely-unsent intents can retry.
- Folder revocation and close failures, scope changes during admission, duplicate
  starts, failed binding with buffered approvals, retained/expired approvals,
  opaque interpreter owner decisions and session generation retirement.
- Task contract validation/defaults and cross-language golden JSON; restart to
  Unknown, overlap leases, Stop before closure, failed closure, deadlines,
  action/reported-token bounds, exact artifact/input receipts and stale evidence.
- Owner-private store bounds, metadata cleanup, memory CAS/history restoration,
  fact confirmation authority, input freshness and credential/path rejection.
- Exactly eight scoped host tools, malformed tool input, request-id/hash conflicts,
  uncertain mutation non-replay, revoked endpoints, fresh resumed generations and
  a real resumed tool mutation.
- Selected-context child grants, parent cancellation, changed grants/source hashes,
  required review, compare-and-save effects and tool-free worker restrictions.
- IANA schedules, spring gaps, fall folds, missed-run policy, first-routine home,
  test-before-enable and contract changes invalidating prior tests.

The native process/socket tests need an execution environment permitting local
Unix sockets and process inspection. One sandboxed run refused those facilities
(27 host test failures and a definitely-unsent live endpoint failure); these are
retained as harness failures, not interpreted as product behavior. The suite was
rerun with the required facilities. Earlier compiler/lint/fixture failures were
fixed before the final canonical gate.

## Live provider tests

Provider executables: Claude Code **2.1.281**, Codex **0.156.1**. Synthetic files and
throwaway Interface state only; authentication remained inside the installed CLIs.
No credential files or pre-existing account transcripts were copied or inspected.
Model aliases were `sonnet` and `gpt-6-luna`; these timings describe single runs,
including host/provider startup, not benchmark distributions or billing estimates.

| Scenario | Claude | Codex |
| --- | --- | --- |
| Create declared JSON artifact | Completed; Verified; 6.67 s | Completed; Verified; 21.44 s |
| Scoped consultation, review and apply | Codex child; Verified; 19.86 s | Claude child; Verified; 32.64 s |
| Resume host tools, revoke folder, deny attempted write | Passed; 18.60 s | Passed; 31.23 s |

Consultation used one selected `numbers.json` containing 20 + 22 and an incorrect
sum of 41. Both directions produced and reviewed a proposal, applied it through
the host, and independently verified 42. Child elapsed times were 7,490 ms for
Codex and 5,543 ms for Claude; provider-reported child tokens were 7,344 and 303.
After integrating Dev under protocol 29, both providers again completed and
verified fresh JSON artifacts (Claude 7.78 s, Codex 16.46 s) and successfully wrote
a scoped task note. Evidence is in
`/private/tmp/iface-agent-live-6365009b97/results.json` and
`host-tool-note-check.json` beside it. The initial note assertion omitted the
product's `Agent request: ` prefix and reported false; a separate deterministic
check of both persisted synthetic notes confirmed their exact expected contents.

Parent and child usage metrics have different provider semantics; they are not a
normalized cost comparison. Child success did not itself verify parent success.

The resume test completed an initial task, closed the session, resumed it with a
new task and wrote a local task note through the new scoped endpoint. It then
started a delayed write, revoked the granted folder, resumed with the narrower
scope, denied the outside write request, and checked that the file did not exist.

Live testing found and fixed these real integration defects:

1. Codex resume echoed stale runtime workspace roots and its inherited private
   TMPDIR. Resume now clears persisted roots with the versioned experimental field
   and verifies the exact permitted echo; it never tolerates arbitrary extras.
2. Codex's native MCP approval mode rejected mutating host tools before they
   reached the backend. Only the private, task-bound server is explicitly enabled.
3. A resumed host-tool mutation used the endpoint scope ID as the task ID. It now
   resolves the active task; old endpoint generations stay revoked.
4. Claude's built-in plugins survived safe mode in a tool-free child. The two
   built-ins are explicitly disabled; unexpected tools still invalidate a result.
5. The former shell policy auto-approved a Python command which wrote into the
   revoked synthetic folder. The stricter default now requires an owner decision
   for opaque commands. The rerun denied it and observed no file. Broad shell
   behavior remains an explicit owner opt-in and is not an OS security boundary.
6. Stop after completed work incorrectly downgraded a valid receipt. Completed
   results retain their evidence; interrupted work cannot retain Verified status.

A direct-versus-consult comparison is intentionally tiny. An initial direct Codex
prompt prohibited all commands (including its file-read path) while the copied
expected-output text still requested a child proposal. It left the file unchanged,
correctly showing Failed verification. That contradictory fixture is excluded
from comparative claims; the corrected fixture allows a read command, requires
no child, and uses a direct output contract. No failed run is counted as success. The corrected Claude direct run passed in
8.18 s versus 19.86 s with consultation. Corrected Codex read the file, then
requested a Python edit despite the no-scripts instruction; the new policy
correctly left it owner-pending at 12.62 s. The harness stopped that run. It is
not a completed latency sample and does not prove a quality advantage for
consultation. This also confirms that model instructions alone are not authority.

Local evidence files from this implementation session (temporary, not committed
account history):

- `/private/tmp/iface-agent-live-83500cf3d6/results.json` — declared artifacts.
- `/private/tmp/iface-agent-live-f34fcb4e12/consultation-results.json` — Claude parent.
- `/private/tmp/iface-agent-live-590800babe/consultation-results.json` — Codex parent.
- `/private/tmp/iface-agent-live-8137314306/results.json` — Claude resume/revocation.
- `/private/tmp/iface-agent-live-65bbd1742f/results.json` — final Codex resume/revocation.
- `/private/tmp/agent-mode-direct-smoke3.log` — corrected direct comparison.

## Hidden desktop checks

The real unpackaged app runs through `npm --prefix electron run handsfree`, with
fake providers, a throwaway profile and a hidden window. Observed controls:
first paused routine prepares Agent home before any thread; calendar timezone and
occurrence preview; saving pauses the schedule; enabling refuses an untested
contract; artifact contract save; memory revision restore; owner confirmation of a
proposed fact. The driver checks that the test app never becomes frontmost.
Screenshots were inspected. The final hidden run also verified scripts and
consultations default off, draft changes clear preview, preview does not apply the
schedule, saving applies the selected time, and untested enablement stays Off with
a visible error. The vendored driver and hook match the shared kit.

These checks do not prove native notification delivery, composited vibrancy,
paired-phone states or live-provider behavior inside the hidden harness. Live
providers were tested separately through the real host protocol above.

## Deliberate limits

No 180-attempt (30 tasks × 2 providers × 3 trials) paid model-quality evaluation
was performed; consultation remains opt-in and no general quality gain is claimed.
The stronger Claude OS sandbox was an audit exploration, not an implemented or
verified security guarantee. Network access and some provider-native actions may
not generate approvals. File hashes/JSON checks do not prove semantic correctness.
No new providers, unrestricted group delegation, browser/hosting or phone-approval
system was added. Calendar routines need the Mac and app awake/running. Do not
run an older host against a profile using the new sidecars: it cannot enforce the
new task/calendar rules. Production promotion, packaging, signing, updater feed,
installation and production restart are separate and were not requested.
