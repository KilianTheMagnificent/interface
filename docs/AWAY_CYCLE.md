# Shared Away cycle for Codex and Claude Code

The one-hour Goal experiment demonstrated useful autonomous fixes but also a
control failure: a delayed review message arrived after the admission deadline
and reopened work. The new `iface away cycle` controller makes deadline, Stop,
feedback receipt and review acceptance Rust-owned state. Both native clients use
the same command through the `away` skill.

Install and usage: [shared command reference](../skills/away/references/command.md).

```text
Codex:       $away Improve interval parsing for 30 minutes
Claude Code: /away Improve interval parsing for 30 minutes
Terminal:    iface-away start --config /absolute/config.json --directory /absolute/new-run
```

The invoking client grounds research, file scope and acceptance tests, then the
controller runs content-only proposal → protected checks → fresh review →
compare-and-save. Both roles can select either installed provider independently.
No dependencies, authentication stores, desktop protocol or existing persisted
schemas change. The installer places the executable and skills in user scope.

## Feedback and review behavior

- Feedback writes an idempotent, immediate receipt to a separate control file.
  The worker must echo the exact revision, feedback ids and unresolved finding
  ids in its structured summary. Receipt and acknowledgment are distinct.
- Findings distinguish candidate defects from validation/scope problems. The
  latter park the run rather than spending source-edit retries on a problem the
  worker cannot fix.
- The latest failed check and unresolved actionable review findings survive
  refinement attempts; compact older history cannot displace them.
- A fresh reviewer receives source, protected acceptance tests, candidate and
  host-executed check output. It must return a structured approval or actionable
  findings with concrete evidence. A passing check is not itself approval.
- Validation names original-source compare-and-save guards separately from
  actual tested-artifact hashes; the reviewer can verify which bytes passed.
- Review binds candidate changes, the complete validation receipt, fixed check plan and
  feedback revision. New feedback before application invalidates an older review.
  Source drift is checked again before compare-and-save.
- Full provider-envelope hashes remain integrity evidence; no-progress detection
  compares candidate changes within the same feedback revision, excluding
  elapsed time, usage and session metadata.

## Stop, time and recovery

The immutable absolute deadline is paired with a monotonic timer while running.
The controller checks both around dispatch and application. Stop and feedback
share an admission lock with application; an owner Stop or feedback waits up to
3 s for that checkpoint to release it (retrying only the lock, never an effect)
and otherwise reports that it was not recorded. During provider/check waits, Stop is
polled every 100 ms and deadline expiry drops the active future, terminating its
process group. An already admitted compare-and-save completes its checkpoint.

Late feedback is retained for inspection and cannot reopen a terminal run.
There is no resume verb. Recovery requires the writer lease, marks interruption
unknown and contacts no provider. Status distinguishes closed admission from
uncertain in-flight effects; unknown outcomes retain exit code 4.

## Scope and limits

This is a bounded selected-file cycle, not an unrestricted repository agent or
desktop Away switch. The invoking client supplies research and realistic checks;
workers have no filesystem, shell, web or delegation tools. Checks run trusted
parent-selected argv in a disposable file copy, not an OS security sandbox.
The command does not commit, merge or release. The invoking agent follows the
project's authorized integration and closeout requirements after checking results.

Run limits are 1–20 iterations and 1–86400 seconds. Input and output packets are
bounded; large repository builds needing undeclared files/dependencies should
be run as integration gates afterward. Logs and receipts are owner-private.
The existing pilot's paused baseline-Stop fix is also included in this change.

## Validation

Deterministic regressions cover feedback during review, stale review identities,
missing acknowledgments, actionable findings on the next attempt, real failed
check output, deadline expiry, late feedback after expiry, duplicate message ids,
recovery, and Stop against non-cooperative proposal/check/review futures. A real
child-process test verifies that cancellation prevents a spawned descendant from
continuing. Provider and installed-command trial results are recorded after the
implementation checks below.

- Full `cargo xtask check --changed` passed both Rust and Electron lanes. The
  installer script correctly required both suites. Rust reported 1,538 passing
  tests and 18 explicitly ignored opt-in tests; deterministic renderer, Swift,
  packaging-script and smoke-pin checks passed. No packaged app or production
  release was requested.
- Fresh disposable trials exercised Codex → Claude review and Claude → Codex
  review. Each accepted one repair, then returned no further change. Each final
  file independently passed 935 behavior assertions; actual file hashes matched
  the tested-artifact receipts and review identities.
- The initial trial exposed ambiguous validation hash labels and a test oracle
  that duplicated the candidate expression. Approval was withheld. The evidence
  now separates source guards from tested artifacts; fresh fixtures use explicit
  expected values and invariant checks. Validation/scope findings now park work.
- Requested models were `codex/gpt-5.6-terra` and `claude/sonnet`. Claude reported
  `claude-sonnet-5`; Codex did not report an independent actual-model identity.
- Installer behavior was tested in an isolated prefix, including refusal to
  overwrite an existing installation. A fresh Codex `debug prompt-input` listed
  `away` through its resolved skill-root alias.
- A fresh Claude Code `/away` invocation listed the skill, executed the installed
  `iface-away --help` and exact completed-run `status` commands, and returned
  successfully. This verified native discovery and command execution, not just
  the presence of files in a skills folder.

Live evidence is retained under `/private/tmp/away-command-live-20260912`, in
`codex-run-v3` and `claude-run-v3`. The earlier failed/preflight trials remain
separate; they were not overwritten or resumed. The code gate log is
`/private/tmp/away-command-gate-20260912.log`.
