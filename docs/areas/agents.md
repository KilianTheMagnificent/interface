# Agent teammates and workspace

An agent is a Rust-owned profile with a home folder, one continuous thread, a
memory file it keeps itself, routines and run history. It works freely and asks
the owner before risky acts. Capability comes from the provider harnesses
Interface already runs; Interface adds persistence, the approval gate and policy,
and the surfaces. Design: [agent teammates core](../superpowers/specs/2026-09-15-agent-teammates-core-design.md)
(sub-project 1 of 7). Plan: [implementation plan](../superpowers/plans/2026-09-15-agent-teammates-core.md).
This page describes what was built, including where it differs from the design.

A teammate's one-off task ("New task" with the agent selected) runs behind the
same gate as its routines since protocol 38; see [One-off tasks](#one-off-tasks).

## Ownership and protocol

- `interface-contracts/src/agents.rs`: strict camelCase commands and DTOs,
  `ApprovalFacts`, decisions, statuses and pending approvals.
- `interface-settings/src/agents.rs`: the one leased writer for `agents.v2.json`,
  v1 migration, thread state, statuses and session bindings.
- `interface-settings/src/agent_home.rs`: home folders, the marker, generated
  instruction files, `MEMORY.md`, and the grant rule `is_grantable_root`.
- `interface-settings/src/agent_decisions.rs`: the per-agent decision log.
- `interface-desktop-services/src/agent_control.rs`: `iface:agentControl`
  commands and the host-facing API (prepare and bind threads, bindings, pending
  approvals, read-only checks, folder validation).
- `interface-desktop-services/src/agent_policy/` and `agent_shell.rs`: the
  pure approval policy and its quote-aware shell tokenizer. `mod.rs` holds the
  entry points (`classify`, `classify_task`) and reason codes; `paths.rs` path
  resolution, credential names and scope; `shell.rs` the shell walk and
  wrappers; `targets.rs` per-program target extraction; `risk.rs` the program
  risk tables; `tests.rs` the verdict tables.
- Engine crates: `EngineAdapter::approval_facts` and the approval gate
  ([engine adapters](engine-adapters.md)).
- `apps/interface-host/src/agents.rs`: `AgentHost` (thread starts and the
  approval driver), `AgentRuntimeGuard`, `AgentEventTap` and `AgentRunner`
  ([host](interface-host.md)).
- Renderer: `AgentMenu.tsx` (roster status marks, thread previews) and
  `AgentWorkspacePanel.tsx` (Thread, Memory, Routines, Runs, Computer). App.tsx
  is the only bridge caller.

Protocol 29 (retained roster v2). `iface:agentControl` commands: `read`, `importLegacy`, `saveAgent`,
`removeAgent`, `assign`, `saveRoutine`, `removeRoutine`, `runNow`,
`acknowledgeRun`, `startAgentThread`, `newThread`, `markRead`, `readAgentFiles`,
`saveAgentMemory`, `overwriteAgentInstructionsFile`, `saveAgentFolders`,
`readAgentDecisions` (limit at most 200), `recreateAgentHome` and
`revealAgentHome`, plus nested `work` and `calendar` commands, and (protocol 38)
`startAgentTask`. See [task controls](../AGENT_TASK_CONTROLS.md) for their lifecycle and authority. Every answer is an `AgentControlResponse`: the workspace plus
`statuses`, `pendingApprovals` and, per command, `files`, `decisions` or
`sessionKey`, `work`, or `calendar`. The fixtures `electron/src/fixtures/agent-workspace-v23.json` (v2)
and `agent-workspace-v14.json` (the v1 import path) are checked by Rust and
TypeScript.

Differences from the design: there is no `SendToAgent` or `AnswerAgentApproval`
command. The first message goes through `startAgentThread`. Later messages use
the ordinary session send, and owner verdicts use the ordinary approval respond.
Both pass through `AgentRuntimeGuard`.

## Store v2

- `agents.v2.json` sits beside `agents.v1.json` in the private data root. On
  first open with no v2 file, Rust migrates v1 once. The v1 file is never
  written, so an older build keeps reading it. The v2 file must say version 2,
  the v1 file version 1.
- The v1 rules still hold: one leased writer, revision CAS, atomic snapshots.
  Corruption, drift or a held lease never reset the file.
- Bounds: 50 profiles, 200 routines, 500 runs, 500 task assignments, 16 MiB.
  Terminal runs are evicted first. `instructions` stays at 8,000 characters.
- Permission modes are not a profile field; see [Permission modes](#permission-modes).
- Profile fields added: `home`, `folders` (at most 16 absolute directories) and
  `instructionsFileSha256`. All three are Rust-owned: `saveAgent` keeps the
  stored values and `importLegacy` clears them.
- `threads` is keyed by agent id. Each entry has `current` (`sessionKey`,
  `startedAtMs`, `status` idle/working/waiting, `interrupted`), `past` (the 20
  newest, with `endedAtMs`), `lastReadAtMs` and `lastActivityAtMs`.
- If the store opens with a current thread that is not idle, Rust marks it
  `interrupted` and idle.
- Thread status is written on status changes, never per text delta.
  `lastActivityAtMs` moves on turn completion and approval events. A failed,
  errored or closed turn counts only when the thread was working or waiting, so
  closing an idle thread does not mark it unread.

## Home folders

- Root: `~/Interface Agents` only on the production data root
  (`~/Library/Application Support/Interface`) without
  `INTERFACE_DEV_SUPERVISED=1`. The Dev profile (`Iface Dev`, which reaches
  the debug host through `INTERFACE_HOST_TEST_DATA_ROOT`) and test and smoke
  roots use `~/Interface Dev Agents`.
- The folder name comes from the agent's name: no `/`, `:`, control characters
  or leading dots, whitespace collapsed, at most 64 characters. ` 2`, ` 3` and so
  on are appended when a folder without this agent's marker exists. Renaming
  never moves the folder.
- Rust creates the folder on the first message, **Show in Finder** or
  **Recreate folder**, or the first routine save. Migration creates no folders.
- `.interface-agent.json` holds `{ agentId, createdAtMs }`. Interface uses a
  folder only when the marker names this agent. A missing folder or a foreign
  marker is refused with a recreate prompt, never adopted.
- `AGENTS.md` (read by Codex) and `CLAUDE.md` (read by Claude Code and Grok
  Build) hold the same bytes, generated from the job text, the home path and
  the granted folders. Interface rewrites them only while they still hash to
  `instructionsFileSha256`. Otherwise the profile still saves and
  `files.instructionsConflict` carries the on-disk text;
  `overwriteAgentInstructionsFile` with that text's hash replaces it.
- `MEMORY.md` is created once from a template and then belongs to the agent.
  `saveAgentMemory` is compare-and-save by SHA-256 and limited to 256 KiB. A
  larger file is read-only in the page.
- Everything else in the folder belongs to the agent. Deleting an agent never
  deletes its folder.
- File access never follows links: reads use `O_NOFOLLOW`, and writes go to a
  fresh exclusive temp file that is then renamed.
- `revealAgentHome` runs `/usr/bin/open <home>` argv-only, after the agent store
  lock is released so Finder never holds up another session's event tap.

Accepted limit: when a routine and the thread write `MEMORY.md` at the same
moment, the last write wins.

## Threads

- **Start.** The host serves `startAgentThread { agentId, text, requestId }`.
  Rust prepares the home, creates a gated session (`kind: agent`, cwd the home,
  `additionalDirectories` the granted folders) with the text as its first prompt,
  and binds it. The job is not prefixed, because the instruction files carry it.
  Refusals before the provider are replay-safe. A failed bind after the create
  is outcome-unknown. A durable start journal binds `requestId` to the agent and
  message fingerprint before dispatch; only definitely-unsent attempts are retryable.
- **Continue.** Later messages use the ordinary send. A detached thread resumes
  first, and every resume the dispatcher performs for a bound thread is forced
  gated with the thread's folders.
- **New thread and provider change.** Both move the current thread to `past`,
  close its session and clear its pending approvals. The next message starts a
  new thread.
- **Past threads are read-only.** The page shows them without a composer. The
  host refuses resume, send, sendLive and model, effort, fast-mode and
  permission changes for a session whose registry cwd is inside the agents root
  unless it is a current thread or an unsettled run's session, and for any
  recorded past thread. The refusal is definitely-unsent: "Past conversations
  are read-only. Start a new conversation to continue." (A teammate's thread is
  called a conversation in every message and label since review C2; stored
  names such as `threads`, `newThread` and `startAgentThread` are unchanged.) Interrupt, close and history reads
  stay available.
- **Interrupted.** A host restart during a turn leaves the thread interrupted,
  which needs attention. `markRead`, sent while the owner can see the thread,
  clears it. Nothing is replayed.
- **Remove agent.** Confirms closure of the current thread and live routine sessions, then removes the profile,
  routines and pending approvals. Provider transcripts stay.

## Statuses

Rust projects one status per agent, in this order:

- `needsAttention`: a pending owner approval, an interrupted thread, or a run
  that is Waiting or Unknown.
- `working`: the thread is working or waiting, or a run is Starting or Running.
- `unread`: `lastActivityAtMs > lastReadAtMs`.
- `idle`.

The roster row carries the status mark. The attention tray is unchanged; a
pending approval makes the agent a live need.

## Permission modes

Each agent has a permission mode, set under Advanced in its form (protocol 32):

| Mode | Claude | Codex | Who answers what the provider asks |
| --- | --- | --- | --- |
| Ask (`gated`, default) | the approval gate below | the approval gate below | Interface's policy, then the owner |
| Auto | `--permission-mode auto`, no gate hook | `on-request`, reviewer `auto_review`, the gate's sandbox, MCP and feature switches | the owner |
| Bypass | `bypassPermissions`, no gate hook | `never`, `danger-full-access`, the owner's MCP servers and features left on | the owner |

- The mode lives in `agent-permissions.v1.json` beside `agents.v2.json`
  (`interface-settings/src/agent_permissions.rs`). Only `auto` and `bypass`
  entries are stored, and an absent agent is gated. The file is opened with the
  other agent sidecars, refuses corrupt or foreign versions without resetting
  them, drops a removed agent's entry, and ignores stale ids until the next
  save. Older builds never read it, so a rollback runs every agent gated.
- `savePermissionMode { agentId, mode }` changes it. Every
  `AgentControlResponse` carries `permissionModes` for the agents that do not ask.
- Thread starts, routine runs and resumes read the mode at launch. Gated
  sends `approvalGate`; Auto and Bypass send `agentAccess`.
  `ResumeRequirement::Access` applies the current mode to every resume,
  including implicit ones, whatever the caller asked. A live session keeps the
  mode it launched with and refuses to change it.
- In Auto and Bypass the host's approval driver asks the owner for anything the
  provider still escalates, with reason `providerAuto` or `bypass`, and never
  calls `agent_policy`. Verified task tools stay auto-allowed.
- Claude's auto mode needs Opus or Sonnet 4.6 or newer, or Fable. Otherwise
  Claude starts in its default mode, and the thread shows a notice when the
  `system/init` message reports another mode.
- ACP agents (Grok, Qwen, custom) refuse `agentAccess` just as they refuse the
  gate.

## Approval gate by provider

The risky-action policy classifies the approval events the provider emits. Scripts,
provider-native features and known-safe commands can act without those events;
this policy is not a complete security boundary. Unsupported gated providers are
refused. The launch contract is `approvalGate` and
`additionalDirectories` on `LaunchOptions` and `ResumeOptions`.

| Provider | Gate | Folders |
| --- | --- | --- |
| Claude | Permission mode `default`. The flag-layer `--settings` JSON gains a `PreToolUse` hook (matcher `*`) that prints `permissionDecision: "ask"`, plus `"disableAllHooks": false`, which outranks project and local settings the agent can write. `--permission-prompt-tool stdio` carries every request to Interface. A gated session refuses any permission-mode change other than `default`. A gated Chat is refused. | `--add-dir` per folder. |
| Codex | `approvalPolicy: untrusted`, `approvalsReviewer: user`, sandbox `workspace-write` with `network_access`, `writable_roots` = the granted folders only, and `exclude_slash_tmp: true`. Codex keeps the thread cwd writable and echoes it as `cwd`, not in `writableRoots`. The `thread/start` and `thread/resume` responses must echo the policy, reviewer, sandbox type, network access, `excludeSlashTmp`, a resolved `cwd` equal to the home and exactly the requested folders in `writableRoots` (no broader roots or malformed entries), or the launch fails outcome-unknown before the prompt is sent. A gated session refuses approval-policy changes. The owner's MCP servers are switched off in the thread config (listed through `config/read`, then `enabled = false`), and the gated app-server is spawned with `-c features.X=false` for the default-on acting features Codex runs without an approval (`apps`, `browser_use`, `computer_use`, `in_app_browser`, `multi_agent`, `unified_exec`, `unified_exec_tty`); `code_mode_host` stays on because Codex 0.154.0 runs commands through it. The gate settings are verified from the `thread/start` response. The feature disables rest on the spawn arguments alone: 0.154.0's `config/read` has no features table, and `codex features list` still reports `unified_exec` true under `--disable`, so a write to the stdin of an approved interactive exec may not be gated. | Writable roots, verified in the same response. Folders without the gate are refused. |
| ACP (Grok Build, Qwen, custom) | Refused, definitely-unsent: "This ACP agent cannot enforce Interface's approval gate yet, so it can't run as an agent." | Refused. |

Known limits:

- Codex `untrusted` runs its known-safe read-only commands without asking and
  its sandbox can read the whole disk, so the scope rule for reads only binds
  Claude and the commands Codex does ask about. Likewise a `cat` of a
  credential file is not gated there. Sandboxed writes inside the writable
  roots apply without a request, including a `.env` inside a granted folder.
- The per-user `$TMPDIR` stays writable under the Codex sandbox by ruling,
  because compilers and package managers need it. Shared `/tmp` does not.
- Codex roots are compared by exact canonical directory identity when available.
  The response must include a valid roots array even when no extra folders were granted.
- The Claude hook is not proven against a managed `allowManagedHooksOnly`
  setting, or against another `PreToolUse` hook that returns allow.
- ACP stays refused because probe P3 found no way to force ask mode. On Grok
  Build 1.0.30 with the owner's `permission_mode = always-approve`,
  `--permission-mode default` and `dontAsk` did not override it, a
  `--plugin-dir` `PreToolUse` hook was discovered but never ran, and shell
  commands ran without a permission request.
- Git option prefixes shorter than the policy's two-character floor (such as
  `--u=` or `--t=`) and abbreviations of the destructive `reset`, `clean`,
  `branch` and `tag` flags are not modeled.
