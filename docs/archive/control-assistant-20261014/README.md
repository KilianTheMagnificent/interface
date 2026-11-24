# Control assistant archive — October 14, 2026

The owner reviewed the October 14 opportunity review
(https://claude.ai/artifact/V6oyGmCTdQ7mMLUVUkvUTd, item F3) and chose "Archive
all three" dormant features. The text Control assistant (desktop "··· → Control
assistant", the phone's control desk) is retired from the active product. This
directory is historical material, not an active specification or a backlog
authorizing more implementation. Re-enabling it requires a new owner instruction.

## Why

Text control landed on Dev on 2026-09-14 as a rehearsal for voice-driven agent
orchestration. VC-01 to VC-10 were not started in the following 30 days, the
panel still said "Audio disconnected", and VC-07 was a latent cap: the store
refused new runs after 64 retained assignments. Two microphone-shaped assistants
with different safety models also confused the product (review C2).

## Source preserved

The local branch **`archive/control-assistant-20261014`** pins the complete
pre-retirement Dev tree at **`78dc388d4c2a7f301a10fac7b5d68afb7292b915`**. The
same commit and the earlier landings stay reachable through Dev history
(implementation `ff818ac1`, landing `732519a`). The archive branch is local.

Key historical source paths:

- `crates/interface-collaboration/src/voice_control.rs`: the durable control-desk
  store, grants, receipts, approvals and retention (plus golden files
  `tests/golden/control.{admitted,recovered}.json`).
- `apps/interface-host/src/voice/control.rs`: the host adapter, coordinator
  launch, follow-ups and Stop; `voice/mcp.rs` `BoundControl` and the
  `control_status`/`control_action` tools.
- `crates/interface-terminal/src/control_input.rs`: autonomous text entry at a
  verified empty Claude/Codex prompt; `apps/interface-host/src/terminal/io.rs`
  `enqueue_control_input`.
- `electron/src/renderer/components/ControlDesk.{tsx,css}`,
  `fixtures/control-desk-fixture.tsx` and the `control-desk` renderer lane; the
  `pane_screen` UI call in `use-voice-assistant.ts`.
- `apps/interface-ios/InterfaceApp.swift` `PhoneControlDesk` and its tests in
  `MacAddressTests.swift`.

```sh
git show archive/control-assistant-20261014:crates/interface-collaboration/src/voice_control.rs
```

Any future implementation should start from then-current Dev and port reviewed
pieces selectively, not reset to the snapshot (it contains unrelated work).

## Filed records

- [Control assistant: text rehearsal](VOICE_CONTROL.md), the design and
  verification record, unchanged apart from its archive note.
- The "Voice control and agent orchestration" to-do entry (VC-01 to VC-10) moved
  verbatim to [TODO-done.md](../../TODO-done.md).

## Retirement boundaries

- **Wire protocol kept.** `iface:voiceControl` (protocol v21) stays in the
  channel table and the preload. The host still decodes the v1 request (a
  malformed one is `invalid-payload`) and refuses every well-formed one with
  `voice-control-unavailable` ("The Control assistant is no longer available."),
  retryable false, replay-safe true: nothing happens. The phone route
  `POST /api/v1/voice-control` stays served for phones built before the
  retirement: it still requires a current phone grant, then gives the same
  refusal. The v1 DTOs (`interface-contracts/src/voice_control.rs`,
  `electron/src/shared/voice-control.ts`) and the shared
  `voice-control-v21.json` fixture remain, with their round-trip tests.
- **Your data stays on disk, unread.** The Mac no longer opens, reads or writes
  the control store: `voice/control/control.v1.json` and `voice/control/control.lock`
  under the app data folder (`~/Library/Application Support/Interface/` for the
  installed app, `~/Library/Application Support/Iface Dev/` for Dev). Nothing
  deletes them. Coordinator sessions are ordinary Claude sessions titled
  "Control assistant"; they stay in the session list and their transcripts in
  `~/.claude/projects/`. The phone's pending-delivery marker
  (`interface.control.pending.v1.<Mac origin>` in the phone app's preferences)
  is no longer read. The desktop marker lived in window session storage only.
- **The Voice assistant is unchanged.** The toolbar microphone, its helper
  (`apps/interface-voice`, `crates/interface-voice`), the `iface:voice*`
  channels, the `iface_voice` MCP server and its 16 tools stay. What Control
  owned inside Voice's code was removed: the two control tools, the per-session
  `BoundControl` gate and the coordinator half of the voice prompt
  (`engine-claude` `VOICE_PROMPT_APPEND`, back to voice-only wording), and
  `control_status` left `VOICE_READ_ONLY_TOOLS`. `list_profiles`, added with
  Control, stays a voice tool. Voice never resumes a Voice-kind session titled
  "Control assistant" as its own session (it used the control store for that
  before); a renamed coordinator could be picked up, and would then run as an
  ordinary voice session whose acting tools still ask first.
- `screen.rs`'s prompt recognizer stays: workspace sleep uses it.
  `TerminalStreamRouter.controlScreen` stays as a tested helper beside
  `screenEvidence`.
- `valid_id` moved from `voice_control.rs` to the `interface-collaboration`
  crate root; project Away and the workers broker use it.
