# Current desktop protocol facts

This table is checked by `electron/scripts/protocol-docs.test.mjs` against the
Rust host version and Electron constants. The existing contract suites check
Rust/TypeScript channel identity, ownership and counts. Update the protocol and
cross-language tests together when deliberately changing a wire surface.

| Fact | Value |
| --- | --- |
| Host protocol version | 72 |
| Request operations | 127 |
| Push operations | 12 |
| Desktop channels | 139 |
| Terminal data protocol | 2 |

`apps/interface-host/src/protocol.rs` and
`electron/src/main/rust-host-client.ts` carry the handshake constants.
`crates/interface-contracts/src/ipc.rs` and `electron/src/shared/ipc.ts` carry
the frozen channel vocabulary. Retained channels remain part of the protocol
even when there is no current in-repository caller.

The Dev-only v25 sidebar collections trial was withdrawn on 2026-09-21.
The rollback restores the unchanged v24 contract and appearance surfaces 1–4;
v25 remains a historical trial identifier. Its separate collections file is
left untouched and is no longer read or written.

Protocol v52 adds `iface:updateSessions` with `read`, `prepare`, `cancel` and
`resume` commands and a version 1 per-session checkpoint view. `restartToUpdate`
requires the matching ready barrier; it keeps its empty request and boolean
response. Rust records before-send receipts in the new private
`update-sessions.v1.json`, waits for each model's acknowledgement and completion,
and resumes exact checkpointed identities after the requested version starts.
Rust and TypeScript share `electron/src/fixtures/update-sessions-v52.json`.
Existing stores and terminal data frames are unchanged. See
[update checkpoints](archive/update-checkpoints-20261021/UPDATE_SESSION_CHECKPOINTS.md).
The feature was removed on 2026-10-21 without a count or version change:
`iface:updateSessions` stays served, answering `read` with an idle view and
refusing the other commands before any effect, and `restartToUpdate` installs
directly again. See [the archive](archive/update-checkpoints-20261021/README.md).

Protocol v16 adds appearance surface v3 with independently persisted terminal reading size and workspace pins. Legacy appearance surface v1/v2 and workspace schemas remain accepted.

Protocol v15 adds the native [pane workspace tools](WORKSPACE_TOOLS.md) request.

Protocol v17 adds `iface:remoteAccess`, the desktop-only administration surface for iPhone remote access v1. Terminal data v2 and persisted schemas are unchanged.

Protocol v18 adds the explicit v3 trusted-device administration envelope on `iface:remoteAccess` and HTTP v3 enrollment/unlock. Legacy v1 administration, v2 workspace/history projections, channel counts, and terminal data v2 remain accepted. The separate private `remote-trust.v1.json` stores device digests and the opted-in loopback listener.

Protocol v19 adds appearance surface v4 with independently persisted project pins. Surface v1-v3 responses, workspace pins, terminal data v2 and channel counts are unchanged. Project paths are display identities and grant no filesystem authority.

The phone connection's [v6 QR wrapper](PHONE_CONNECTION_V5.md#direct-address-pairing-wrapper-v6)
adds the explicit desktop-only `pairDirect` v6 envelope on `iface:remoteAccess`.
The host handshake, channel counts, v5 administration and encrypted transport
remain unchanged; Rust, TypeScript and Swift share `remote-connection-v6.json`.

Protocol v20 adds `iface:harnessControl`, the additive Rust-owned command, attention and task-evidence surface v1. Legacy stores and terminal data v2 remain unchanged.

Protocol v20 also adds `iface:inspectTerminalServers`, a read-only, generation-fenced process-tree listener projection. Listening does not establish health.

Protocol v21 adds `iface:voiceControl`, the audio-independent control desk v1. Its separate durable store records bounded owner grants and never replays ambiguous actions. Existing voice channels and audio helper remain available.

The Control assistant was retired on 2026-10-14
([archive](archive/control-assistant-20261014/README.md)). `iface:voiceControl`
stays in the channel table with its v1 request and snapshot shapes and the
`voice-control-v21.json` fixture; the host decodes each request and refuses a
well-formed one with `voice-control-unavailable` (replay-safe: nothing ran).
The phone's `POST /api/v1/voice-control` gives the same refusal after its
grant check. No shape, count or protocol number changes.

Pane delegation was retired on the same day
([archive](archive/pane-delegation-20261014/README.md)). `iface:readTerminalDelegation`
keeps its v3 view and `terminal-delegation-v12.json`; the host answers a current
terminal generation with `available: false` and no workflows, as default builds
have since 2026-10-06. The Tailscale phone route was retired too
([archive](archive/tailscale-route-20261014/README.md)): `iface:remoteAccess`
v1 `enable` keeps its `origin` field but serves only the empty (loopback) value,
refusing any other with `invalid-origin`, and `publicUrl` is always null. Phone
`MacRoutesV1` keeps version 1 and admits nearby origins only. None of these
changes a shape, count or the protocol number.

On 2026-11-14 `iface:stopOrchestrationAgent` and
`iface:readOrchestrationTranscript` began accepting `agentId`, the spelling the
TypeScript contract (and 0.1.169) always sent; the host had accepted only
`agent_id`, so the Agent Canvas Stop buttons were refused as an invalid
payload. `agent_id` stays accepted, and Rust callers now send `agentId`.
Channel counts and the protocol number are unchanged.