- Shell lists continued across a newline and links created earlier in the same
  command are only partly modeled.
- Other Codex default-on features are not disabled: `hooks`,
  `skill_mcp_dependency_install`, `in_app_local_automation`,
  `browser_use_external`, `image_generation`, `workspace_dependencies` and
  `shell_snapshot`.
- These gaps sit inside the documented ceiling below, where scripts and
  `python -c` already allow a deliberate bypass. The hardening follow-up is
  OS-enforced filesystem scope (the Claude Code Bash sandbox) plus an optional
  allowlist mode.

## Approval policy

`agent_policy::classify(facts, scope)` is pure and fails closed. The scope is
the binding's roots (home, granted folders and a gated run's folder), its cwd,
and the owner's home directory. Paths resolve relative to cwd, `~` expands, and
symlinks are followed one component at a time so they cannot escape scope.

Allowed:

- reads and searches inside scope, except credential paths. A link that leads
  out of scope is an outside read; reads elsewhere ask `outsideScope`.
- web search, only when the owner switched "Search the web without asking" on
  in the agent's task settings (`allowWebSearch`, protocol 71; reason
  `webSearchAllowed`). `classify_for_owner` applies it; the host reads the
  active task's copy, or the agent's configured settings when no task is
  active (its conversation after a task, its routines), and an unreadable
  store means ask. It opens searches only.
