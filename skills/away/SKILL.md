---
name: away
description: Start, inspect, give feedback to, or stop a bounded unattended research, implementation, test and review cycle on a local project using Codex or Claude Code.
---

Use the installed `iface-away` command. Invoke this skill as `/away` in Claude
Code or `$away` in Codex. Interpret the user's trailing text as the objective,
duration, selected provider/model, or a status/feedback/stop request.

Read [the command reference](references/command.md) when starting a run or when
the syntax is unfamiliar. Run `iface-away --help` before relying on an installed
version. If it is absent, locate Interface-Rust's `scripts/install-away-command.sh`
and follow the reference's installation instructions; do not substitute a native
Goal, `/loop`, queued chat message, or prompt-only timer.

For status, feedback or Stop, use the exact run directory from this conversation
or its retained receipt. Do not infer a run from the newest unrelated session.
`feedback` writes a durable receipt immediately, but acknowledgment occurs only
when a completed proposal echoes its revision and ids. Late feedback never
extends the deadline or restarts a run. Stop is `iface-away stop`, followed by
status verification; do not just queue “stop” to a model.

When starting:

1. Ground the project, its instructions and the user's authorized objective.
   Choose a small useful file scope and realistic acceptance tests. Use existing
   tests where suitable. Add a regression first when correcting a demonstrated
   defect. Declare acceptance files as protected. Do not weaken them to get a
   green cycle. The worker receives file content, not tools or web browsing;
   supply needed research and context before launch.
2. Preserve concurrent work. Prefer an isolated task worktree for a Git project;
   read its own applicable instructions. Never switch, stash or discard the
   owner's checkout. Explicitly select the intended base. If needed uncommitted
   changes are part of the task, copy only their inspected, authorized diff into
   the isolated worktree. Otherwise explain which committed base is used.
3. Create a bounded v1 configuration outside the project using the reference
   schema. Resolve supported, authenticated models from installed CLI metadata
   or current session context; preserve explicit choices and never guess a new
   model id. Default to the invoking provider for both roles unless the user
   selects cross-provider review. Every review is a fresh isolated turn even
   when the provider/model is the same. Default to 3600 seconds and 12 iterations
   only when the user gave no limit. Honor shorter user limits.
4. Run the chosen acceptance command on the baseline. Confirm it exercises the
   intended behavior and works from a disposable copy of the declared files.
   This command is trusted local code, not an OS sandbox. Avoid commands that
   install dependencies, publish, contact production or require secret files
   unless separately authorized. Include all required fixtures/configuration as
   explicit validation inputs. Report unsupported toolchain/context needs.
5. Start `iface-away start --config CONFIG --directory NEW_RUN`. A dedicated
   terminal or an existing process supervisor may keep it alive when the UI is
   closed; capture its stdout/stderr in owner-private files and retain its PID.
   The Rust command owns time and acceptance. Do not start another supervising
   agent loop around it or automatically resume it after expiry or failure.
6. Verify the run record exists and report the run directory, objective, provider
   choices, deadline and Stop command. During supervision, send findings through
   `iface-away feedback` with a stable id, then inspect receipt/acknowledgment.
   Do not label feedback “reviewed” merely because it was received.

After completion, inspect the retained proposal, checks, review, application
receipts and actual source diff. Run project integration gates appropriate to
accepted changes. Follow the project's existing authorized closeout procedure;
the cycle itself does not commit, merge, publish or restart applications. Report
accepted work, rejected/blocked attempts, in-flight uncertainty, and running Dev
separately. An expired or stopped run requires a new explicit user request before
starting another run with a new directory and deadline.
