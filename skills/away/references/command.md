# Away command

`iface-away` is the shared Rust command. Native entries are `$away` in Codex and
`/away` in Claude Code; both load this workflow and invoke the same controller.

```text
$away Improve interval parsing for 30 minutes
/away Improve interval parsing for 30 minutes
```

From a prepared isolated project, write a JSON config at an absolute path:

```json
{
  "version": 1,
  "project": "/absolute/isolated/project",
  "objective": "Repair interval merging; cover empty, overlapping and reversed intervals without mutating input.",
  "files": ["intervals.py"],
  "contextFiles": ["README.md", "test_intervals.py"],
  "validation": {
    "version": 1,
    "files": ["intervals.py", "test_intervals.py"],
    "protectedFiles": ["test_intervals.py"],
    "checks": [["/usr/bin/python3", "-B", "test_intervals.py"]]
  },
  "workerModel": "codex/EXPLICIT_SUPPORTED_MODEL",
  "reviewModel": "claude/EXPLICIT_SUPPORTED_MODEL",
  "durationSeconds": 1800,
  "maxIterations": 12
}
```

Replace model placeholders with installed, explicitly selected model IDs. Both
roles may use `codex/<model>` or `claude/<model>` independently. Authentication
stays with each installed CLI; no keys or account configuration are copied.

```sh
iface-away start --config /absolute/config.json --directory /absolute/new-run
iface-away status --directory /absolute/new-run
iface-away feedback --directory /absolute/new-run --id review-1 --message 'Include touching intervals in the regression.'
iface-away stop --directory /absolute/new-run
iface-away recover --directory /absolute/new-run
```

Run directories must be new and outside the project. Config and scope are fixed
at creation. Feedback ids are idempotent for identical text; reused ids with
different text fail. `recover` requires the controller writer to have exited,
marks interruption unknown, and contacts no provider. There is no resume command.

Validation distinguishes `inputs` (original source guards for safe application)
from `validatedInputs` (hashes of the tested candidate bytes). Protected acceptance
files must also be supplied in `contextFiles` so both worker and reviewer can
inspect them.

Read `cycle-run.v1.json` for attempts, complete proposal hashes, validation
results, review identity and application receipts. `cycle-control.v1.json` shows
received feedback and Stop. The status response reports `admissionOpen`; this is
permission to admit work, not proof the process is alive. A completed proposal
acknowledges the exact feedback revision and ids in its structured summary.

The loop runs proposal → protected checks → fresh review → compare-and-save,
then refines from the latest check failure or actionable review findings. Findings targeting protected validation or missing scope park the run; the
candidate worker cannot repair those by changing unrelated code. Review
is mandatory, tied to candidate bytes, the complete validation receipt, check plan and feedback
revision. Passing tests alone cannot accept a candidate. New feedback before
application invalidates an older review. Repeated candidates stop the cycle.

The deadline covers provider and check waits; Stop polls within 100 ms between
short filesystem operations. Providers and check process groups are terminated
when their futures are cancelled. An already admitted compare-and-save finishes
its short checkpoint and retains its receipt. Stopping does not undo accepted
work. If the computer sleeps, no work proceeds while asleep; the absolute
deadline closes admission after wake. Keep the computer awake if useful work
must continue throughout an absence.

The controller handles 1–20 iterations, 1–86400 seconds, at most 48 worker/context
files, bounded text packets, and fixed argv checks in a disposable explicit-file
copy. It cannot run a whole repository build that needs undeclared files or
dependencies. Choose a coherent tested unit; the invoking agent performs the
repository's integration gate afterward. This is a selected-file cycle, not an
unrestricted terminal agent. It does not perform web research during a run.

## Installation

In the Interface-Rust checkout containing the implementation:

```sh
cargo build --locked -p iface
scripts/install-away-command.sh target/debug/iface
```

The installer copies the binary to `~/.local/lib/interface-away/iface`, installs
the launcher at `~/.local/bin/iface-away`, and installs the same skill under
`~/.codex/skills/away` and `~/.claude/skills/away`. Existing differing installations
are refused unless an explicit update is requested with `--replace`. Refresh
the skills list or start a fresh native session if discovery has not refreshed.

The supported native mechanisms are documented in [Codex skills](https://developers.openai.com/codex/skills)
and [Claude Code skills](https://code.claude.com/docs/en/skills). CLI invocation
uses the repository's existing content-only adapters; see [Claude programmatic
usage](https://code.claude.com/docs/en/headless). No native scheduled Goal is
required for either client.