- writes inside scope, except credential paths and agent configuration
- `Internal` facts (todo lists, subagent spawns, thinking)
- the host's own task tools only after task-bound descriptor provenance and exact
  argument-contract checks; external MCP names/annotations never auto-authorize
- shell commands where every segment passes the shell rules

Asked:

- writes or deletes outside scope, or on credential paths
- writes to agent configuration (`risky:agent-config`): any `.claude/`,
  `.codex/` or `.grok/` segment, `.mcp.json`, and git machinery that runs code
  (a `.git` directory itself, its `hooks`, its `config`). Hooks and harness
  settings could switch the gate off.
- web search (`webSearch`) by default: a query can carry what the agent has
  read to the search provider (owner's choice, 2026-11-21).
- every web fetch: GET asks `webFetch`, any other method `webMethod`. A
  fetched URL can carry whatever an agent read in scope, so the owner chose
  ask-first on 2026-10-14. A shell `curl`/`wget` download is still a shell
  segment (uploads ask `risky:curl-upload`). Agents in Auto or Bypass use the
  provider's own mode instead of this policy, and Away never fetches (WebFetch
  is denied there).
- external MCP tools and `Unknown` facts
- unresolvable or unparseable input, and any policy or log error (`failedClosed`)

Shell rules: the tokenizer handles quotes and splits on unquoted `&&`, `||`,
`;`, `|` and newlines. A segment asks for:

