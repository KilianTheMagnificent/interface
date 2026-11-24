# Terminal Pass Off — design

Date: 2026-08-31. Status: approved by the owner in chat (design + "same
worktree" amendment). Scope: the integrated terminal workspace only.

## What it does

From a terminal pane running an agent CLI, the pane-header `⋯` menu offers
**Pass off to…** with the same agent list as `+` (Claude, Codex, Grok, Gemini;
unavailable ones disabled with their reason; Shell excluded). Choosing one
splits a new pane off the source pane. The new agent starts already holding a
host-templated prompt that points at a host-written file containing the source
pane's terminal transcript, and it runs **in the source agent's live working
directory** — either the selected checkout or an explicitly chosen worktree.
The source pane keeps running; the user closes it when satisfied. Focus lands
on the new pane (existing `splitTerminalPane` behaviour).

## Why this shape

- Argv is host-owned (`docs/CODEMAP.md:633`); the renderer must not hold CLI
  strings (`TerminalLauncher.test.ts` pins this). So the prompt is appended by
  the host, never typed by the renderer — no input-injection race.
- `saveFile` cannot create files and is workspace-rooted, so the transcript
  file is host-written under `std::env::temp_dir()/Interface/handoffs/`, the
  same pattern as pasted images (`apps/interface-host/src/main.rs:235`).
- Every roster CLI accepts an initial prompt for an interactive session
  (verified on this machine): `claude [prompt]`, `codex [PROMPT]`,
  `grok [PROMPT]`, `gemini <query>`. Qwen would need `-i`; it is not in
  `PROFILE_ORDER` and is excluded from pass-off targets.
- The transcript is the rendered xterm text — what the user sees. Vendor
  session logs (richer; Interface knows the Claude/Grok/Gemini session ids)
  are a later upgrade, out of scope here.

## Wire contract (no new channel, no launch version bump)

New optional field on `CreateTerminalRequest` (both sides, `deny_unknown_fields`):

```rust
pub struct TerminalHandoffV1 {          // crates/interface-contracts/src/terminal.rs
    pub version: u8,                    // 1
    pub source_terminal_id: TerminalId, // UUID format, validated like other ids
    pub transcript: String,             // ≤ 512 KiB, NUL-free
    pub from_title: Option<String>,     // ≤ 200 bytes
}
// CreateTerminalRequest { …, pub handoff: Option<TerminalHandoffV1> }
```

TS mirror in `electron/src/shared/ipc.ts`. `TerminalLaunchRequestV1` stays at
version 1 — handoff is a transient creation input, not part of the launch
identity: it is **excluded from** `CreateFingerprint`, `TerminalSummary`, and
the persisted `TerminalWorkspaceTerminal` (a pass-off pane reopens like any
other pane: profile + session resume, no transcript replay). The 109-operation
count is unchanged; the payload encoder for `create_terminal` accepts the new
optional field (contract test both sides; frame limit 192 MiB is ample).

## Host behaviour (`crates/interface-terminal`, service `create`)

When `handoff` is present and the launch profile is one of Claude/Codex/Grok/
Gemini (else `TerminalError::HandoffUnsupportedProfile`):

1. **Source lookup**: the source terminal entry must exist and be Running
   (else `TerminalError::HandoffSourceUnavailable`).

   **Amended in implementation:** this lookup and the live-cwd read in step 2
   run *outside* `lifecycle_gate`, unlike the codex peer scan. Step 2 shells
   out to `lsof` with a ~2 s worst-case deadline, and holding the global
   lifecycle lock across it would stall every unrelated terminal create and
   restart on one wedged mount. `create` already runs on a blocking worker, so
   the wait costs only the request that asked for it.

   The TOCTOU this opens is benign: the source pane can exit between the check
   and the spawn, but nothing the pass-off carries goes stale. The transcript
   is a snapshot the renderer already captured, and it stays exactly as true
   of the work done. The resolved cwd was authorized against
   `ApprovedTerminalRoots` and is re-asserted by `assert_directory` inside
   `prepare` on both sides of the spawn, so a directory that moves or
   disappears fails the create rather than starting a pane somewhere
   unauthorized. The only visible consequence is a pass-off from a pane that
   has just died, which is what the user asked for a moment earlier.
2. **Live cwd**: read the source child's current working directory by
   shelling out to `/usr/sbin/lsof -a -p <pid> -d cwd -Fn` (macOS; pid is
   already retained by the process control, `embedded.rs:124`). The original
   `libc::proc_pidinfo` route is impossible — the workspace forbids
   `unsafe_code` (root Cargo.toml) — and the crate already spawns `kill`/`ps`
   for process facts, so `lsof` follows the established pattern (~100 ms per
   pass-off, user-initiated). On any failure, fall back to the source entry's
   recorded cwd.
3. **Authorization**: new `ApprovedTerminalRoots::authorize_pass_off(target)`
   beside `authorize` (`safety.rs`): same `snapshot_directory` safety checks
   (canonical, non-symlink, stable identity), but accepts a directory that is
   the same as **or inside** an approved root (`starts_with` on the canonical
   path). This is a deliberate, narrow widening: the path is host-derived from
   a live process Interface itself spawned — the renderer still cannot name an
   arbitrary cwd. The request's `cwd` field (the source pane's cwd, sent by
   the renderer as fallback) is used only when live-cwd resolution fails, and
   then through the existing exact `authorize`.
