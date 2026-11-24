# Update checkpoints archive — October 21, 2026

"Halt all and update" landed on Dev on 2026-10-20 (landing `f28fecf1`) and
shipped in 0.3.164. Restart to update asked every pane and thread it judged to
be working to save a handoff, waited for each acknowledgement, installed, then
sent one continuation to the same sessions after the restart.

On 2026-10-21 the owner pressed it and every open pane received the stop request
although none was running a turn. The cause: an agent pane counted as idle only
after reporting a finished turn in the current app session, so panes reopened
at launch were all "working"; Codex panes also never passed the process-tree
check, because Codex runs its MCP servers in their own process groups. The
owner chose to remove the feature rather than fix the detection. This directory
is historical material, not a specification or a backlog; bringing the feature
back requires a new owner instruction.

## Source preserved

The pre-removal Dev tree is `b44f0b14`. Key paths there:

- `crates/interface-updater/src/sessions.rs` — the barrier, prompts and
  restart recovery. `crates/interface-settings/src/update_sessions.rs` — the
  `update-sessions.v1.json` receipt store.
- `apps/interface-host/src/update_sessions.rs` (the port), `terminal/update.rs`,
  `provider_capture/muse_resume.rs`, and the update arms of `updater.rs`.
- `crates/interface-terminal/src/service/update.rs` (the acknowledgement
  scanner), `muse_resume.rs`, and the update fences in `service.rs`,
  `workspace_sleep.rs` and `embedded/workspace_sleep_process.rs`;
  `interface-runtime`'s `hold_for_update`.
- Renderer: `components/UpdateSessionsPanel.{tsx,css,test.ts}`,
  `fixtures/update-sessions-fixture.tsx`, and the polling in `use-updater.ts`.
- [The design record](UPDATE_SESSION_CHECKPOINTS.md) (was
  `docs/UPDATE_SESSION_CHECKPOINTS.md`).

## Retirement boundaries

- **Restart to update installs directly**, as before 0.3.164: the confirm
  dialog asks to save editor changes, then the app restarts. Panes and threads
  close with the normal shutdown and reopen through ordinary workspace
  restoration; nothing is sent to them.
- **Wire protocol kept.** `iface:updateSessions` stays in the channel table,
  the preload and the shared DTOs (`interface-contracts/src/update_sessions.rs`,
  `electron/src/shared/update-sessions.ts`, fixture `update-sessions-v52.json`).
  The host answers `read` with `{version:1, phase:"idle", sessions:[], detail:""}`
  and refuses `prepare`, `cancel` and `resume` with `update-checkpoint`, before
  any effect. `restartToUpdate` keeps its empty request and boolean response.
  No count or protocol number changes.
- **Kept, used elsewhere.** Antigravity and Muse native session capture,
  tagged `agy-`/`muse-` bindings (protocol 53), exact-session reopen argv and
  their activity hooks stay: ordinary pane restoration and the activity dots use
  them. The `interface-checkpoints` Antigravity plugin keeps its name.
- **Your data stays on disk, unread.** Nothing reads or writes
  `update-sessions.v1.json` or `.update-sessions.lock` under the app data folder
  any more, and nothing deletes them. A pane that saved a handoff during an
  earlier pause keeps it in its project; continue it by asking the model.
