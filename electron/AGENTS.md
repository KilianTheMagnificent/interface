# electron/

The retained shell. It renders state and translates frozen DTOs; it must not own
canonical business logic or stores. Appearance, updater policy, terminals,
orchestration and file access are all decided in Rust.

Module maps, entry points and the mechanism behind these rules:
`docs/areas/electron-main.md` (main, preload, shared, scripts) and
`docs/areas/renderer.md` (React). Read the one you are editing.

## Owns

`src/main/` the native window, the supervised `interface-host` child (versioned NDJSON
over stdio plus fd3/fd4 PTY pipes), and positional-argument → named-payload
translation. `src/shared/` the frozen contract surface compiled into both
bundles. `src/renderer/` React, presentational, with every testable decision
pushed into a plain `.ts` module. `scripts/` dev supervisor plus packaging,
signing, fuse and smoke gates. `src/engines/` does not exist — engine adapters
are entirely Rust.

## Dev instance

`npm run dev` (see `README.md`) runs beside the installed app with its own
profile: `--user-data-dir` and `INTERFACE_HOST_TEST_DATA_ROOT` both point at
`~/Library/Application Support/Iface Dev` (override `INTERFACE_DEV_PROFILE`),
while `INTERFACE_HOST_TEST_HOME` stays the real home so providers work. **Keep
that profile path short** — the collaboration broker's socket dies over 103
bytes and collaboration then reports itself unavailable. Renderer edits hot-swap
over Vite; `src/main/**` edits restart Electron once `tsc` emits into
  `dist-main`; a rebuilt checkout-local `interface-host` debug binary restarts
  it too. Never share Cargo target directories between worktrees.

## Do not break

- **The request and push surface is frozen** (current counts and protocol
  version: [protocol facts](../docs/PROTOCOL_FACTS.md)). Triple-asserted: `ipc-payload.ts` throws at module scope on a count,
  uniqueness, missing-encoder or extra-encoder mismatch — a startup crash, not a
  test failure; `rust-host-client.ts` rejects a ready frame not reporting
  the request/push constants (`RUST_HOST_REQUEST_OPERATIONS`); `scripts/smoke-integrated-terminal.mjs`
  checks again. Adding one means editing `shared/ipc.ts`, `ipc-payload.ts`,
  the `preload.cts` method that calls `ch("<key>")`, the `interface-contracts`
  channel table and a Rust host owner table together.
- **Never remove a retained channel.** `readFileSnapshot`, `saveFile`,
  `setWorkspaceOpen`, `searchProject`, `listReviewCheckpoints`,
  `advanceTerminalControls`, `discoverReviewChecks`, `runReviewChecks`,
  `forgetSession`, `getCollaborationStatus`, `voiceInterrupt`, `readFile`,
  `resumeSession`, `sendLive` and `listSessions` have no renderer caller and are
  wire protocol (why each is kept: `docs/CODEMAP.md`). Retired features'
  channels (`voiceControl`, `readTerminalDelegation`) stay too; the host
  answers them unavailable. They grep identically to
  genuinely dead helpers; check `crates/interface-contracts/` for a Rust owner
  before concluding anything.
- `preload.cts` cannot import at runtime. It builds each channel string as
  `iface:${key}`, the rule `shared/ipc.ts` and `main/bridge.ts` assert with
  `satisfies`, and its keys are typed against `typeof ChannelMap` /
  `typeof ExtraChannelMap`; its terminal handshake channels are typed against
  `terminal-bridge-controller.ts`. A rename there is a compile error here, not
  a silent gap.
- Provider effects stay fail-closed in the renderer: `OutboundLane.sendWithContext`
  (`src/renderer/outbound-lane.ts`) keeps definitely-unsent separate from
  unknown-outcome. An unknown must not resend, must not restore captured
  context, must not mark an echo undelivered. It decides from the host's typed
  error, which crosses to the renderer only as the message's v1 error-shape
  marker (`src/shared/ipc-error.ts`); a failed transmission without a proof of
  non-delivery is unknown. Display a caught IPC error through `ipcMessage` or
  `withoutIpcErrorShape`, never its raw `.message`. `markSteeringPromptInflight`
  persists **before** transmission. `outbound-lane.test.ts` pins each of these.