Protocol v22 adds `iface:projectAway`, the one-button project loop v1, and permits
idle Codex thread creation with an empty launch prompt. Ordinary sends still
require nonempty text. This lets the host persist the session identity before
dispatching project work. The pilot, CLI cycle and existing stores are unchanged.

Protocol v23 changes `iface:agentControl` to the agent teammates v2 response
(`statuses`, `pendingApprovals`, optional `files`/`decisions`/`sessionKey`), adds
thread, memory, folder and decision commands, and adds `approvalGate`/
`additionalDirectories` to launch and resume options. The v1 agent store file is
preserved; v2 is `agents.v2.json`.

Protocol v24 adds `iface:setTerminalWorkspaceActivity`. Its v1 request carries
an exact terminal generation, workspace visibility, and optional rendered prompt
with a cumulative output cursor. Rust owns idle detection and process suspension;
no terminal state, saved workspace schema, or binary data frame changes.

Protocol v26 adds `iface:createProject` with `{name}` and a canonical path
response. Rust creates a new folder in `~/Desktop/Projects` and grants its
workspace root. Existing entries are never reused or overwritten. No Git or
remote repository is created; persisted schemas and terminal data are unchanged.

Protocol v27 adds `iface:resolveMovedTerminalFolder` with `{path}` and a
`string | null` response, and an optional `movedFolders: [{from, to}]` on
`iface:setTerminalWorkspaces`. Resolve answers only for a saved pane's cwd that
now leads to a different real directory (the folder moved and left a link). A
save carrying a move re-proves that `from` still leads to exactly `to`, then
carries matching unmanaged host restart proofs and preserved provider cwds to
the new path in the same save; anything else refuses the whole save. The
terminal cwd gate is unchanged, and persisted schemas and terminal data are
unchanged.

Protocol v28 adds the read-only project Away status v2 request on the existing
channel. It returns revisions, active project identities and start/cancellation
receipts without report bodies. Existing v1 Away requests and snapshots, all
persisted schemas, channel counts and terminal data frames remain unchanged.

Event buffer policy v2 is the deliberate streaming compatibility change shipped
with v28: provider/subscriber queues gain payload budgets, discarded transcript
content produces a warning notice, and control-only overflow fails closed.
No event DTO variant or persisted schema changes. The legacy count-only factory
retains its previous behavior for callers that explicitly select it.

Protocol v29 adds versioned agent work contracts, bounded execution, artifact verification, memory provenance and calendar routine controls through `iface:agentControl`. Existing v2 roster and interval routines remain readable.

Protocol v30 adds the read-only `iface:previewOutput` v1 list/read surface for
the [output preview workspace](OUTPUT_PREVIEW.md). Rust returns bounded,
credential-filtered, SHA-256-bound text/image/PDF snapshots from current approved
project roots. Persisted schemas and terminal data frames are unchanged.

Protocol v37 adds shipping a thread's work to `iface:threadControl`: the
commands `shipStatus`, `commit`, `createBranch`, `openPullRequest`,
`failingChecksPrompt`, `reviewComments` and `mergePullRequest`, and the
response fields `ship` (the checkout read for shipping: branch, upstream
counts, the files Commit stages and those it leaves out, the suggested
message, why a push is blocked, `gh`'s state and how to fix it, the pull
request and its checks), `shipped` (a ship effect's `done` or `unknown`
outcome; a refused one is an error) and `reviewComments`;
`failingChecksPrompt` answers in `prompt`. A worktree thread commits every
change; a Local thread only the files its own turns' edits named; neither
ever commits a credential-shaped file, and a Local thread on the default
branch is never pushed. Push, pull request create and merge are re-read
through git and `gh` when their outcome is unclear and never retried; every
git and `gh` child is bounded and credential-filtered. Rust and TypeScript
share `electron/src/fixtures/threads-v37.json`; channel counts, persisted
schemas and terminal data frames are unchanged.

Protocol v36 adds each Threads turn's `endedAt` (when its run ended) to the
`ThreadsView` that `iface:threadControl` answers and `iface:threads` pushes,
so "Worked for…" survives a restart. Absent means unknown: a turn still
running, one recorded before v36, or one closed without an end the run tap
saw. The thread store moves to `threads.v2.json` (store version 2): each turn
also keeps the repository-relative files its own edits named and whether
others shared its checkout when it ended, and such a turn's `turnDiff` (and
`threadDiff` scope `turn`) covers only those files, refusing with
`threads-turn-files-unknown` when they are unknown. A `threads.v1.json` is
read once, never changed, and carried over with those fields absent. Rust and
TypeScript share `electron/src/fixtures/threads-v36.json`; channel counts and
terminal data frames are unchanged.

Protocol v51 adds the provider CLI drift canary to Settings' Terminal tools
(review F2, 2026-10-15). A `details` listing's `TerminalLaunchProfile` gains
`changedSinceLastCheck` (`TerminalToolChangeV1`: `version` 1, `from` when the
CLI's version is what changed, `to`, `checkedAt`, and `findings`, one
sentence per thing Interface depends on that no longer matches). The host
checks a found CLI again whenever its `--version` line differs from the one it
last checked, or the canary's expectations changed, using only `--version`,
`--help`, listing and schema output and the installed files (never a model
turn), and keeps what it saw in `provider-canary.v1.json`. A change is shown
for a week, or while something still does not match. The launcher's plain
listing is unchanged. Rust and TypeScript share
`electron/src/fixtures/terminal-tool-change-v51.json`. Channel counts and
terminal data frames are unchanged.

