import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const source = readFileSync(fileURLToPath(new URL("./main.ts", import.meta.url)), "utf8");

describe("Rust desktop cutover wiring", () => {
  it("takes single-instance ownership before opening the service graph", () => {
    const lock = source.indexOf("app.requestSingleInstanceLock()");
    const ready = source.indexOf("app.whenReady().then(start)");
    expect(lock).toBeGreaterThan(-1);
    expect(ready).toBeGreaterThan(lock);
  });

  // A hands-free agent session must move userData before the lock: the owner's
  // profile lock would hand `second-instance` to the owner's window, which
  // reveals and focuses it. Its window is never shown, yet keeps painting.
  // A quiet Dev refresh (devLaunchStaysHidden) is decided before the window
  // exists, and only the owner switching to the app brings it up.
  it("keeps a quiet Dev refresh hidden until the app is activated", () => {
    const decided = source.indexOf("hiddenUntilActivated = devLaunchStaysHidden(appName, process.env);");
    expect(decided).toBeGreaterThan(-1);
    expect(source.indexOf("const win = createWindow(", decided)).toBeGreaterThan(decided);
    expect(source).toContain('app.on("did-become-active", () => { if (hiddenUntilActivated) revealWindow(); });');
  });

  it("enters hands-free mode before the lock and never shows its window", () => {
    const handsfree = source.indexOf("const handsfree = startHandsfree(here, app.isPackaged);");
    expect(handsfree).toBeGreaterThan(-1);
    expect(source.indexOf("app.requestSingleInstanceLock()")).toBeGreaterThan(handsfree);
    expect(source.indexOf("app.getPath(")).toBeGreaterThan(handsfree);
    const window = source.indexOf("new BrowserWindow({");
    expect(source.indexOf("show: !handsfree.enabled && !packagedSmoke && !hiddenUntilActivated,", window))
      .toBe(source.indexOf("show:", window));
    expect(source).toContain("if (handsfree.enabled || packagedSmoke) paintWhileHidden(win);");
    expect(source).toContain("...handsfree.hostEnvironment(process.env),");
    expect(source).toContain("new RustHostClient({ ...packagedSmokeHostCommand(packagedSmoke, command), detached: handsfree.enabled, env: {");
    const policy = source.indexOf("function applyPermissionPolicy(");
    const refuse = source.indexOf("if (handsfree.enabled || packagedSmoke) {\n      // A hands-free session reaches no microphone", policy);
    expect(refuse).toBeGreaterThan(policy);
    expect(source.indexOf('decide(ownDocument && (microphoneOnly || permission === "notifications"));')).toBeGreaterThan(refuse);
  });

  // Finder, the browser, Simulator, an external terminal, the microphone and a
  // LAN listener are all one click away in the UI; the guard runs before any
  // other bridge work. (The kit hook itself removes the default menu.)
  it("guards every hands-free renderer request before the host sees it", () => {
    const bridge = source.indexOf("installRustIpcBridge({");
    const before = source.indexOf("async beforeInvoke(operation, payload) {", bridge);
    const guard = source.indexOf("handsfree.guard(operation, payload);", before);
    expect(before).toBeGreaterThan(bridge);
    expect(guard).toBeGreaterThan(before);
    expect(source.indexOf("if (operation ===", before)).toBeGreaterThan(guard);
    expect(source.indexOf("return nativePayload(operation, payload);", before)).toBeGreaterThan(guard);
  });

  it("starts the Rust host, registers the full bridge and loads the renderer without waiting", () => {
    const start = source.indexOf("async function start()");
    const csp = source.indexOf("applyCsp(location);", start);
    const permissions = source.indexOf("applyPermissionPolicy(location);", csp);
    const failure = source.indexOf("host.onFailure", start);
    const ready = source.indexOf("const hostReady = host.start()", failure);
    const settled = source.indexOf("hostReady.catch(() => undefined);", ready);
    const bridge = source.indexOf("installRustIpcBridge({", ready);
    const before = source.indexOf("async beforeInvoke(operation, payload) {", bridge);
    const gated = source.indexOf("await hostReady;", before);
    const forwarded = source.indexOf("return nativePayload(operation, payload);", before);
    const load = source.indexOf("win.loadURL(", bridge);
    expect(start).toBeGreaterThan(-1);
    // The first document loads under the header CSP and permission policy.
    expect(csp).toBeGreaterThan(start);
    expect(permissions).toBeGreaterThan(csp);
    expect(load).toBeGreaterThan(permissions);
    expect(failure).toBeGreaterThan(start);
    expect(ready).toBeGreaterThan(failure);
    expect(settled).toBeGreaterThan(ready);
    expect(bridge).toBeGreaterThan(settled);
    // Every renderer request waits for the host (after the hands-free guard).
    expect(gated).toBeGreaterThan(before);
    expect(gated).toBeLessThan(forwarded);
    expect(load).toBeGreaterThan(bridge);
    // Nothing between starting the host and loading the renderer waits for it.
    expect(source.slice(ready, load)).not.toMatch(/^ {2}await /mu);
  });

  it("applies the persisted appearance once the host is ready, failing startup durably", () => {
    const load = source.indexOf("const loaded = win.loadURL(");
    const ready = source.indexOf("await hostReady;", load);
    const appearance = source.indexOf('"appearance service"', ready);
    const applied = source.indexOf("applyAppearance(appearanceState(", ready);
    const failure = source.indexOf("} catch (error) {", appearance);
    const report = source.indexOf("showStartupFailure(error);", failure);
    const destroy = source.indexOf("if (!win.isDestroyed()) win.destroy();", report);
    const shutdown = source.indexOf("beginShutdown();", destroy);
    const loaded = source.indexOf("await loaded;", shutdown);
    const handshakeDeadline = source.indexOf("terminalBridge.armRequestDeadline();", loaded);
    expect(load).toBeGreaterThan(-1);
    expect(ready).toBeGreaterThan(load);
    expect(applied).toBeGreaterThan(ready);
    expect(appearance).toBeGreaterThan(applied);
    expect(failure).toBeGreaterThan(appearance);
    expect(report).toBeGreaterThan(failure);
    expect(destroy).toBeGreaterThan(report);
    expect(shutdown).toBeGreaterThan(destroy);
    // The missing-handshake deadline never counts host startup.
    expect(loaded).toBeGreaterThan(shutdown);
    expect(handshakeDeadline).toBeGreaterThan(loaded);
  });

  // The handshake itself is behavior-tested in terminal-bridge-controller.test.ts.
  // main.ts gates terminal creation on it, installs it before the first
  // document can ask for its port, and removes it before the host stops.
  it("wires the terminal-port handshake around the first document and the host", () => {
    const start = source.indexOf("async function start()");
    const gate = source.indexOf("const terminalBridgeGate = new TerminalDataBridgeGate()", start);
    const requireReady = source.indexOf("terminalBridgeGate.requireReady()", gate);
    const controller = source.indexOf("const terminalBridge = createTerminalBridgeController({", requireReady);
    const shared = source.indexOf("gate: terminalBridgeGate,", controller);
    const disposer = source.indexOf("removeTerminalDataBridge = () => { terminalBridge.dispose(); };", shared);
    const load = source.indexOf("win.loadURL(", disposer);
    const arm = source.indexOf("terminalBridge.armRequestDeadline();", load);
    const ready = source.indexOf("await terminalBridge.ready;", arm);
    const cleanup = source.indexOf("removeTerminalDataBridge?.()", ready);
    const hostStop = source.indexOf("host?.stop(10_000)", cleanup);
    expect(gate).toBeGreaterThan(start);
    expect(requireReady).toBeGreaterThan(gate);
    expect(controller).toBeGreaterThan(requireReady);
    expect(shared).toBeGreaterThan(controller);
    expect(disposer).toBeGreaterThan(shared);
    expect(load).toBeGreaterThan(disposer);
    expect(arm).toBeGreaterThan(load);
    expect(ready).toBeGreaterThan(arm);
    expect(cleanup).toBeGreaterThan(ready);
    expect(hostStop).toBeGreaterThan(cleanup);
  });

  it("prunes terminal input retirements from authoritative lifecycle removal", () => {
    expect(source).toContain("function updateTerminalDataRetention");
    expect(source).toContain("transport.retainTerminal(terminal.id)");
    expect(source).toContain("transport.releaseTerminal(event.id)");
    expect(source).toContain("frame.operation === CHANNELS.terminalLifecycle");
  });

  it("shows only the startup failure when the backend fails before it has started", () => {
    const start = source.indexOf("async function start()");
    const declared = source.indexOf("let hostStarted = false;", start);
    const heldFailure = source.indexOf("let startupHostFailure: Error | null = null;", declared);
    const listener = source.indexOf("host.onFailure((error) => {", heldFailure);
    const held = source.indexOf("if (!hostStarted) {", listener);
    const recordFailure = source.indexOf("startupHostFailure ??= error;", held);
    const crash = source.indexOf('if (showCrashAlert("backend", error.message)) relaunchApp();', listener);
    const ready = source.indexOf("await hostReady;", crash);
    const failure = source.indexOf("showStartupFailure(error);", ready);
    const loaded = source.indexOf("await loaded;", failure);
    const bridgeReady = source.indexOf("await terminalBridge.ready;", loaded);
    const updaterReady = source.indexOf("await requireUpdaterConfiguration();", bridgeReady);
    const startupHostCheck = source.indexOf("if (startupHostFailure !== null) {", updaterReady);
    const started = source.indexOf("hostStarted = true;", startupHostCheck);
    const updaterStart = source.indexOf("updater.start();", started);
    expect(declared).toBeGreaterThan(start);
    expect(heldFailure).toBeGreaterThan(declared);
    expect(listener).toBeGreaterThan(heldFailure);
    expect(held).toBeGreaterThan(listener);
    expect(recordFailure).toBeGreaterThan(held);
    expect(crash).toBeGreaterThan(recordFailure);
    expect(failure).toBeGreaterThan(ready);
    // Started only once loaded, terminalBridge.ready, and updater configuration succeed.
    expect(loaded).toBeGreaterThan(failure);
    expect(bridgeReady).toBeGreaterThan(loaded);
    expect(updaterReady).toBeGreaterThan(bridgeReady);
    expect(startupHostCheck).toBeGreaterThan(updaterReady);
    expect(started).toBeGreaterThan(startupHostCheck);
    expect(started).toBeLessThan(updaterStart);
    expect(source.match(/hostStarted = true;/gu)).toHaveLength(1);
    expect(source).toContain("if (startupFailureShown) return false;\n  crashAlertShown = true;");
    expect(source).toContain("if (startupFailureShown || crashAlertShown) return;\n  startupFailureShown = true;");
  });

  it("destroys an already-created window and shuts down durably on top-level startup rejection", () => {
    const catchBlock = source.slice(
      source.indexOf("app.whenReady().then(start).catch((error: unknown) => {"),
      source.indexOf('app.on("window-all-closed"'),
    );
    const report = catchBlock.indexOf("if (!shutdownStarted) showStartupFailure(error);");
    const destroy = catchBlock.indexOf(
      "if (appWindow !== null && !appWindow.isDestroyed()) appWindow.destroy();",
      report,
    );
    const shutdown = catchBlock.indexOf("beginShutdown();", destroy);
    expect(report).toBeGreaterThan(-1);
    expect(destroy).toBeGreaterThan(report);
    expect(shutdown).toBeGreaterThan(destroy);
    expect(catchBlock).not.toContain("app.quit()");
  });

  it("makes an idle host failure visible without adding a renderer channel", () => {
    expect(source).toContain('if (showCrashAlert("backend", error.message)) relaunchApp();');
    expect(source).toContain("dialog.showErrorBox(alert.options.message, alert.options.detail ?? \"\");");
  });

  it("adds the renderer's waiting panes to the Dock badge (review D5)", () => {
    expect(source).toContain("ipcMain.on(DOCK_ATTENTION_CHANNEL, (event, payload: unknown) => {");
    const handler = source.slice(source.indexOf("ipcMain.on(DOCK_ATTENTION_CHANNEL"),
      source.indexOf("ipcMain.on(WORK_IN_FLIGHT_CHANNEL"));
    expect(handler).toContain("if (!isAuthorizedIpcSender(event, contents)");
    expect(handler).toContain("isAllowedRendererNavigation(event.senderFrame?.url ?? \"\", location)");
    expect(handler).toContain("const report = parseDockAttention(payload);");
    expect(handler).toContain("paintDockBadge();");
    expect(source).toContain("const next = dockBadgeCount(waitingSessions, dockAttention);");
    expect(source).toContain("if (frame.operation === CHANNELS.registry) paintDockBadge(frame.payload);");
  });

  it("relaunches through the ordinary quit after a crash alert (review D2)", () => {
    expect(source).toMatch(/function relaunchApp\(\): void \{\n  app\.relaunch\(\);\n  quitWithoutPrompt\(\);\n\}/u);
    expect(source).toContain('if (showCrashAlert("window", "")) app.relaunch();');
    // Harnesses that answer every dialog themselves never get a Relaunch button.
    expect(source).toContain("crashDialog(kind, appProductTitle, detail, !handsfree.enabled && !packagedSmoke)");
  });

  it("has no legacy TypeScript backend fallback", () => {
    expect(source).not.toContain("registerBridge(");
    expect(source).not.toContain("new ClaudeAdapter");
    expect(source).not.toContain("new CodexAdapter");
    expect(source).not.toContain("createOrchestrationDispatcher");
    expect(source).not.toContain("./file-provider.js");
  });

  it("asks Rust to resolve the native directory picker start path", () => {
    expect(source).toContain("host.invoke(NATIVE_DIRECTORY_PICKER_START_PATH");
    expect(source).toContain("preferredPath: defaultPath");
    expect(source).toContain("The Rust host returned an invalid directory picker start path.");
  });

  it("stops the host even when an earlier teardown step throws", () => {
    const prepare = source.indexOf("function prepareShutdown(): Promise<void> {");
    const steps = source.indexOf("runShutdownSteps([", prepare);
    const bridge = source.indexOf("() => { removeTerminalDataBridge?.(); },", steps);
    const updater = source.indexOf("() => { nativeUpdater?.dispose(); },", bridge);
    const hostStop = source.indexOf("host?.stop(10_000)", updater);
    expect(prepare).toBeGreaterThan(-1);
    expect(steps).toBeGreaterThan(prepare);
    expect(bridge).toBeGreaterThan(steps);
    expect(updater).toBeGreaterThan(bridge);
    expect(hostStop).toBeGreaterThan(updater);
  });

  it("retains the unsaved-work veto and durable host shutdown path", () => {
    expect(source).toContain('app.on("before-quit"');
    expect(source).toContain('win.webContents.on("will-prevent-unload"');
    expect(source).toContain("host?.stop(10_000)");
    expect(source).toContain('app.on("window-all-closed", () => { app.quit(); })');
  });

  it("backgrounds an ordinary macOS close without stopping agent event processing", () => {
    const coordinator = source.indexOf("createWindowLifecycleCoordinator({");
    const macBackground = source.indexOf(
      'shouldHideOnClose: () => backgroundMode.enabled', coordinator,
    );
    const hide = source.indexOf("hideWindow: () => { hideWindow(win); }", macBackground);
    const close = source.indexOf('win.on("close", (event) => {');
    const delegate = source.indexOf("lifecycle.onCloseRequested(event)", close);
    expect(coordinator).toBeGreaterThan(-1);
    expect(macBackground).toBeGreaterThan(coordinator);
    expect(hide).toBeGreaterThan(macBackground);
    expect(close).toBeGreaterThan(hide);
    expect(delegate).toBeGreaterThan(close);
    expect(source).not.toContain("win.minimize()");
    // Throttling follows native show/hide state; it is not permanently disabled
    // in BrowserWindow options. Hidden event batches still drain from a timer.
    expect(source).toContain("installWindowVisibility(win)");
    expect(source).toContain("createStuckRendererRecovery(win, {");
    expect(source).toContain("...MAC_WINDOW_MATERIAL");
    expect(source).not.toContain("backgroundThrottling");
  });

  it("reveals the same background window from the Dock or a second launch", () => {
    const reveal = source.indexOf("const revealWindow = (): void => {");
    const restore = source.indexOf("if (win.isMinimized()) win.restore()", reveal);
    const show = source.indexOf("win.show()", restore);
    const focus = source.indexOf("win.focus()", show);
    const sync = source.indexOf("syncWindowVisibility(win)", focus);
    const secondInstance = source.indexOf('app.on("second-instance", revealWindow)', focus);
    const activate = source.indexOf('app.on("activate", revealWindow)', secondInstance);
    expect(reveal).toBeGreaterThan(-1);
    expect(restore).toBeGreaterThan(reveal);
    expect(show).toBeGreaterThan(restore);
    expect(sync).toBeGreaterThan(focus);
    expect(secondInstance).toBeGreaterThan(sync);
    expect(focus).toBeGreaterThan(show);
    expect(secondInstance).toBeGreaterThan(focus);
    expect(activate).toBeGreaterThan(secondInstance);
  });

  it("sets the quit guard before closing a background window", () => {
    const beforeQuit = source.indexOf('app.on("before-quit", (event) => {');
    const setQuitting = source.indexOf("quitting = true", beforeQuit);
    const closeWindow = source.indexOf("win.close()", setQuitting);
    expect(beforeQuit).toBeGreaterThan(-1);
    expect(setQuitting).toBeGreaterThan(beforeQuit);
    expect(closeWindow).toBeGreaterThan(setQuitting);
  });

  it("installs the tested lifecycle coordinator before awaited startup work", () => {
    const installFunction = source.indexOf("function installWindowLifecycle");
    const coordinator = source.indexOf("createWindowLifecycleCoordinator({", installFunction);
    const destroyedFallback = source.indexOf(
      'win.webContents.on("destroyed"', coordinator,
    );
    const start = source.indexOf("async function start()");
    const install = source.indexOf("installWindowLifecycle(win)", start);
    const hostStart = source.indexOf("host.start()", install);
    expect(installFunction).toBeGreaterThan(-1);
    expect(coordinator).toBeGreaterThan(installFunction);
    expect(destroyedFallback).toBeGreaterThan(coordinator);
    expect(install).toBeGreaterThan(start);
    expect(hostStart).toBeGreaterThan(install);
  });

  it("keeps renderer failure on the same idempotent shutdown path", () => {
    const rendererGone = source.indexOf("onRendererGone() {");
    const destroy = source.indexOf("win.destroy()", rendererGone);
    const rendererShutdown = source.indexOf("beginShutdown()", destroy);
    expect(rendererGone).toBeGreaterThan(-1);
    expect(destroy).toBeGreaterThan(rendererGone);
    expect(rendererShutdown).toBeGreaterThan(destroy);
    expect(source).not.toContain("onTerminalRendererDestroyed");
  });

  it("creates the native window before waiting on Rust startup", () => {
    const start = source.indexOf("async function start()");
    const create = source.indexOf("const win = createWindow", start);
    const hostStart = source.indexOf("host.start()", start);
    expect(create).toBeGreaterThan(start);
    expect(hostStart).toBeGreaterThan(create);
  });

  // At module scope its ~160 modules evaluated before `ready`, so before the
  // first window; configure() waits for the load (native-updater.test.ts).
  it("loads electron-updater only after the window and the host have started", () => {
    const start = source.indexOf("async function start()");
    const create = source.indexOf("const win = createWindow", start);
    const hostStart = source.indexOf("host.start()", create);
    const load = source.indexOf('import("electron-updater")', hostStart);
    const effects = source.indexOf("new NativeUpdaterEffects({", hostStart);
    expect(hostStart).toBeGreaterThan(create);
    expect(effects).toBeGreaterThan(hostStart);
    expect(load).toBeGreaterThan(effects);
    expect(source).not.toMatch(/^import [^;]* from "electron-updater";$/mu);
  });

  it("bounds the startup update and appearance host calls", () => {
    expect(source).toContain('withStartupDeadline("update service", updater.configure({');
    expect(source).toContain('withStartupDeadline(\n      "appearance service"');
  });

  it("loads the renderer while updater configuration is pending and gates updater start", () => {
    const configure = source.indexOf(
      "const requireUpdaterConfiguration = createStartupGate(",
    );
    const configureCall = source.indexOf(
      'withStartupDeadline("update service", updater.configure({', configure,
    );
    const identityFence = source.indexOf("updaterIsActive", configureCall);
    const load = source.indexOf("win.loadURL(", identityFence);
    const terminalReady = source.indexOf("await terminalBridge.ready", load);
    const awaitConfiguration = source.indexOf("await requireUpdaterConfiguration()", terminalReady);
    const updaterStart = source.indexOf("updater.start()", awaitConfiguration);
    expect(configure).toBeGreaterThan(-1);
    expect(configureCall).toBeGreaterThan(configure);
    expect(identityFence).toBeGreaterThan(configureCall);
    expect(load).toBeGreaterThan(identityFence);
    expect(source.slice(identityFence, load))
      .not.toMatch(/^  await requireUpdaterConfiguration\(\);$/mu);
    expect(terminalReady).toBeGreaterThan(load);
    expect(awaitConfiguration).toBeGreaterThan(terminalReady);
    expect(updaterStart).toBeGreaterThan(awaitConfiguration);
  });

  it("rechecks for updates when the hidden window is focused again", () => {
    const updaterStart = source.indexOf("updater.start()");
    const focus = source.indexOf('win.on("focus"', updaterStart);
    const active = source.indexOf("if (!updaterIsActive()) return;", focus);
    const nudge = source.indexOf("void updater.checkOnForeground()", active);
    expect(updaterStart).toBeGreaterThan(-1);
    expect(focus).toBeGreaterThan(updaterStart);
    expect(active).toBeGreaterThan(focus);
    expect(nudge).toBeGreaterThan(active);
  });

  it("watches the local update feed so a staged release is checked without Settings", () => {
    const updaterStart = source.indexOf("updater.start()");
    const watch = source.indexOf("updater.watchLocalFeed(", updaterStart);
    const feed = source.indexOf('join(app.getPath("userData"), "update-feed")', watch);
    expect(updaterStart).toBeGreaterThan(-1);
    expect(watch).toBeGreaterThan(updaterStart);
    expect(feed).toBeGreaterThan(watch);
  });

  it("gates every public updater operation before invoking Rust", () => {
    const beforeInvoke = source.indexOf("async beforeInvoke(operation, payload)");
    const getState = source.indexOf("operation === CHANNELS.getUpdateState", beforeInvoke);
    const check = source.indexOf("operation === CHANNELS.checkForUpdates", getState);
    const restart = source.indexOf("operation === CHANNELS.restartToUpdate", check);
    const configuration = source.indexOf("await requireUpdaterConfiguration()", restart);
    const nativePayload = source.indexOf("return nativePayload(operation, payload)", configuration);
    const afterInvoke = source.indexOf("async afterInvoke(operation, result)", nativePayload);
    expect(beforeInvoke).toBeGreaterThan(-1);
    expect(getState).toBeGreaterThan(beforeInvoke);
    expect(check).toBeGreaterThan(getState);
    expect(restart).toBeGreaterThan(check);
    expect(configuration).toBeGreaterThan(restart);
    expect(nativePayload).toBeGreaterThan(configuration);
    expect(afterInvoke).toBeGreaterThan(nativePayload);
  });

  it("turns a deferred updater configuration failure into durable shutdown", () => {
    const terminalReady = source.indexOf("await terminalBridge.ready");
    const finalConfiguration = source.indexOf(
      "await requireUpdaterConfiguration()", terminalReady,
    );
    const failure = source.indexOf("} catch (error) {", finalConfiguration);
    const activeFence = source.indexOf("if (!updaterIsActive()) return;", failure);
    const report = source.indexOf("showStartupFailure(error)", activeFence);
    const destroy = source.indexOf("win.destroy()", report);
    const shutdown = source.indexOf("beginShutdown()", destroy);
    const done = source.indexOf("return;", shutdown);
    const updaterStart = source.indexOf("updater.start()", done);
    expect(terminalReady).toBeGreaterThan(-1);
    expect(finalConfiguration).toBeGreaterThan(terminalReady);
    expect(failure).toBeGreaterThan(finalConfiguration);
    expect(activeFence).toBeGreaterThan(failure);
    expect(report).toBeGreaterThan(activeFence);
    expect(destroy).toBeGreaterThan(report);
    expect(shutdown).toBeGreaterThan(destroy);
    expect(done).toBeGreaterThan(shutdown);
    expect(updaterStart).toBeGreaterThan(done);
    expect(source.slice(failure, done)).not.toContain("app.quit()");
  });

  it("re-fences native updater effects after the Rust round trip", () => {
    const afterInvoke = source.indexOf("async afterInvoke(operation, result)");
    const check = source.indexOf("if (operation === CHANNELS.checkForUpdates)", afterInvoke);
    const checkFence = source.indexOf("if (!updaterIsActive()) return undefined;", check);
    const nativeCheck = source.indexOf("await updater.check()", checkFence);
    const postCheckFence = source.indexOf("if (!updaterIsActive()) return undefined;", nativeCheck);
    const restart = source.indexOf(
      "if (operation === CHANNELS.restartToUpdate && result === true)", postCheckFence,
    );
    const installFence = source.indexOf(
      "if (updateInstallRequested || !updaterIsActive()) return false;", restart,
    );
    const install = source.indexOf("await updater.install()", installFence);
    expect(afterInvoke).toBeGreaterThan(-1);
    expect(check).toBeGreaterThan(afterInvoke);
    expect(checkFence).toBeGreaterThan(check);
    expect(nativeCheck).toBeGreaterThan(checkFence);
    expect(postCheckFence).toBeGreaterThan(nativeCheck);
    expect(restart).toBeGreaterThan(postCheckFence);
    expect(installFence).toBeGreaterThan(restart);
    expect(install).toBeGreaterThan(installFence);
  });

  it("keeps the visible app alive until the native updater is ready to quit", () => {
    const restart = source.indexOf("operation === CHANNELS.restartToUpdate");
    const requested = source.indexOf("updateInstallRequested = true", restart);
    const installer = source.indexOf("await updater.install()", requested);
    const nativeReady = source.indexOf(
      'electronAutoUpdater.on("before-quit-for-update"', installer,
    );
    const discard = source.indexOf("discardUnsavedForUpdate = true", nativeReady);
    const quit = source.indexOf("quitting = true", discard);
    const shutdown = source.indexOf("function beginShutdown()", installer);
    expect(restart).toBeGreaterThan(-1);
    expect(requested).toBeGreaterThan(restart);
    expect(installer).toBeGreaterThan(requested);
    expect(shutdown).toBeGreaterThan(installer);
    expect(nativeReady).toBeGreaterThan(shutdown);
    expect(discard).toBeGreaterThan(nativeReady);
    expect(quit).toBeGreaterThan(discard);
    expect(source).not.toContain("closeWindowForUpdate");
    expect(source).not.toContain("updateQuitFallback");
  });

  it("only bypasses dirty-editor unload after native update readiness", () => {
    const unload = source.indexOf('win.webContents.on("will-prevent-unload"');
    const bypass = source.indexOf("if (discardUnsavedForUpdate)", unload);
    const nativeReady = source.indexOf(
      'electronAutoUpdater.on("before-quit-for-update"', bypass,
    );
    const enable = source.indexOf("discardUnsavedForUpdate = true", nativeReady);
    const nativeError = source.indexOf('electronAutoUpdater.on("error"', enable);
    const restore = source.indexOf("resetPendingUpdateAfterFailure()", nativeError);
    expect(unload).toBeGreaterThan(-1);
    expect(bypass).toBeGreaterThan(unload);
    expect(nativeReady).toBeGreaterThan(bypass);
    expect(enable).toBeGreaterThan(nativeReady);
    expect(nativeError).toBeGreaterThan(enable);
    expect(restore).toBeGreaterThan(nativeError);
    expect(source).toContain('autoUpdater.on("error", onUpdateFailure)');
    // The listener stays for the process lifetime: MacUpdater re-emits native
    // Squirrel errors as 'error', and an unlistened 'error' throws.
    expect(source).not.toContain('autoUpdater.removeListener("error", onUpdateFailure)');
  });

  // Electron's default uncaughtException handler is a modal error box that keeps
  // the process alive until dismissed, which stalls the Squirrel install of an
  // update. During shutdown the failure is recorded to disk instead.
  it("records rather than modally reports a main-process error during shutdown", () => {
    expect(source).toContain('process.on("uncaughtException"');
    expect(source).toContain('process.on("unhandledRejection"');
    expect(source).toContain("if (phase === \"shutdown\") return;");
    expect(source).toContain("recordMainProcessError(");
  });

  // Without a handler a SIGTERM ends Electron on the spot: the Rust host is
  // never stopped and keeps the profile's locks, and the renderer's last
  // debounced writes are lost. The signals must take the ordinary quit path.
  it("turns a termination signal into the same graceful quit the menu uses", () => {
    expect(source).toContain('for (const signal of ["SIGTERM", "SIGINT", "SIGHUP"] as const) {');
    expect(source).toContain("process.on(signal, () => { quitWithoutPrompt(); });");
    const quit = source.slice(source.indexOf("function quitWithoutPrompt(): void {"));
    expect(quit.indexOf("quitWithoutAsking = true;")).toBeGreaterThan(-1);
    expect(quit.indexOf("quitWithoutAsking = true;")).toBeLessThan(quit.indexOf("app.quit();"));
  });

  // While agents work the Mac stays out of idle sleep (review D3, 2026-10-14):
  // one blocker, from the same report the quit prompt reads, released on
  // idle, when the host dies and at shutdown; never in a session with nobody
  // at the Mac.
  it("keeps the Mac awake from the work-in-flight report and releases it on quit", () => {
    expect(source).toContain(
      "const keepAwake = handsfree.enabled || packagedSmoke ? NO_KEEP_AWAKE : createKeepAwake(powerSaveBlocker);");
    expect(source).not.toContain("prevent-display-sleep");
    expect(source).toContain("    workInFlight = null;\n    keepAwake.update(null);");
    const shutdown = source.slice(source.indexOf("function prepareShutdown(): Promise<void> {"));
    expect(shutdown.slice(0, shutdown.indexOf("runShutdownSteps("))).toContain("keepAwake.release();");
  });

  // Quitting with agents mid-turn asks first (owner review A2, 2026-10-14).
  // The prompt is asynchronous so a dev-supervisor SIGTERM still quits while
  // it is up, and nobody is asked in a hands-free or packaged-smoke session.
  it("asks before a quit would end working agents", () => {
    const quit = source.slice(source.indexOf('app.on("before-quit", (event) => {'));
    expect(quit).toContain("    if (!quitting && askBeforeQuit()) return;\n    quitting = true;");
    const ask = source.slice(source.indexOf("function askBeforeQuit(): boolean {"),
      source.indexOf("function quitWithoutPrompt(): void {"));
    expect(ask).toContain("if (quitWithoutAsking || handsfree.enabled || packagedSmoke");
    expect(ask).toContain("const prompt = quitPrompt(workInFlight);");
    expect(ask).toContain("void dialog.showMessageBox(win, {");
    expect(ask).not.toContain("showMessageBoxSync");
    expect(ask).toContain("signal: abort.signal,");
    expect(source).toContain("ipcMain.on(WORK_IN_FLIGHT_CHANNEL, (event, payload: unknown) => {");
    expect(source).toContain("    workInFlight = work;\n    keepAwake.update(work);");
    expect(source).toContain("if (!isAuthorizedIpcSender(event, contents)");
  });

  // Electron routes every web Notification the renderer displays through the
  // permission request handler, not only Notification.requestPermission(). A
  // handler that refuses "notifications" drops the agent-finished and
  // needs-input notifications silently while Notification.permission still
  // reads "granted" in the page.
  it("admits the renderer's own notifications through the permission handler", () => {
    const policy = source.indexOf("function applyPermissionPolicy(");
    const decide = source.indexOf(
      'decide(ownDocument && (microphoneOnly || permission === "notifications"));', policy);
    expect(policy).toBeGreaterThan(-1);
    expect(decide).toBeGreaterThan(policy);
  });
});
