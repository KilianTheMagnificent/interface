# electron/ — main process and shared contracts

Module map, entry points and mechanism for the main process, the preload and
the shared contract surface. Rules for editing here: `electron/AGENTS.md`.
Navigation: `docs/CODEMAP.md`.

**src/main/**

- `main.ts` — entry point: window creation and security options, quit/shutdown
  choreography, CSP header injection, Rust host boot, dock badge, appearance
  application, native updater orchestration, and the wiring of the terminal
  MessagePort handshake. The renderer loads while the host starts: bridge
  requests and terminal handshakes wait on `hostReady`, `main.tsx` holds its
  first render for `getAppearance`, and a failed start destroys the window
  before the durable shutdown. The renderer reports what is still working on
  the main-only `iface:workInFlight` channel (`shared/quit-guard.ts`, sender
  and navigation authorized like the bridge): `before-quit` asks before ending
  it (never for a signal, hands-free or packaged smoke), and `keep-awake.ts`
  holds one `prevent-app-suspension` blocker while any of it is working.
- `app-menu.ts` — the macOS application menu `main.ts` installs at startup in
  place of Electron's stock one: App (About, Settings… ⌘, shown but left to
  the renderer, Hide, Quit), the stock Edit and View roles, Window (with the
  stock File menu's Close ⌘W) and Help (Reveal Logs, Copy Diagnostics, both
  answered by `iface:diagnostics` in Rust; main copies the report). Settings…
  and a Help failure reach the renderer on the main-only `iface:menuCommand`
  (`shared/app-menu.ts`). The accelerator audit is [APP_MENU.md](../APP_MENU.md).
  Hands-free and packaged-smoke sessions keep no menu.
- `terminal-bridge-controller.ts` — the per-document terminal MessagePort
  handshake: authorizes the request and ack channels, waits out host start,
  mints the token, transfers the port, arms the two 5 s deadlines, and fails
  closed (sticky per document) on a malformed request, a lost port, a transport
  failure or a dead renderer. Behavior-tested with fake Electron objects in
  `terminal-bridge-controller.test.ts`.
- `preload.cts` — the sandboxed contextBridge surface: `ch(key)`, which builds
  each channel string as `iface:${key}` (the rule `shared/ipc.ts` and
  `bridge.ts` assert at compile time), the `invoke()` wrapper (strips
  Electron's rejection wrapper and keeps the error-shape marker), the terminal MessagePort client with
  its own frame validation and backlog, every `window.iface` method.
- `ipc-payload.ts` — per-channel encoders turning positional renderer arguments
  into named Rust payloads; owns `RENDERER_REQUEST_CHANNELS` and the module-scope
  invariants that pin the [frozen channel surface](../PROTOCOL_FACTS.md),
  including the push-set count.
- `rust-host-client.ts` — supervises the child: spawn with five stdio pipes,
  Versioned NDJSON parsing with size limits, ready/response/push dispatch
  (`RUST_HOST_PUSH_CHANNELS` is the one push set; `ipc-payload.ts` removes it
  from the request surface), bounded
  pending-request map, fail-closed terminal error, SIGTERM/SIGKILL escalation.
  Host stderr keeps at most 2,000 characters per line, forwarding only WARN/ERROR
  prefixes to the main log; an unterminated line cannot grow the retained buffer.
- `rust-ipc-bridge.ts` — registers `ipcMain.handle` for every frozen request
  channel with authorize/beforeInvoke/afterInvoke hooks, and fans pushes out.
  Electron passes a renderer only a rejection's text, so a Rust host error
  leaves as the v1 error shape (`shared/ipc-error.ts`): its code, retryable and
  replaySafe in one marker after the message. Main's own refusals stay untyped.
  `rust-ipc-bridge-error-shape.test.ts` drives the shape through the real
  preload into the renderer's send and reopen classifiers.
- `rust-host-path.ts` — resolves the packaged or debug host binary (absolute,
  regular file, no PATH lookup; env override only in dev).
- `terminal-data-transport.ts` — the binary terminal data layer: ITRM frame
  encode/decode, ordered per-stream sequence enforcement, bounded fair-share
  output queue, MessagePort attach/rewrite, EOF and generation retirement.
- `terminal-output-window.ts` — metadata-only, generation-scoped output ledger.
  Data protocol v2 ACKs release contiguous consumed output independently of input
  sequences; token handshake remains v1. See `docs/TERMINAL_DATA_V2.md`.
- `terminal-data-bridge.ts` — `TerminalDataBridgeGate`: the process-local token
  fence deciding when terminal creation is allowed.
- `native-updater.ts` — the electron-updater adapter plus `NativeUpdaterEffects`:
  feed configuration parsing, scheduled, window-focus, local-feed, and
  on-demand checks, `quitAndInstall`, and a serialized event tail into the
  Rust updater owner. `main.ts` imports electron-updater after creating the
  window and starting the host; `configure()` asks Rust at once and waits for
  that load, and a failed load fails startup. Only the renderer's own check
  rejects; timer, focus and feed checks settle, since the event tail already
  warned. A feed change during a check runs one more check after it.
- `native-payload.ts` — injects Electron-only observations and effects
  (`nativeTheme.shouldUseDarkColors`, the native directory picker) before Rust
  validates the payload; `main.ts`'s startup appearance read goes through it
  too, so the appearance surface version lives only here.
- `appearance-state.ts` — validates the host's appearance result (preference,
  resolved theme and each optional surface, from one table, including the v5
  theme-file list, Follow macOS pair and painted CSS) before `main.ts` applies
  it to the window. `main.ts` keeps the last state and, between host reads,
  resolves Follow macOS itself (`resolveAppearanceTheme`) for the window fill
  (`windowBackground`) and native theme source (`appearanceThemeSource`).
- `window-security.ts` — renderer location resolution, navigation allowlist,
  external-URL validation, IPC sender authorization, the CSP string (its one
  `frame-src` is `iface-visual:`).
- `visual-frames.ts` — visual replies' frames (T3 §8): the in-memory store of
  wrapped pages (48 documents, 48 MiB) served on `iface-visual://frame/<token>`
  with the visual CSP and sandbox as response headers (`protocol.handle` on
  the app session), `iface:visualFrame` (`register`/`release`, authorized as
  the bridge is), and the window's navigation guard
  (`rendererNavigationGuard`: a new subframe may load a visual; a frame
  showing one never navigates). The fence itself is described in
  `shared/visual-frame.ts`.
- `visual-network-fence.ts` — the window's network fence below CSP (review,
  15 Nov): WebRTC ignores CSP, so the app window's web contents get
  `disable_non_proxied_udp` and its session (`defaultSession`) a proxy that
  leads nowhere with `<-loopback>`, so a visual's script reaches no address
  over UDP or TCP. Nothing else in that session uses the network (the
  packaged page is `file:` with `connect-src 'self'`; the Browser pane and
  electron-updater have their own partitions); `npm run dev`'s Vite host is
  exempt. Applied in `start()` before the first document loads.
- `browser-pane.ts` — the one `<webview>` the window allows (`webviewTag`):
  `will-attach-webview` refuses any session but `persist:browser-pane` or a
  non-http(s) `src`, and pins the guest to no preload, no Node, sandboxed and
  context-isolated (plus Chromium's PDF viewer). Guests navigate only to
  http(s); another app's scheme asks first. Every decision that makes a page
  behave like a Chrome tab is a pure function here: `browserOpenAction` (new
  tab in front or behind, a sized `window.open` a popup window whose page is
  pinned by `browserPopupOptions` and whose window main builds itself through
  `createWindow` from `browserPopupWindowOptions`, never from the page's
  features, sized by `browserPopupBounds`; refuse), `BrowserActivation` (a tab or window only
  just after a press in the page, one per press, as Chrome's popup blocker),
  `browserPopupTitle` (a popup's title leads with its host),
  `browserContextMenu` (the right-click menu),
  `browserDownloadName`/`browserDownloadPath` (Chrome's naming, never
  overwriting), `quarantineBrowserDownload` (macOS's quarantine flag and
  "Where from" on every finished download before it is called saved),
  `browserPermissionRequest`/`browserPermissionCheck` and
  `BrowserGrants` (ask once per site and permission, in memory for the run;
  also a download the page starts without a press),
  `browserQuestion` (the sheet's words) and `browserExternalScheme`.
  `installBrowserPaneGuards(host, session, hooks)` applies them to every page
  and popup; its `BrowserPaneHooks` are the seams (Downloads folder, the
  owner's answer, the menu, opening another app, the push to the renderer,
  and for tests the quarantine writer, popup window and screen work area)
  that tests and the browser-chrome lane fill. `file:` is not served and the
  user agent drops Electron's token. The guest has no IPC: handlers accept only
  the app window's main frame, and main tells the renderer about pages on the
  main-only `iface:browserEvent` (`shared/browser-events.ts`: a strict parser,
  the page chords and Chrome's zoom steps). Point and fix needs nothing more
  from main: the embedder runs its picker with `executeJavaScript` and crops
  with `capturePage`, Electron's own embedder-owned guest calls.
- `browser-pane-native.ts` — the running app's `BrowserPaneHooks`: the
  owner's Downloads folder, the native Block / Allow sheet on the app window
  (Block the default),
  the native menu, `shell.openExternal`, the Dock's Downloads bounce, and
  `iface:browserEvent` to the app window. A hands-free or packaged-smoke run
  is quiet: every answer is Block, popups stay hidden, nothing opens outside
  and downloads go to a temp folder.
- `window-lifecycle.ts` — the close/quit coordinator (deferred retry,
  unload-cancel token).
- `dev-readiness.ts` — supervised Dev-only renderer/preload/Rust read probe,
  reporting a launch-bound observation to the Rust restart command.
- `startup-lifecycle.ts` — `withStartupDeadline`: bounds the startup host calls.
- `json.ts` — `plainObject`, the one narrowing every main-process boundary uses
  before reading a field off untrusted JSON. The preload keeps its own copy
  because it cannot import at runtime.
- `bridge.ts` — `EXTRA_CHANNELS` (adoptAll, sendLive, importProgress, worktree),
  their DTOs, and the declaration-merge that adds them to `IfaceApi`.
- `badge.ts` — `waitingCount`, deliberately identical to the renderer's attention
  count.
- `handsfree.ts` — agent test sessions (`HANDSFREE=1`,
  `docs/AGENT_HANDSFREE_TESTING.md`): loads the vendored kit hook before the
  single-instance lock, refuses packaged builds, and gives the host a session
  HOME/data root with owner-state variables removed and fake provider CLIs
  (override variables, plus a session bin of fakes that leads a system-only
  PATH and, through the session `.zprofile`, login-shell PATH). `guard`, called
  first in the bridge's `beforeInvoke`, refuses requests whose effects leave
  the session: `/usr/bin/open` launches in the host (workspace tools, external
  terminal, agent-home reveal), voice, and phone remote enable/pair.
  The host then runs in its own process group: the driver's `stop` signals
  Electron's group, and a host killed by that signal skips its stdin-EOF
  teardown, leaving slept (SIGSTOPped) workspace shells orphaned and stopped.

**src/shared/** — compiled into both bundles. The frozen contract surface.

- `ipc.ts` — `CHANNELS`, the `IfaceApi` contract, every wire DTO, history limit
  and paging rules.
- `engine.ts` — engine ids, `EngineEvent`/`SessionSummary`/`LaunchOptions`,
  `sessionKey`, the context-usage remaining/change helpers (normalization is
  Rust-side).
- `orchestration.ts` / `orchestration-control.ts` — the orchestration V1
  records, limits, role and policy constants the renderer reads, and the
  request and projected snapshot/page DTOs it exchanges with the Rust host.
- `provider-commands.ts` — slash-command parsing, matching and draft rendering.
- `prompt-images.ts` — prompt image limits and the `data:` preview-URL guard.
- `questions.ts` — agent question payload parsing and summarization.
- `appearance.ts` — theme list, per-theme window/label tables, theme resolution
  and CLI/workspace chrome pairing; theme-file ids, summaries and the Follow
  macOS pair (appearance surface v5).
- `threads.ts` — the Threads surface v1 mirror of `interface-contracts`
  `threads.rs` (`ThreadCommand`, `ThreadsView`, `ThreadControlResponse`);
  `threads.test.ts` checks the shared `threads-v34.json` and `threads-v36.json`
  fixtures against it (and `threads-hidden-v58.json` for `hiddenKeys`).
- `model-catalog.ts` — model key normalization, catalog lookup, effort ladders.
- `updater.ts` — the `UpdateState` wire type, its runtime guard, constructors.
- `ipc-error.ts` — the main → renderer error shape (v1): encode and split the
  trailing `[iface-error/1 code=… retryable=… replay-safe=…]` marker, and the
  host's three delivery-effect codes. The preload matches the same tail itself.

**scripts/** — `dev.mjs` (host build, Vite with an identity-checked readiness
endpoint, tsc watch, Electron, coordinated signal shutdown; its port probe,
voice-helper stamp and cargo lookup live in `dev-support.mjs`, over
`cargo-target.mjs`'s `findExecutable`, so tests can run them) plus `dev-app.mjs`
(macOS-only clone of stock Electron.app into `electron/.dev-app/Interface Dev.app`,
installed to `/Applications/Interface Dev.app` without touching Interface.app),
`make-icon.mjs` (icon rasterizing; requires Google Chrome and fails without it),
`sign-mac.cjs` (electron-builder custom macOS signer: hardened runtime and
Apple's secure timestamp on every file, entitlements from
`build/entitlements.mac.plist` for the app and `entitlements.mac.inherit.plist`
for nested Electron code, its own short list for each `Resources/bin`
executable, and on the app bundle alone an explicit designated requirement
pinned to the team read from the signing certificate's subject OU; there is no
untimestamped or unhardened build), `verify-fuses.cjs`,
`builder-require.cjs` (requires electron-builder's own packages —
`@electron/osx-sign`, `@electron/asar`, app-builder-lib files — through
app-builder-lib, since this package declares only electron-builder),
`smoke-packaged.mjs` (drives the signed app over CDP; every renderer literal it
asserts on is declared in its exported `RENDERER_PINS` table with the file that
carries it, and `smoke-pins.test.mjs` in `npm run test:release` proves each one
still exists; it checks what packaging can break — bridge, host, PTYs, labels,
the six-pane 3 × 2 grid and its 3 + 2 reflow, one focused tile — and no look
values: gutters, corners, head height, borders, shadows, the launcher's
columns, the model selector's width and the toolbar's centring are the renderer
DOM gate's, checked against the tile tokens in `workspace-hierarchy-fixture.tsx`
at the smoke's 1920px width, so a look change fails `cargo xtask check`, not
the promote (review F2, 2026-10-15)), `smoke-integrated-terminal.mjs`
(headless terminal lifecycle against the debug host; imports from `dist-main`).
`contract-parity.test.mjs` compares the Rust contracts with `src/shared`,
`src/main` and `src/remote` field by field and the channel tables by name (see
`docs/areas/contracts.md`).
`handsfree.mjs` and `handsfree-electron.cjs` are byte-identical copies of the
`_AI/Tools/handsfree` kit (driver and hook); `handsfree-app.mjs` builds and
starts a session (`npm run handsfree`) with Foundation's home in the session;
`handsfree-tui.mjs` (`npm run handsfree:tui`) runs `iface` the same way in a
private tmux server, fenced by `handsfree-session.mjs` (the fake-provider
variables and commands, system PATH and session profile, which
`handsfree.test.ts` holds equal to `handsfree.ts`'s);
`handsfree-fake-provider.sh` stands in for provider CLIs and hands Claude's
structured launches to `handsfree-fake-claude.mjs` (scripted turns, edits and
transcripts; `src/main/handsfree-fake-claude.test.ts`).
`check.mjs` runs `npm run check`: the parts in `check-parts.mjs` (unit tests,
renderer typecheck, then build followed by the renderer DOM lanes) side by side,
stopping the rest on the first failure (`INTERFACE_CHECK_SERIAL=1` runs them in
turn). `test-renderer-dom.mjs` runs the DOM lanes listed in `renderer-lanes.cjs`
(budgets, `INTERFACE_*_ONLY` switches, `INTERFACE_RENDERER_LANE=a,b`) through a
pool of three (`INTERFACE_RENDERER_POOL=<n>`; `INTERFACE_RENDERER_LANES_SERIAL=1`
runs them one at a time in listed order), kills a lane that outlives its budget,
bounds Vite's start, and on a pass deletes the lanes' Chromium profiles and
prunes older runs from `$TMPDIR`. `renderer-fixture-main.cjs` is one lane (its
`RUN` table), runs as an accessory app with a profile of its own, and its
native show/minimize cycle keeps the window at opacity 0.

**Config** — `vite.config.ts` (renderer build, vitest config, dev readiness
middleware), `tsconfig.json` (renderer typecheck), `tsconfig.main.json`
(`src/main` + `src/shared` → `dist-main`, NodeNext, tests excluded, incremental
with its `.tsbuildinfo` inside `dist-main`), `package.json`, whose `check` is
`node scripts/check.mjs` (see above) and whose `build`
removes `dist-main` before `tsc`, so a stale incremental output cannot survive a
full build. Its `dependencies` list only what `dist-main` imports
(electron-updater): electron-builder packs every one into `app.asar`, while
Vite already bundles the renderer's packages, which are devDependencies.
`scripts/package-parity.test.mjs` fails when the two drift.

**Entry points** — `main.ts`'s `app.whenReady().then(start)`; `preload.cts` at
module scope (it initiates the terminal MessagePort handshake);
`installRustIpcBridge`; `RustHostClient.start()`; `electron/scripts/dev.mjs`.

**Invariants** — the rules an editor must not break are in `electron/AGENTS.md`;
this is the mechanism behind them.

- The terminal token handshake (`terminal-bridge-controller.ts`): main mints a
  UUIDv4, transfers one MessagePort with it, and `requireReady()` refuses `iface:createTerminal`/
  `iface:restartTerminal` until the preload echoes that exact token. A late ack
  from a previous document must never make the current document look ready.
- Terminal sequence numbers are per (terminalId, generation, direction) and
  strictly monotone on both sides; the transport rewrites the renderer's
  per-document counter onto the continuous host-pipe counter so a reload does not
  restart the host's sequence.
- A full terminal-data queue is flow control, not a protocol failure.
  `FairTerminalFrameQueue.push` and `TerminalDataTransport.send` return `false`
  and the refused frame is discarded for that one stream, leaving its expected
  sequence unadvanced. Only a genuine protocol violation may reach `#fail`,
  which fences the pipes every terminal shares.
- The two authorization checks are `isAuthorizedIpcSender` (sender is the one
  webContents **and** senderFrame is its mainFrame) and
  `isAllowedRendererNavigation` on `senderFrame.url`.
- The window is single-instance. On macOS an ordinary red traffic-light close
  is prevented and hides the existing window without minimizing it, leaving its
  web contents and Rust host alive. `window-visibility.ts` disables Chromium
  throttling while the native window is shown and not minimized, so macOS
  occlusion cannot strand its frames. Hidden/minimized windows stay throttled;
  the renderer drains agent events through its hidden-document timer.
  `hideWindow` must allow throttling *before* `win.hide()`, or Chromium keeps
  the hidden page drawing. Because a shown page is never hidden, a wedged
  Chromium compositor (`cc::FrameSorter`: tracker bookkeeping grows until the
  renderer burns a full core) never clears by itself, so on macOS
  `createStuckRendererRecovery` hides and re-shows the focused window for a few
  milliseconds after ten minutes above 0.4 core while input is idle. Dock activation or a second launch
  restores that same window. Explicit Quit and updater restarts set `quitting`
  first and keep the dirty-editor veto plus durable `beginShutdown()` path.
  `destroyed` and `closed` still route to shutdown so a renderer crash cannot
  strand a headless main.
- Effects stay fail-closed: `RustHostClient` records one terminal error and
  rejects every pending and future request with it; `NativeUpdaterEffects`
  serializes events onto a single tail promise; `restartToUpdate` sets
  `updateInstallRequested` once, and the dirty-editor bypass is armed only by
  electron's `before-quit-for-update`.
- `dist-main/` and `dist/` are build output in the working tree but gitignored;
  `smoke-integrated-terminal.mjs` imports from `dist-main`, so
  `npm run smoke:terminal` builds the debug host and `dist-main` first.
- Every Dev supervisor startup also rebuilds `dist/` before launching Electron.
  Dock/Spotlight launches use that built renderer after the supervisor quits,
  so a Dev refresh must update both it and the live Vite renderer.

**Gotchas**

- Parts of `src/shared/` are ported-but-unused. Before assuming a shared helper is
  live, grep the whole repo (`electron/`, `crates/`, `apps/`, `xtask/`) and check
  for a Rust owner in `crates/interface-contracts/` before adding to one.
- Failing the terminal bridge with `closeTransport=true` closes the shared host
  pipes permanently for the process, so a later re-handshake fails with
  `terminal-data-closed` rather than recovering. `did-start-navigation` on the
  main frame resets the gate and tears down the port.
- Two independent EOF/retirement bookkeeping systems exist for terminals — one in
  the preload and one in the main transport. Input retirement is capped at 32
  terminal ids; main separately retains up to 1,024 EOF stream keys. They can
  disagree after eviction; main's copy is authoritative.
- `scripts/dev.mjs` restarts Electron only when a `dist-main` `.js` file is
  *newer than the running app* (`isNewerMainBuild`), not on every `fs.watch`
  event. macOS re-delivers a compile's FSEvents for as long as something reads
  the emitted files, and a restarted Electron reads all of `dist-main`, so an
  event-only trigger turns one `tsc` emit into an endless restart loop (observed:
  279 restarts in 45 minutes, no source file changed). Do not simplify the mtime
  comparison away.
- `scripts/dev.mjs` picks a fixed port (5199, `--strictPort`) and proves it
  reached the process it just spawned via the `/__interface_dev_ready__` endpoint
  that echoes the `--mode` run id. A plain HTTP 200 from a stale Vite is not
  proof; do not simplify the handshake away. On macOS it also clones stock
  `Electron.app` into gitignored `electron/.dev-app/Interface Dev.app` (Fold
  icon, `com.andreromero.interface.dev`) and copies it to
  `/Applications/Interface Dev.app` so the Dock is not generic Electron.
  A Dock/Spotlight launch has no argv, so the wrapper packs a loader into
  `default_app.asar` (the Electron stub exits if that file is missing) that
  sets the Iface Dev profile and imports `electron/dist-main/main/main.js`. The
  installer refuses `/Applications/Interface.app`. A stamp of electron version +
  icon hash + package version + electronRoot skips the clone and ad-hoc sign on
  later boots. A copy sealed inside the bundle
  (`Contents/Resources/interface-dev-stamp.json`) makes the supervisor reinstall
  the shared `/Applications` copy when another checkout installed its own
  since. Do not rename the helper Mach-Os or the main `Electron` stub in
  that wrapper — SIP `com.apple.provenance` xattrs make codesign refuse a
  renamed stock stub, and Electron finds helpers by path. Renaming is the
  packaged-app pipeline's job.