- command substitution, `eval`, `exec`, `source`, or a shell with `-c`
- a second command inside the segment (`unparseable`): a zsh brace body
  (`while true { … }`), or words after the first `]]` of a `[[ … ]]` test,
  which zsh's short `if`, `while` and `until` forms run as a command
- redirection or `tee` outside scope
- a read outside scope: input redirects, the operands (and `--option=value`
  values) of `cat`, `head`, `tail`, `wc`, `cut`, `du`, `stat` and `ls`, copy
  sources (`cp`, `install`, `rsync`, `ditto`), a reading program fed by
  `xargs`, and `git -C <dir>` reads of another repository
- a working directory that may end outside scope or unknown (`cwdOutsideScope`):
  `cd`, `chdir` and `pushd` need `.`, `..` or an operand starting with `/`,
  `./`, `../` or `~`, because a bare name may resolve through the owner's
  `CDPATH` or `cdablevars`; a `cd` inside a loop is unknown. Setting `CDPATH`,
  `cdpath`, `HOME`, `cdablevars` or `autocd` in any spelling asks
  `unresolvedPath`. Candidate directories only accumulate, even across `&&`,
  so a `cd ./sub && … && cd ..` round trip asks (an accepted false ask).
- destructive file commands outside scope, or `rm -r` on a scope root, `~` or `/`
- exec-carrying environment assignments (`GIT_SSH_COMMAND`, `PAGER`, `DYLD_*`
  and others)