- `App.tsx` is the only production renderer file allowed to call `window.iface`.
  Components receive typed API props or emit callbacks; hooks take the api as
  a parameter. The one exception is `main.tsx`'s pre-mount `getAppearance()`;
  `window-iface-owner.test.ts` enforces both. The isolated DOM fixture
  calls the real preload explicitly to verify its transport boundary.
- Terminals: raw PTY bytes never cross the JSON protocol; the token handshake
  gates `createTerminal`/`restartTerminal`; panes are covered, never unmounted
  (unmounting disposes xterm and loses the buffer — the replay tail in
  `TerminalStreamRouter` is what survives a React remount, PTYs do not
  replay); routing identity is
  `terminalOutputStreamKey(id, generation)`, and `TerminalView` remounts per
  generation so handlers and buffers never cross a restart.
- Renderer IPC is authorized twice per call — sender identity and navigation
  allowlist — on the invoke bridge and both terminal handshake channels.
- Unknown never renders as zero: `sessionDiffStat`/`formatDiffStat` return
  null/`""`, usage prints "not reported", `recapLine` drops unprovable clauses.

## Gotchas

- **Several suites assert on component SOURCE TEXT.**
  `src/main/main-wiring.test.ts` matches ~30 exact substrings of `main.ts`
  including whitespace; `scripts/package-parity.test.mjs` does the same for
  `package.json` and `electron-builder.yml`;
  `src/renderer/components/Conversation.queue.test.ts`,
  `src/renderer/agent-canvas.test.ts` and `src/renderer/workspace-ui.test.ts` are
  the renderer ones most likely to bite, and ~two dozen other renderer tests use
  `readFileSync` too. Refactoring inside those files breaks tests by
  construction even when behavior is unchanged. Read the failing assertion before
  assuming a regression, and budget for updating it.
- **`scripts/smoke-packaged.mjs` drives the real renderer by class name and
  copy**, and every literal it asserts on is declared in its exported
  `RENDERER_PINS` table with the file that carries it;
  `scripts/smoke-pins.test.mjs` (in `npm run test:release`) reads each pinned
  file and fails in a second when a literal has moved or changed. The fast DOM
  lane covers selected interactions, not every packaged-smoke selector; a change to a
  pinned surface must update its pin in the same change — otherwise the pins
  test catches it, and if the pin is missing entirely the next
  `cargo xtask package-mac --smoke` fails thirteen minutes in on a stale probe.
- The renderer suite combines pure-function/static markup tests with
  `npm run test:renderer` (isolated Electron DOM, real preload/MessagePort/xterm).
  Exported helpers that look unused are often consumed only by their own
  `.test.ts` sibling; deleting one means deleting the test too.
- `src/renderer/contrast.ts` has no production consumer at all. It exists so
  `theme-accessibility.test.ts` and `chrome-mode.test.ts` can measure WCAG
  ratios.
- A throw inside a host-push listener becomes an `uncaughtException`: emit is
  synchronous and nothing on that path has a try/catch. Keep push code total.
- The terminal MessagePort is negotiated once per **document**, not per process.
  Failing the bridge with `closeTransport=true` closes the shared host pipes
  permanently for the process.
- Deliberately partial React dependency arrays exist on purpose (the settled-turn
  re-key effect, `composerDefaults`). Do not "fix" them for exhaustive-deps.
- Composer, Conversation and FormSelect load their CSS once through imports.
  Preserve cascade order and CLI/refined variants. `toolGlyph`'s
  `FILE_TOOLS`/`DIFF_TOOLS` in `tool-activity.ts` are a second tool vocabulary
  beside that module's `*_ACTIVITY` Sets (`isShellTool` aliases
  `isCommandActivity`), matched on the lower-cased name only. Edit both halves
  or neither.
- `scripts/handsfree.mjs` and the hook `scripts/handsfree-electron.cjs` are
  vendored: keep them byte-identical to `~/Developer/Projects/_AI/Tools/handsfree`
  (`cmp`). Usage: `docs/AGENT_HANDSFREE_TESTING.md`.

## Verify

```
cd electron
npx vitest run src/renderer/components/Conversation.queue.test.ts   # scoped
npx vitest run src/main                                             # main process
npm run check          # vitest + tsc + Electron DOM + build — the JS-side gate
cargo xtask check      # canonical gate (runs npm run check too)
```
