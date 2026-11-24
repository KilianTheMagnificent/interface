# Inactive workspace terminals

Inactive workspaces automatically sleep once their terminals finish working.
Selecting the workspace resumes the same processes and PTYs; input from the
phone or another authorized source also wakes a terminal before writing bytes.
The active workspace stays awake. Hidden Code surfaces and hidden windows count
as inactive; temporary pane menus do not change workspace membership.

Sleep uses STOP/CONT on exact retained process identities. It reduces CPU work
and skips shell/provider metadata polling (a sleeping pane's provider poll checks
back every five seconds, the awake idle pace) while retaining memory, terminal output,
working directories, session bindings, generations, and workspace leases. Nothing
is relaunched, resent, or persisted as a fake exit. Closing a sleeping terminal
resumes it before the normal verified teardown.

After host loss, macOS startup can settle a captured stopped orphan without
resuming it. The owner-private capture must still match; the kernel must confirm
its exact PID, process group, terminal device, start time and UID, parent PID 1,
and a whole group of stopped/exited members. Ownership is rechecked before KILL,
and the group must be observably gone before its receipt is reconciled and
retired. Running groups, leaderless groups, changed identities and uncertain
observations remain untouched. The recovered provider ID is preserved for reopen;
no prompt or input is replayed. Normal shutdown closes panes before slower service
drains so suspended groups cannot be skipped while those drains wait.

Rust waits five seconds after evidence of idleness. If the process tree then
declines the suspension (a separately grouped background job, or a child that
is still running), Rust retries after five seconds and then waits twice as long
each time, up to a minute; new activity evidence (a turn or prompt that newly
ends), input or a visibility change starts that wait over. Plain output does
not, so a declining tree whose background job keeps logging is not re-probed
every second. Claude Stop hooks, Codex
completion notifications, OpenCode activity receipts, and exact current idle
Claude/Codex prompt observations can supply that evidence. A Claude Stop that
still lists a running background agent or workflow reports work instead, so
switching workspaces never suspends a workflow its turn left running. A shell's bracketed
paste prompt transition plus an empty child-job roster permits sleep; a quiet
shell command or a silent built-in loop does not. Foreground/background shell
jobs and separately grouped background agent jobs keep the terminal awake.
Unknown CLI activity, approvals, and shells without prompt markers stay awake
until reliable evidence is available. This is deliberately conservative because
the owner chose to let running work finish.

The renderer sends coalesced, ordered visibility facts for each generation.
Prompt observations are admitted only if their byte cursor matches the host and
xterm has drained its pending writes. Rapid switches cannot leave an older sleep
request applied after the latest wake. Protocol v24 versions the new operation;
existing saved workspace and terminal stream formats are unchanged.

Validation covers real process stop/resume and identity preservation, silent
foreground/background jobs, service generation/input/lease behavior, fragmented
and ordered completion signals, rapid switching, shared Rust/TypeScript payloads,
and switching away/back with real xterm replay in the renderer fixture.