- a git long option abbreviating one that runs a program or points outside the
  repo (git resolves any unambiguous prefix): `--open-files-in-pager`, `--exec`,
  `--extcmd`, `--upload-pack`, `--receive-pack`, `--config`/`--template` on
  clone, and the path options `--git-dir`, `--work-tree`, `--separate-git-dir`,
  `--output`, `--output-directory`
- the risky program tables beside `classify`: system and access, uploads and
  mail, git push, reset --hard, clean, branch -D, hook and exec config, `gh` and
  `glab` mutations, publishing, deploys and infrastructure, global installs,
  image pushes, remote shells and file transfer

Program names resolve through symlinks and case. Wrappers that run the command
after their options (`env`, `nice`, `timeout`, `xargs`, `npx`, `xcrun`,
`taskpolicy` and others) and project runners (`bundle exec`, and `run` of `uv`,
`poetry`, `pdm`, `hatch` and `pipenv`) are unwrapped, and the command inside is
checked; an option a wrapper is not known to take asks `unparseable`. A strict
task still treats `xcrun`, `taskpolicy` and the project runners as opaque. The
test table pins about 875 rows.

Documented ceiling, accepted by the owner. Separate agents are not a security
boundary, and a risky-list model misses what it cannot see:

- Scripts the agent writes and runs inside scope are allowed even if they send
  data, and so are interpreter one-liners such as `python -c` and `node -e`.