Protocol v50 moves the agent-exit shell handoff into Rust. An agent CLI
terminal (any launch profile but `shell`) that exits with code 0 by itself is
handed a plain shell in its folder by the host the moment that exit is
delivered, whatever the window shows; before, the renderer did it only for a
pane in the shown workspace, so a parked workspace's pane stayed a dead tab
until it was shown. An exit anyone asked for (`closeTerminal`: a closed tab,
a deleted workspace, a voice or phone close, quitting), a crash, a signal or
a failed start keeps its tab and its Start again. `TerminalSummary` gains
`replaces`, the exited agent whose pane the shell took; the host then
dismisses that agent. The renderer moves the tab from that id to the shell
in the live and saved layouts and files the shell in its place, also on load,
and never reopens a saved agent a live shell replaces. Rust and TypeScript
share `electron/src/fixtures/terminal-shell-handoff-v50.json`. Channel
counts, persisted schemas and terminal data frames are unchanged.

Protocol v48 makes Interface's tools opt-in for Grok and Antigravity (the
owner's security choice, 2026-10-14). The host no longer writes the `iface`
MCP entry into their user config at start. `TerminalLaunchProfile` gains
`paneTools` (`on`, `off`, `taken` when an `iface` entry that is not
Interface's exists, `unknown` when the CLI's `mcp list` could not be read),
present in a `details` listing for a found Grok or Antigravity only.
`listTerminalLaunchProfiles` also takes `paneTools: {profileId, enabled}`:
on writes Interface's entry through the CLI's own `mcp add` (never over a
`taken` one), off removes it through `mcp remove` only when it is exactly
Interface's, and the answer is the details listing. An entry an earlier
host wrote stays and reads `on`. Every CLI command is bounded to ten
seconds; one that did not answer is not replay-safe. Rust and TypeScript
share `electron/src/fixtures/terminal-pane-tools-v48.json`. Channel counts,
persisted schemas and terminal data frames are unchanged; the old
`<data root>/panes/iface-mcp-config.v1` marker is no longer read.

Protocol v47 lets a project Away loop's reviews run on another provider
(owner, 14 Oct: "Other vendor by default"). `iface:projectAway`'s options
(defaults, presets, runs, and the `start` and `save` commands) gain the
optional `reviewer` (`codex` or `claude`); absent means the worker's own
provider and model review, as in every loop before. A reviewer from the other
provider runs at its own default model with its own read-only restrictions
(Codex read-only sandbox; Claude without Write, Edit, Bash and the rest).
`ThreadView.away` gains `reviewer`, the provider that reviews. The Away store
moves to `project-away.v3.json` (store version 3): a `project-away.v2.json`,
else a `project-away.v1.json`, is read once, never changed, and carried over
with no reviewer. Rust and TypeScript share
`electron/src/fixtures/project-away-v47.json`; channel counts and terminal
data frames are unchanged.