4. **Handoff file**: write `temp_dir()/Interface/handoffs/<new-terminal-id>.md`
   — a short header (from title/profile, cwd, timestamp, "rendered terminal
   transcript, newest last; may include TUI chrome") plus the transcript in a
   fenced block. Prune like pasted images: TTL 24 h, keep ≤ 64.
5. **Prompt argv**: after session binding, append one positional argument
   (host-templated, ≤ 4 KiB argv cap respected):
   *"You are taking over for another agent (<title>). Read <file> — its
   terminal transcript, newest last — then continue its work in this
   directory. Do not redo work the transcript shows as finished."*
6. **No implicit worktree launch** — integrated terminal launches never pass
   `--worktree` to a provider CLI. An ordinary pane stays in the checkout the
   user selected, and a pass-off stays in the source agent's live directory.
   Composer, Agent Canvas, and orchestration worktrees remain separate,
   explicit per-launch choices; there is no global worktree default.

## Renderer behaviour

- `TerminalViewHandle` gains `readLines(): string` (or `string[]`):
  generalizes the existing private `recentTerminalLines` — walk
  `buffer.normal` fully (scrollback is 5,000) and, when the alternate screen
  is active, append the visible alt-screen rows.
- New pure module `electron/src/renderer/pass-off.ts`: transcript trim
  (drop trailing blanks, keep the newest ≤ 512 KiB, note truncation) +
  target-profile eligibility (running source, non-shell target). Unit-tested.
- `IntegratedTerminalCenter`: new `passOff(paneId, profileId)` callback next
  to `split` (NOT between `kickRedraw` and `insertPaths` — the redraw
  integration test slices source there; and `split` itself must stay
  byte-identical including its `[activeCwd, createTerminal, report]` dep
  array). It reads the source handle from `handles.current`, builds the
  transcript, and calls the existing `createTerminal` with
  `{ cwd: source.cwd, title: profile.label, launch: {version:1, profileId},
  handoff, split: {paneId, orientation} }`. Threads through
  `TerminalWorkspaceCallbacks.onRequestPassOff` → `TerminalSplitWorkspace` →
  `TerminalPane`.
- `TerminalPaneMoreMenu`: gains a **Pass off to…** item under the splits that
  flips the same popover to a profile-list page (extract the profile-button
  list from `TerminalPaneNewMenu` into a shared presentational component;
  `placeOn` re-measures on page flip; Escape/back returns). Disabled when the
  active terminal is not running or profiles are unavailable.
- Errors surface through the existing `report` path.

## Ceilings (accepted, documented, not built)

- Fullscreen TUIs (Grok's default) leave little in scrollback → thin handoff.
  Upgrade: host reads the vendor's own session log by session id.
- No summarization pass; the raw rendered transcript rides along.
- macOS-only live-cwd (the app is packaged for macOS only); other platforms
  fall back to the pane cwd.

## Tests

- Rust: `safety.rs` authorize_pass_off (root itself, nested dir, outside,
  symlink); launch prompt-append per profile with exact argv; service create
  with handoff (file written with expected content shape, fingerprint and
  summary unchanged, source-missing and shell-target errors); host dispatcher
  round-trip with the new field; handoff prune.
- TS: `pass-off.test.ts` (trim/cap/eligibility); `TerminalPrimitives.test.ts`
  additions for the menu page and callback threading, respecting the existing
  pinned assertions (menu item counts, header order, `split` source text, no
  `"claude --"`/`"codex --"` strings in the center).
- Gate: `cargo xtask check`.
