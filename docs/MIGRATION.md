# Contract-preserving migration

## Completion boundary

The migration rewrites the operational core in Rust while retaining the
Electron/React frontend. Rust owns canonical state, validation, provider and PTY
processes, worktrees, orchestration, collaboration, review, updates, recovery,
and CLI behavior. Electron main/preload owns only application lifecycle,
BrowserWindow integration, and transport adaptation. React owns presentation
and ephemeral interaction state. TypeScript must not remain as a second domain
implementation or a concurrent writer to canonical stores.

BridgeMind research informed the process boundary and later product direction,
but the strict port added no product behavior. That port is now complete. The
integrated terminal workspace and its launcher are deliberate post-parity
increments with separately versioned contracts; teammates, durable memory,
skills, scoped Places, routines, and plugin capabilities remain deferred.
Retired Interface teammate state from 2026-08-26 is not imported during the
port.

## Migration order

1. Freeze golden fixtures for shared DTOs, IPC, sockets, storage, provider
   streams, CLI output, and crash boundaries.
2. Port shared contracts and validators.
3. Port owner-private atomic storage, CAS, journals, locks, and recovery.
4. Port Codex JSON-RPC, generic ACP, direct Claude transport, and real PTY
   supervision.
5. Port orchestration, collaboration, worktrees, review, search, and WCP.
6. Port `iface` commands, control socket, and TUI.
7. Run a long-lived, bounded Rust host with a ready handshake, correlated
   requests, push events, backpressure, crash reporting, and restart rules.
8. Replace each Electron main-process service with a typed host operation while
   preserving the existing renderer API and event ordering.
9. Delete replaced TypeScript services after golden and end-to-end parity.
10. Package the Rust host inside the signed Electron app and prove update,
    rollback, tamper, cold-restart, and data-migration behavior.

## Non-negotiable contract and parity gates

- The current Electron/Rust control surface: protocol v6 with exactly 104
  renderer request operations and 10 push families, including Rust-owned
  terminal-launch profile discovery and adversarial-review orchestration.
- Exactly one canonical backend writer; renderer and preload never open domain
  stores directly.
- Provider readiness before registration; stable provider IDs; serialized
  lifecycle operations; approvals settled exactly once.
- Owner-only `0700` directories and `0600` files; symlink, hardlink, traversal,
  oversize, corruption, and write-uncertain refusal.
- One-writer election, CAS revisions, request idempotency, effect receipts, and
  unknown-outcome quarantine.
- Existing registry, worktree, project-plan, orchestration, collaboration,
  checkpoint, CLI, and appearance data read without lossy rewriting.
- Claude, Codex, Grok, Qwen, configurable ACP, and Gemini behavior.
- CLI text/JSON/exit-code and PTY parity at narrow and normal terminal sizes.
- Signed app/DMG/update artifacts, bundle identity, protected-folder behavior,
  cold restart, and actual running-byte replacement.
- No unversioned feature drift: the accepted source-Interface behavior remains
  frozen, while each post-parity increment must version its wire contract and
  earn new acceptance evidence.

## Updater transition

Electron remains the shipping shell and continues to consume `latest-mac.yml`
and the signed ZIP. The transition release adds the signed Rust host binary to
the application resources, teaches Electron to supervise it, and migrates one
operation family at a time. The installed acceptance gate proves that the
running app contains the expected Rust-host bytes, that host failure is visible
and recoverable, and that no legacy Node service writes the same store.