Protocol v46 adds a second opinion and Continue with to `iface:threadControl`
(review B4). `secondOpinion {key, engine, requestId}` submits an ordinary
human consult through the collaboration service: the thread's session is
resumed through the runtime dispatcher when it is not open, the objective
names the latest turn's changes (bounded by the objective's 4,000 characters)
and the consult packet adds the recent conversation; the answer's `consult`
is the `CollaborationStatusDto` the collaboration channels report, awaiting
the owner's approval of the exact packet. `continueWith {key, engine}`
prepares the thread's folder (Local) or its existing worktree again for a new
thread on `engine` and answers `prepared` and the first `prompt` (the
thread's changes and newest conversation, at most 32 KiB); the renderer
launches it and binds it with the token, and the new thread's lineage is the
new `handoff` relation (`keptTurns` 0). Both refuse the thread's own
provider and a running turn; Second opinion also refuses a session a Code
pane holds, and the collaboration port refuses every preview and every
effect that would write to such a session (target launch, advisory, park),
definitely unsent. The thread store moves to `threads.v3.json` (store
version 3), which may hold a `handoff` lineage and a prepared workspace's
lineage; a `threads.v2.json` (else `threads.v1.json`) is read once, never
changed, and carried over. Rust and TypeScript share
`electron/src/fixtures/threads-handoff-v46.json`. Channel counts and
terminal data frames are unchanged.

Protocol v45 runs each project Away loop as a Threads thread (review B3).
`iface:projectAway`'s v1 snapshot gains the optional run `thread` (the work
session's key, which is the thread's, the loop's own worktree and its branch)
and each work turn's `threadTurn` (the thread turn its prompt ran as); both
are absent for loops from before. `ThreadView` gains `away`, Rust's
projection of the loop a thread runs as: its state, detail, provider, model,
objective, start, deadline, iteration limit and rounds (role, state, the
thread turn of a work round, a review's own session, the verdict, summary
and checks from its `AWAY_RESULT`, cut for display; only the newest 24 loops
carry their rounds). While a loop runs it holds its thread: any other send,
`restoreTurn`, `editFrom` and `openInTerminal` refuse with
`threads-away-running`, definitely unsent. Each work round is a turn of the
one work session in the loop's worktree; each review is a fresh session in
that worktree and stays out of Threads. The Away store moves to
`project-away.v2.json` (store version 2): a `project-away.v1.json` is read
once, never changed, and carried over with every run's thread absent; an
unreadable one keeps Away unavailable, as before. Rust and TypeScript share
`electron/src/fixtures/project-away-v45.json` and
`electron/src/fixtures/threads-v45.json`; channel counts and terminal data
frames are unchanged.

Protocol v44 moves "needs you" for terminal panes into Rust (review D6).
`TerminalSummary` gains `attention` (`{version: 1, since}`), present while
the pane's program waits for the user: Claude's PermissionRequest hook frame,
a Codex approval-family OSC 9 message, the host's private OpenCode `waiting`
title, or a bare BEL (not from Codex, nor from Claude beside a finished turn),
until a spinner title, the CLI's own end or resume, or a keystroke that is not
a terminal protocol reply. `crates/interface-terminal/src/attention.rs` reads
the PTY bytes and input the host already sees, so a pane nobody has opened,
or one a reloaded window has not seen a byte of, still says so; each change
is an ordinary lifecycle upsert. The renderer publishes Rust's answer as the
pane's "waiting" instead of its own. Rust and TypeScript replay the shared
`electron/src/fixtures/terminal-attention-v44.json`. Channel counts,
persisted schemas and terminal data frames are unchanged.

Protocol v43 puts the launcher's tools on Settings → Models & providers
(review C4). `TerminalLaunchProfile` gains `installHint` (how to install a
missing CLI, ending "then press Refresh") and `version` (the found CLI's
`--version`, first line, at most 60 characters, kept per file).
`listTerminalLaunchProfiles` also takes `{details: true}`; only that listing
runs the `--version` reads, each bounded to five seconds, so the launcher
never waits on them. Executable paths still never leave Rust. Rust and
TypeScript share `electron/src/fixtures/terminal-tools-v43.json`. Channel
counts, persisted schemas and terminal data frames are unchanged.

Protocol v42 adds the bridges between Threads and Code to
`iface:threadControl` (review C1). `ThreadsView` gains the section
`fromTerminals`: a session a Code pane or the CLI started, settled `external`
and never brought into Threads. The command `openInTerminal {key}` refuses
while a turn runs or a pane already holds the session, closes the thread's
structured session (an unconfirmed close is not replay-safe and holds
nothing), holds the session for the pane and answers `terminal`
(`profileId` `claude` or `codex`, `providerSessionId`, `cwd`) for the pane
the renderer opens with `resume: true`. The command `continueFromTerminal
{key}` waits up to five seconds for no running pane to be bound to the
session, then un-settles and visits it. Every runtime resume and every send
or setting change refuses, definitely unsent, a session a running Code pane
is bound to or one held for a pane (30 seconds at most): one provider session
never has two writers. Rust and TypeScript share
`electron/src/fixtures/threads-v42.json`; channel counts, persisted schemas
and terminal data frames are unchanged.

Protocol v41 is about signed-out providers and CLIs installed while
Interface runs. `EngineInfo` gains `auth` (`signedIn` | `signedOut`, absent
when the CLI cannot say), from each CLI's own status: `claude auth status
--json` and Codex's app-server `account/read`; Interface never reads their
credentials, and ACP agents have no status command. The `error` engine event
gains `cause: "signedOut"` when the provider refused for that reason
(Claude's `authentication_failed`, Codex's `codexErrorInfo: unauthorized`,
ACP's reserved "Authentication required"). `CreateTerminalRequest` gains
`signIn`: the `launch` profile's own sign-in (`claude auth login`, `codex
login`, `grok login`; Rust owns the argv) instead of a chat, with no chat
binding, session or pass-off. `probeEngines` and `listTerminalLaunchProfiles`
accept `{refresh: true}`: Rust runs a fresh login shell, rebuilds the launch
catalog and repoints each engine's executable before answering, so Refresh
and the launcher's Try again find a CLI installed since start; running panes
and sessions keep their process, and each launch reads one catalog. An
absent payload keeps the pre-v41 behaviour. Rust and TypeScript share
`electron/src/fixtures/providers-v41.json`. Channel counts, persisted
schemas and terminal data frames are unchanged.

Protocol v40 adds the Projects folder to `iface:createProject`. Its v1
request `{name}` is unchanged and now creates in that folder: the one the
owner chose with Change… while it exists, else `~/Developer/Projects` while
it exists, else the folder holding the last folder picked in Interface, else
`~/Developer/Projects` (created). The same channel takes
`{projectsFolder: "read"}` and `{projectsFolder: "choose", selectedPath}`,
both answered with a `ProjectsFolderView` v1 (`path`, `display` with `~`,
`source`: `chosen`, `default` or `lastPicked`). Electron's main process
opens the native picker for `choose` and replaces `selectedPath` with its
answer (null when cancelled). The choice and the last picked parent folder
live in the new private `projects-folder.v1.json`. Rust and TypeScript share
`electron/src/fixtures/projects-folder-v40.json`; channel counts, existing
persisted schemas and terminal data frames are unchanged.

Protocol v39 adds `iface:diagnostics`, what a user can reach when something
goes wrong. Its commands are `report` (the Help → Copy Diagnostics text:
version, macOS, architecture, the data folder, each provider's probe and the
last 50 WARN/ERROR lines of `host.log`, credential-filtered in Rust and naming
no account), `notices` and `dismissNotice` (this run's one-time notices for a
store the host could not read and set aside: the threads list, the
permission modes threads resume in, the session cache), and `reveal` (Finder
shows `host.log`, the data folder, or a notice's set-aside file). Notices live
for the run only; a reveal is never replayed. `host.log` is new: the host's
WARN and ERROR lines, owner-only in the data root, rotating at 5 MB with one
predecessor. Rust and TypeScript share
`electron/src/fixtures/diagnostics-v39.json`. Persisted schemas, push
channels and terminal data frames are unchanged.

Protocol v38 adds the host-only `startAgentTask` request on `iface:agentControl`:
a teammate's one-off task (`agentId`, `text`, `cwd`, `requestId`, optional
`title`, `model`, `effort`) launched behind the agent's own gate, like a routine
run: its permission mode, its home and granted folders as additional
directories, its job in front of the prompt, and only in a folder the grant rule
accepts. It is recorded as a run with no routine (`task:<requestId>`), which an
older build reads as an ordinary run. The answer is the usual
`AgentControlResponse` with `sessionKey`. Rust and TypeScript share
`electron/src/fixtures/agent-task-start-v38.json`. Channel counts, persisted
schemas and terminal data frames are unchanged.

Protocol v35 adds the `muse` terminal launch profile (Meta's Muse Code). Launch
and workspace DTOs stay v1; Rust and TypeScript share
`electron/src/fixtures/terminal-muse-v35.json`. No channel, persisted schema or
terminal data frame changes.

Protocol v34 adds `iface:threadControl` and the `iface:threads` push, the
Threads surface v1 (Rust-owned thread state store `threads.v1.json`). A thread
is a Threads-surface session, keyed by its session key; Rust derives its
section (pinned, active, snoozed, settled), unread state, order keys and
workspace (Local checkout, a git worktree, or none for plain chat), and the
renderer only draws the `ThreadsView` and sends `ThreadCommand`s. Every
command answers a `ThreadControlResponse` carrying the whole view; the push
carries the whole view after every committed change. `iface:threadControl` is
never replay-safe as an operation; a refused command says definitely-unsent
explicitly. Rust and TypeScript share `electron/src/fixtures/threads-v34.json`.
`registry.json`, existing stores and terminal data frames are unchanged.
Refusals carry stable `threads-…` codes (for example `threads-unknown`,
`threads-workspace-shared`, `threads-worktree-unavailable`); a store that
did not open answers `threads-store-unavailable`. Design:
[threads design](superpowers/specs/2026-10-06-threads-design.md).

Protocol v33 adds theme files and appearance surface v5. A theme is a
`<name>.css` file in `~/.config/interface/themes`; the wire and
`appearance.json` name it `custom:<name>`. Surface v5 snapshots add
`customThemes` (one summary per file: id, name, light or dark kind, window
fill, revision, declared swatch colours, a problem when it cannot be painted),
`systemThemes` (the light and dark theme Follow macOS switches between,
persisted in `system-themes.v1.json`) and `themeCss` (the painted file's CSS).
`iface:setAppearance` accepts a `custom:` id or a `systemThemes` v1 value only
at surface v5. The new host push `iface:appearanceChanged` (empty payload)
fires when a file in the themes folder is added, removed or saved. Rust and
TypeScript share `docs/fixtures/custom-themes-v1.json`. Builds before v33 read
a saved `custom:` id as Follow macOS.

Protocol v32 adds agent permission modes. `iface:agentControl` gains the
`savePermissionMode` request, and every `AgentControlResponse` may carry
`permissionModes` (agents set to `auto` or `bypass`; an absent agent is
`gated`). Launch and resume options gain the host-set `agentAccess`
(`auto` | `bypass`), which replaces `approvalGate` for those agents and is
refused by providers that cannot apply it. Rust and TypeScript share
`agent-permission-modes-v32.json`. Channel counts, `agents.v2.json` and terminal
data frames are unchanged; the modes live in the new private
`agent-permissions.v1.json`.

Protocol v31 adds the live, never-persisted `startupDownload` v1 field to
`TerminalSummary`: the startup files (git HEAD/config/branch ref, the
CLAUDE.md/AGENTS.md chain, project settings) a pane's program is still waiting
for a File Provider such as iCloud Drive to download. Rust names at most three
and counts all of them; the renderer shows them over the pane until they are
local. Rust and TypeScript share `terminal-startup-download-v31.json`. Channel
counts, persisted schemas and terminal data frames are unchanged.

Protocol v49 adds three desktop requests: `backgroundControl`,
`customizationControl`, and `recoveryControl` (122 requests, 12 pushes). Existing
`threadControl` gains immutable commit reviews, explicit file/hunk commit choices,
and project workflow commands; `previewOutput` accepts an additive v2 durable
review envelope while retaining v1 reads. Prompt sends may carry original prompt
context for Rust-owned recovery receipts. Every extension has a shared Rust/TS
fixture; existing preference and thread schema versions remain unchanged.

Phone HTTP `/api/v2/inbox` combines authorized Threads and roster-bound Agent
work. The v1 Threads endpoints remain available; display limits never remove
authorization from an older client's visible thread. Shared Rust/TS/Swift
`remote-inbox-v2.json` pins read-only task and sessionless queue behavior.

Protocol v53 preserves captured Antigravity (`agy-<v4 UUID>`) and Muse Code
(`muse-<v7 UUID>`) pane bindings. The prefix identifies native provider proof;
legacy Gemini UUIDs still reopen fresh. The v1 DTO and persisted store shapes
are unchanged. The shared fixture is `terminal-native-resume-v53.json`.

Protocol v54 adds the `kimi` terminal launch profile (Moonshot's Kimi Code).
Launch and workspace DTOs stay v1; Rust and TypeScript share
`electron/src/fixtures/terminal-kimi-v54.json`. No channel, persisted schema or
terminal data frame changes.

Protocol v55 preserves captured Kimi Code (`kimi-<v4 UUID>`) pane bindings,
reported by Interface's Kimi plugin hooks and reopened as
`kimi --session session_<uuid>`; a Kimi pass-off is typed into its TUI once
the editor is ready. The v1 DTO and persisted store shapes are unchanged. The
shared fixture is `terminal-kimi-resume-v55.json`.

Protocol v56 adds the request channel `iface:threadJournal` (TS
`threadJournal`), owned by the Threads host beside `iface:threadControl`. It
only reads each thread's journal, the append-only record Rust writes under
`<data root>/thread-journal/` as a thread's prompts, answers, tool calls,
approvals and turn ends happen: `read` and `tail` answer a `JournalPage`,
`search` answers at most 100 `JournalHit`s (prompts and messages, literal and
case-insensitive, 2 s deadline). Every command is replay-safe. `readHistory`
is unchanged on the wire; for a thread whose provider answers it unavailable,
the host answers from the journal instead. Rust and TypeScript share
`electron/src/fixtures/thread-journal-v56.json`. No push, persisted schema of
an existing store or terminal data frame changes. Contract:
[T3-style systems, section 1](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v57 adds team tools: an agent in a Threads thread starts, waits
for, reads and messages other threads through the `iface_team` MCP server
the host gives each ordinary code session (a socket per session; the caller
is the socket, never tool input). The wire gains the lineage relation
`child` (a thread team tools started; `ThreadLineage.relation`) and the
setting `ThreadSettings.teamTools` (absent means on; only `false` is
written, so older views and fixtures read unchanged). No channel, push or
command is added or removed: request and push counts stay 124 and 12. The
Threads store becomes `threads.v4.json` (it reads `threads.v3.json` once and
carries it over), and a new Rust store `thread-team.v1.json` keeps the
started-thread ledger and call receipts. Rust and TypeScript share
`electron/src/fixtures/threads-team-v57.json`. Contract:
[T3-style systems, section 2](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v58 adds `hiddenKeys` to the `ThreadsView` that `iface:threadControl`
answers and `iface:threads` pushes: the registry's sessions Threads leaves out
as workers (orchestration workers and Away's reviews), sorted, absent when
there are none. The host reads orchestration workers from the whole agent
list (the owner's store, or a client paging `agent.list`), not the 64-agent
`getOrchestrationSnapshot`, past which old finished workers had come back as
new threads; Chat's session list now hides the sessions this field names
too, not only the workers that snapshot still holds. A change in which
sessions are hidden commits a new Threads revision so it reaches the window. Rust and TypeScript share
`electron/src/fixtures/threads-hidden-v58.json`. Channel counts, persisted
schemas, the orchestration control protocol and terminal data frames are
unchanged.

Protocol v59 adds switching a thread's provider inside the same thread: the
command `ThreadCommand::SwitchProvider` (`switchProvider {key, engine,
model?}`; `THREAD_COMMAND_KINDS` 40), `ThreadView.chain` (the provider
sessions a switched thread ran on, head last) and
`ThreadControlResponse.handoff` (`sent`, `unsent` or `unknown`; `prompt`
carries an ACP session's launch handoff). `continueWith` stays on the wire
and served. No channel or push is added or removed: request and push counts
stay 124 and 12. The Threads store becomes `threads.v5.json` (it reads
`threads.v4.json` once and carries it over). Rust and TypeScript share
`electron/src/fixtures/threads-switch-v59.json`. Contract:
[T3-style systems, section 5](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v60 adds several accounts per provider: the request channel
`iface:providerAccounts` (TS `providerAccounts`), owned by the host's
provider accounts owner, with the commands `read`, `refresh {provider?}`,
`add {provider, label}`, `rename {provider, account, label}`, `setDefault
{provider, account}` and `remove {provider, account, deleteDirectory?}`,
each answering a version 1 `ProviderAccountsView` (Claude, Codex and Grok,
each with its Default account first). `read` runs no CLI; `refresh` asks each
account's CLI whether it is signed in, with that account's environment.
Optional fields: `LaunchOptions.account` (the account a new session runs on;
absent means the provider's default account), `EngineUsage.account` and
`accountLabel` (`readUsage` answers one row per added account after the
Default row), `CreateTerminalRequest.account` (only with `signIn`),
`ThreadView.account` (the added account a thread's session is bound to) and
`ThreadCommand::ContinueWith.account` (continue on the same provider's other
account). Request operations become 125 (desktop channels 137); pushes stay
12. Two new Rust stores, `provider-accounts.v1.json` and
`session-accounts.v1.json`; no existing store changes. Rust and TypeScript
share `electron/src/fixtures/provider-accounts-v60.json`. Contract:
[T3-style systems, section 6](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v61 adds visual replies: the engine event `visual.shown {id, title,
bytes}` (a page an agent showed through team tools' new `show_visual`), the
journal entry `visual {id, title, bytes}`, and the `iface:threadJournal`
command `visual {key, id}`, answered as `ThreadJournalResponse.visual {id,
html}` (absent when the thread no longer keeps it; replay-safe). Rust keeps
each page owner-private under `<data root>/thread-visuals/` (at most 64 pages
and 16 MiB a thread). No host channel or push is added or removed: request
and push counts stay 125 and 12; persisted schemas of existing stores and
terminal data frames are unchanged. Main adds its own renderer channel
`iface:visualFrame` and serves the wrapped page sandboxed on `iface-visual:`;
the app CSP's `frame-src` becomes `iface-visual:`. Rust and TypeScript share
`electron/src/fixtures/visual-replies-v61.json`. Contract:
[T3-style systems, section 8](superpowers/specs/2026-11-14-t3-systems-design.md).
Protocol v62 adds scheduled prompts and pull-request watch: the commands
`ThreadCommand::ScheduleSet` (`scheduleSet {key, id?, name?, prompt, when}`,
`when` one of `{kind: "once", at}`, `{kind: "interval", minutes}`, `{kind:
"calendar", calendar: {timezone, weekdays, hour, minute}}`),
`ScheduleDelete {id}`, `ScheduleEnable {id, enabled}` and `WatchSet {key,
enabled}` (`THREAD_COMMAND_KINDS` 44), and `ThreadView.schedules` (the
thread's schedules, soonest first, and its watch; absent when it has
neither). The `iface_team` server gains `schedule_prompt`, `list_schedules`
and `cancel_schedule` (twelve tools with v61's `show_visual`), and the
orchestration control socket (`host.sock`) the method
`thread.schedule.trigger` that `iface trigger` calls. No channel or push is
added or removed: request and push counts stay 125 and 12 (desktop channels
137). The new store is `thread-schedules.v1.json`. Rust and TypeScript
share `electron/src/fixtures/threads-schedules-v62.json`. Contract:
[T3-style systems, section 7](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v63 adds composer references and stash: the request channel `iface:draftStash` (TS
`draftStash`), owned by the thread host, with the commands `list {key}`,
`push {key, draft}`, `pop {key, id, current?}` and `delete {key, id}`, each
answering a version 1 `DraftStashView` (the thread's head key, its stashes
newest first, and `pop`'s draft). A `draft` is `{text, items}`, the items the
frozen `PromptContextDescriptor`s a draft holds (at most 16, 64 KiB of text);
`pop` with a non-empty `current` stashes it in the same commit. A `chat`
prompt-context item is now a thread reference: its `value` is a thread key,
and `validatePromptContext` answers it with Rust's reference block in
`safeContent` (title, key, the `read_thread` hint when the sending thread has
team tools, the journal's last prompt and answer, at most 4 KiB), or refuses
it (another project, the sender itself, a thread not listed, a sender that is
not a project thread). Request operations become 126 (desktop channels 138);
pushes stay 12. One new Rust store, `draft-stash.v1.json`; no existing store
changes. Rust and TypeScript share
`electron/src/fixtures/composer-references-v63.json`. Contract:
[T3-style systems, section 9](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v65 adds outside agents (`iface mcp`): `ThreadSettings.outsideAgents`
(`off`, `read` or `full`; absent means off, and only another level is written)
and `ThreadView.startedFromOutside` (the client name an outside agent's MCP
client declared, display only). Rust keeps the level in the team tools ledger,
never in the Threads store, which `setSettings` keeps writing unchanged. The
ledger moves to `thread-team.v2.json` (version 2: the level and each started
thread's `startedBy {kind: outside, client}`), reading `thread-team.v1.json`
once and leaving it unchanged. No host channel or push is added or removed:
request and push counts stay 126 and 12 (desktop channels 138), as protocol
v63 left them. The host binds a separate owner-only
socket, `<data root>/team/outside.sock` (0700 directory, peer uid checked;
`/private/tmp/iface-out-<uid>-<8 hex>/s.sock` when that path is over 103
bytes), speaking only `hello` and `call`; it is not a desktop channel. Rust
and TypeScript share `electron/src/fixtures/outside-agents-v65.json`.
Contract: [T3-style systems, section 11](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v66 adds resume at reset: `RateLimitNotice.resetsAt` (on
`usage.updated`'s `rateLimited`; when the provider says when its usage limit
resets, in the usage windows' UTC ISO form), `ThreadView.limit {resetsAt?}`
(the thread's provider hit its limit since the thread last completed a turn),
`ThreadScheduleView.kind` (`resume`; absent for a prompt) and the command
`ThreadCommand::ResumeAtReset` (`resumeAtReset {key, prompt?}`,
`THREAD_COMMAND_KINDS` 45). No channel or push is added or removed: request
and push counts stay 126 and 12 (desktop channels 138). The scheduled prompts store becomes
`thread-schedules.v2.json` (it reads `thread-schedules.v1.json` once and
carries it over). Rust and TypeScript share
`electron/src/fixtures/threads-resume-v66.json`. Contract:
[T3-style systems, section 10](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v67 adds the quick wins: the commands `ThreadCommand::SuggestShipText`
(`suggestShipText {key, what: "commit"|"pullRequest", paths?}`, answered as
`ThreadControlResponse.shipText`), `SideAsk {key, question}` and `SideClose
{key}` (`THREAD_COMMAND_KINDS` 48 with v66's `resumeAtReset`); `ThreadView.secretRequest {id, name,
reason, agent}` and `ThreadView.side {key, question, askedAt}`; the request
channel `iface:threadSecret` (TS `threadSecret`), owned by the thread host,
with `list {}`, `give {key, id, value}`, `decline {key, id}` and `forget {key,
name}`, each answering a version 1 `ThreadSecretsView` of names (a value
crosses once, in `give`, and is never echoed); and team tools' thirteenth
tool, `request_secret {name, reason}`, answering `{path, note}` or
`{declined: true}`. Request operations become 127 (desktop channels 139);
pushes stay 12. New stores: `thread-sides.v1.json` and the owner-only
`thread-secrets/` folder; no existing store changes (`threads.v5.json`
stays). Rust and TypeScript share `electron/src/fixtures/quick-wins-v67.json`.
Contract: [T3-style systems, section 12](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v68 adds `SessionSummary.permissionMode` (optional): the permission
mode the host runs and resumes each session in, from `session-access.v1.json`
(`default` when it keeps none for the session). The host puts it on the
desktop's `iface:registry` pushes and `iface:listSessionSnapshot` rows for
sessions with permission controls outside the agent roster (not Chat, Voice
or an agent's session), and pushes the registry again when a remembered mode
changes. The renderer shows that mode before the mode saved for the model, and
never applies the saved mode before the first send of a session the host gave
a mode for, so a reloaded window or a thread the host started (a team helper)
shows its own mode and keeps it. The registry cache never stores the field;
no store, channel or count changes. Rust and TypeScript share
`electron/src/fixtures/session-mode-v68.json`.

Protocol v69 adds each switched thread's segment line totals:
`ThreadChainSegment.added?` and `removed?` (both or neither), the lines that
segment's own turns changed, summed over the turns that measured both counts,
so a switched thread's inbox row and pane head total every segment and match
the Changes panel's Whole thread. They are derived on read from the turns each
segment's record already keeps in `threads.v5.json`; no store changes, and a
segment recorded before v69 gets its totals the same way. No command, channel
or push is added: request and push counts stay 127 and 12 (desktop channels
139). Rust and TypeScript share `electron/src/fixtures/threads-totals-v69.json`.
Contract: [T3-style systems, section 5](superpowers/specs/2026-11-14-t3-systems-design.md).

Protocol v70 adds appearance surface v6 and workspace presentation version 2:
`{version: 2, terminalFontSize, pinnedWorkspaces}`, where the terminal text
size runs from 10 to 26 CSS pixels in half steps (default 11.5; version 1 is
whole sizes from 12). Electron negotiates surface v6, whose snapshots carry
version 2 and whose `iface:setAppearance` accepts version 2 only. Surfaces
v3–v5 are unchanged on the wire: they read the version 1 view (the next whole
size, never under 12) and write version 1, which Rust keeps as version 2. Rust
stores version 2 in the new `workspace-presentation.v2.json`; until that file
exists it reads what an earlier build saved in `workspace-presentation.v1.json`,
which it never writes again, so an older build keeps its own size and pins. No
channel or count changes. Rust and TypeScript share
`docs/fixtures/workspace-presentation-v2.json`.

Protocol v72 adds folder trust. Run headless, the Claude CLI asks no trust
question and loads a folder's own `.claude/settings*.json` and instruction
file at once, so a Claude session started or resumed in a folder nobody
trusted now runs with the user's own settings only (`--setting-sources=user`
and `--strict-mcp-config`); the registry keeps the sources the session asked
for, so a resume after a trust restores them. A folder is trusted when the
owner trusted its trust root (its repository's top folder; a linked worktree
counts for its main repository) in Interface, when Claude Code's own
`.claude.json` for the session's account records `hasTrustDialogAccepted` for
exactly that folder, when Interface owns it (an agent's home, the Chat
folder), or when New project made it. `ThreadView.folderTrust?` (`{folder,
asked}`) is joined at read for a Claude thread whose folder is not trusted.
Two `iface:threadControl` commands are added, so `THREAD_COMMAND_KINDS` goes
from 48 to 50: `folderTrust {folder}` (a read) and `trustFolder {folder,
trust}`, both answered with `ThreadControlResponse.folderTrust?` (`{folder,
state: "trusted" | "unasked" | "declined"}`); a path that is not absolute,
is over 4096 bytes, has control characters or does not exist is refused
`threads-folder-trust-refused`, definitely unsent. After a trust, the
folder's idle Claude threads end their provider process (a thread mid-turn
finishes it first) and their next message resumes them with the folder's
settings. No channel or push is added: request and push counts stay 127 and
12 (desktop channels 139). New store: `trusted-folders.v1.json` (`{version:
1, trusted, declined}`, at most 4096 entries each); an unreadable file is set
aside and nothing is trusted. No existing store changes. Rust and TypeScript
share `electron/src/fixtures/folder-trust-v72.json`.

Protocol v71 makes two agent permissions ask by default, each with an owner
switch. An agent teammate's web search asks (`webSearch`) unless its task
settings carry `allowWebSearch: true` ("Search the web without asking"); the
field is written only when on, so an `agent-work` file saved with it off is
unchanged, and absent reads as off. Threads settings gain
`paneToolsWithoutAsking`: off, only `read_pane` is pre-approved in Claude and
Codex panes, and `pass_off` and `send_to_pane` ask. Rust keeps that switch in
`threads.pane-tools-without-asking` beside the Threads store, never in
`threads.v5.json`, writes it on the wire only when on, and rebuilds the launch
catalog when it changes, so panes opened afterwards follow it. No channel or
count changes. Rust and TypeScript share `electron/src/fixtures/ask-first-v71.json`.
