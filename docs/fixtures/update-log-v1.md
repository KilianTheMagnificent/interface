# Interface Update Log

Complete history of production releases (`v0.3.x`) and Dev landings, including notes on what each update is, how it is verified, and when it was completed. Maintained automatically by `cargo xtask closeout`, `cargo xtask promote`, and `cargo xtask update-log`.

- **Production releases:** 3 (`v0.3.164` – `v0.3.166`)
- **Dev landings:** 3 (`1` unreleased on Dev)

## Unreleased on Dev

### Harden terminal pass-off

- **Completed:** 2026-10-21T14:48:47-05:00
- **Kind:** fix
- **Release:** Unreleased (on Dev)
- **Commit:** `1c2a9b5` (`fix/pass-off-review`)
- **Notes:** Passed-off panes launch for every profile and custom command
- **Commits:** fix(pass-off): quote custom shell commands; test(pass-off): cover profiles
- **Pins:** `apps/interface-host/src/pane_mcp.rs::pass_off`, `electron/src/renderer/pass-off.test.ts::custom`

## Production Releases

### v0.3.166 — 2026-10-21T09:17:39-05:00 (`8f11382`)

- **[feature]** `2222222` · 2026-10-21T08:52:32-05:00 · **Name Antigravity panes from the conversation title** (`feat/antigravity-pane-titles`)
  - Notes: Antigravity pane adopts its conversation title automatically
  - Pins: `crates/interface-terminal/src/antigravity_names.rs::title`
- **Left on Dev:** `1c2a9b5` Harden terminal pass-off

### v0.3.165 — 2026-10-20T18:00:00-05:00 (`1111111`)

- **[speed]** `3333333` · 2026-10-20T17:30:00-05:00 · **Cache terminal font metrics**
  - Notes: Startup terminal measurement runs once per font family

### v0.3.164 — 2026-10-19T12:00:00-05:00 (`4444444`)

- _Version bump only._

## All Dev Landings

### Harden terminal pass-off

- **Completed:** 2026-10-21T14:48:47-05:00
- **Kind:** fix
- **Release:** Unreleased (on Dev)
- **Commit:** `1c2a9b5` (`fix/pass-off-review`)
- **Notes:** Passed-off panes launch for every profile and custom command
- **Commits:** fix(pass-off): quote custom shell commands; test(pass-off): cover profiles
- **Pins:** `apps/interface-host/src/pane_mcp.rs::pass_off`, `electron/src/renderer/pass-off.test.ts::custom`

### Name Antigravity panes from the conversation title

- **Completed:** 2026-10-21T08:52:32-05:00
- **Kind:** feature
- **Release:** v0.3.166
- **Commit:** `2222222` (`feat/antigravity-pane-titles`)
- **Notes:** Antigravity pane adopts its conversation title automatically
- **Pins:** `crates/interface-terminal/src/antigravity_names.rs::title`

### Cache terminal font metrics

- **Completed:** 2026-10-20T17:30:00-05:00
- **Kind:** speed
- **Release:** v0.3.165
- **Commit:** `3333333` (`speed/font-metrics`)
- **Notes:** Startup terminal measurement runs once per font family
- **Supersedes:** `0000000` Earlier font cache