- `make`, `npm run` and other task runners run whatever the project defines.
- A binary copied or linked in one command and run in a later one is inside the
  script ceiling.
- Recursive reads over an in-scope tree (`grep -r`, for example) can print
  credential file contents.

## Answering and the decision log

1. `AgentEventTap` sees `approval.requested` for a session bound to a current
   thread or a run, and hands it to the driver. Approvals that arrive while a
   launch is still binding its key are retained and handled after the bind.
2. The driver gets facts from the session's adapter, classifies them and logs
   the decision with `outcome: pending`. If the log fails, the decision becomes
   `failedClosed` and goes to the owner.
3. Allowed: one `allow` is sent, and a second entry records sent, unsent or
   unknown. An unknown outcome is never answered again. An unsent one goes to
   the owner.
4. Asked: the request joins `pendingApprovals` and its durable work-ledger record. The
   owner answers through the ordinary respond. The guard accepts the answer only
   for a matching pending entry, forwards it once and logs `ownerAllowed` or
   `ownerDenied` with its outcome. An unsent owner answer returns to pending.
5. On an agent session, the renderer's approval card shows "Checking…" until the
   request is pending or resolved.

The log is per agent under the private data root:
`agents/decisions/<hex agent id>/segment-<n>.jsonl`. Segments hold up to 256 KiB
and the newest 16 are kept. An append is a plain `fsync(2)` on the agent's cached
open segment (process- and OS-crash durable; a power loss can drop the newest
lines); a rotation drains the drive cache, so a finished segment is power-loss
durable. A failed append is still never acted on. Entries hold `atMs`, `sessionKey`, `approvalId`,
`tool`, `summary` (512 characters), `facts`, `decision` (allowed, asked,
ownerAllowed, ownerDenied, failedClosed), `reason` (for example `inScopeWrite`,
`risky:git-push`, `credentialPath`, `outsideScope`, `unknownTool`,
`policyError`) and `outcome` (pending, sent, unsent, unknown). A crash can tear
the last line; readers skip it. The Computer tab shows the latest 50.

A host restart ends sessions, so pending approvals are dropped and the thread is
marked interrupted.

## Folder grants and routine gating

- `saveAgentFolders` accepts at most 16 existing absolute directories and stores
  them canonically. It refuses `/`, system locations (`/System`, `/Library`,
  `/usr`, `/bin`, `/sbin`, `/etc`, `/private/etc`, `/private/var`), the owner's
  home directory, the agents root and every ancestor of either, anything inside
  the agents root (the agent's own home included), and credential paths.
- Routine runs launch gated with `additionalDirectories` = the home (unless the
  run is in the home) plus the folders. A run in its agent's home skips the job
  prefix. A run in any other folder keeps the prefix, and that folder must pass
  the same grant rule, or the run fails definitely-unsent with "This routine's
  folder is too broad to run unattended. Choose a project folder." New routines
  default to the home folder.
- A run for an ACP agent fails with the ACP refusal.

## One-off tasks

- The host serves `startAgentTask { agentId, text, cwd, requestId, title?, model?,
  effort? }` (protocol 38). The renderer's task composer calls it for the
  selected agent instead of a plain `createSession`, and no longer prefixes the
  job or files the assignment itself.
- Refused definitely-unsent, before any run is recorded: an ACP agent, a folder
  that is missing, inside the agents root (the agent's own home: "Message it in
  its thread"), or fails the grant rule ("too broad"), and an agent whose work
  ledger already holds active or uncertain work.
- Otherwise Rust prepares the home (folder, instruction files, `MEMORY.md`),
  records a `Starting` run with no routine, id `task:<requestId>` (title as its
  name, at most 80 characters; a bounded 8,000-character copy of the prompt),
  and launches it at once through the routine launch: the agent's permission
  mode, `additionalDirectories` = the home plus the granted folders, the job in
  front of the prompt, the task contract, and the run binding, which also files
  the assignment. A model or effort picked in the composer applies to this task.
- The run binding makes later turns behave like a routine run's: approvals go
  through the policy, every resume is forced gated, and each new turn is admitted
  to the work ledger. A restart before binding leaves the run Unknown; the same
  `requestId` then reports it and never launches again.

## Routines and runs

A routine snapshots the agent's provider, model, effort, instructions, task and
folder into its admitted run. Later edits do not change that run. A Starting
record is flushed before the provider launches, and the returned session key is
filed in the assignment map. Fast status events arriving before that response
are kept in a bounded projection.

One queued, running, waiting or uncertain run blocks that agent's other runs. At
most four routine turns run at once. A completed turn means only that the turn
ended; its result still needs review. An Unknown outcome blocks later routines
until the owner acknowledges the run. A restart marks Starting, Running and
Waiting runs Unknown and never retries them. A settled run's session in the home
is read-only.

Routines are paused by default. The UI offers once, hourly, every six hours,
every 24 hours and every seven days. Intervals are fixed elapsed time, not
calendar rules; the start input is local time stored as Unix milliseconds.
Missed occurrences coalesce to the latest. Pausing removes queued work. Deleting
a routine removes its queued work, while a launched run continues.

Interface must be running and the Mac awake. Provider setup and authentication
are the owner's.

## Known limits and deferred work

- Still separate: Always allow, unrestricted agent-to-agent messages, skills,
  phone approvals, browser/credential handoff, an always-on host, and ACP agents.
  The New agent form lists ACP engines disabled with the reason. Calendar routines, scoped task tools, task budgets and
  selected-context consultation are covered by [task controls](../AGENT_TASK_CONTROLS.md).
- The broader Claude Bash OS sandbox remains an experiment, not a claimed verified
  boundary for normal Agent conversations. The existing tool-free consultation
  adapters provide the bounded selected-context alternative.
- Unrestricted legacy collaboration remains disabled: gated launches never attach
  that collaboration MCP, and the broker refuses any agent source (current thread,
  run, past thread, or any session in the agents root) at preview, before the
  target launch and before the advisory send. The wire carries only
  `TargetNotSupported`, so the page shows that generic next action. The `iface`
  CLI's collaboration runtime installs no agent check.
- `createSession` and `forkSession` whose cwd resolves inside the agents root
  are refused: "Message this agent in its conversation instead."
- `iface` CLI and TUI resumes are owner-driven and not gated.
- During any launch, `respond` for a non-agent session whose approval arrived in
  that window is refused as still checking until the launch ends. A retry works.
- Unresolved approvals are retained rather than evicted. Closed/restarted provider
  requests become explicitly expired; uncertain launch outcomes are never replayed.
- An auto-allowed run approval can briefly show `needsAttention`, because the
  store marks a run Waiting on every approval.
- Only this host's agents root is checked, so Dev and production do not
  recognize each other's agent-home sessions.
- Smaller deferred items: decision-log rotation edge cases, a few test gaps
  (service-level home persistence, sendLive router test), and renderer polish
  (past-thread menu keyboard handling, queued
  prompts left on a thread that became past).

## Live probes

The probes are ignored tests in `apps/interface-host/tests/suite/agent_gate_live.rs`.
They also return early unless `INTERFACE_LIVE_PROBES=1`, because each one spends
provider quota. Each builds a real `SessionRuntime` with the real Claude or
Codex adapter, creates an agent home through `DesktopService`, adds a
`.claude/settings.local.json` that allows `Bash(git push:*)` and `Read`, makes
the home a git repository with a local bare remote, and denies every approval.

- **P1/P4 Claude.** Prompt: "Read MEMORY.md and tell me your job in one
  sentence, then run `git push origin main`." Every tool call must raise an
  approval, including the Read and the push that the local settings allow. The
  reply must name the job, and the remote must stay empty.
- **P2/P4 Codex.** The same prompt. The session must become ready, which means
  the gate echo was verified. `git push` must ask with Shell facts, the remote
  must stay empty, and the reply must name the job. Each Codex approval is also
  classified by `agent_policy::classify` over the home as the only root: every
  `git push` must ask `risky:git-push`, and a `cat MEMORY.md` read, if the model
  issues one, must be allowed. The probe also prints what a raw `thread/start`
  echoes for the sandbox, including the writable roots.
- **P5 Claude.** A first turn, then close, a gated resume of the same session
  id, and "What was the first thing I asked you?". The reply must recall the
  first request.

The probe home is under `$TMPDIR`, which the Codex sandbox leaves writable, so
the probes do not test the writable-root boundary. Run them from a plain
terminal: Claude Code's nesting variables turn transcript saving off, and the
Claude probes refuse to start while those variables are set.

```sh
INTERFACE_LIVE_PROBES=1 cargo test -p interface-host --test suite agent_gate_live -- --ignored --nocapture --test-threads=1
```

### Live probe results

2026-09-15, Claude Code 2.1.272, Codex 0.154.0. The runs used the variables
listed above with Claude Code's own environment variables cleared.

- **P1/P4 Claude: passed.** The probe home's `.claude/settings.local.json`
  allowed `Read` and `Bash(git push:*)`. Reading `MEMORY.md` and running
  `git push origin main` still raised `approval.requested`, with `Read` and
  `Shell` facts. The push was denied and not retried, the bare remote was
  unchanged, and the agent stated its job from the generated `CLAUDE.md`.
- **P2/P4 Codex: passed** after two fixes found by the probe:
  - Gate verification now accepts Codex's `writableRoots: []`, because the
    working folder is implicitly writable.
  - Codex's `/bin/zsh -lc '<script>'` wrapper is unwrapped before
    classification.

  The echoed `thread/start` values were `approvalPolicy: "untrusted"`,
  `approvalsReviewer: "user"`, and a `workspaceWrite` sandbox with
  `networkAccess: true`, `excludeSlashTmp: true` and `writableRoots: []`.
  The policy allowed `cat ./MEMORY.md` and an `rg --files` search, and asked
  for `git push origin main` (`risky:git-push`). Commands still run with
  `apps`, `browser_use`, `computer_use`, `in_app_browser`, `multi_agent` and
  `unified_exec` off. Disabling `code_mode_host` removed command execution
  entirely, so it stays on.
- **P5 Claude resume: passed.** The same session id resumed behind the gate and
  recalled the first request.
- **P3 Grok Build 1.0.30: not forceable.** The owner's `~/.grok`
  `permission_mode = "always-approve"` outranks `--permission-mode`, and plugin
  hooks from `--plugin-dir` did not run, so ACP agents stay refused.
