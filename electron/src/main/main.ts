import { applyWindowMaterial, MAC_WINDOW_MATERIAL } from "./window-material.js";
import {
  STUCK_RENDERER_CHECK_MS, createStuckRendererRecovery, hideWindow, installWindowVisibility, paintWhileHidden,
  syncWindowVisibility,
} from "./window-visibility.js";
import { startHandsfree } from "./handsfree.js";
import { BROWSER_PANE_PARTITION, installBrowserPaneGuards } from "./browser-pane.js";
import { browserPaneNativeHooks } from "./browser-pane-native.js";
import { installVisualFrameProtocol, rendererNavigationGuard, VisualFrames } from "./visual-frames.js";
import { fenceWindowNetwork } from "./visual-network-fence.js";
import { parseVisualFrameRequest, VISUAL_FRAME_CHANNEL } from "../shared/visual-frame.js";
import { guardPackagedSmoke, packagedSmokeHostCommand, startPackagedSmoke } from "./packaged-smoke.js";
import { statSync } from "node:fs";
import { DEV_READINESS_PROBE, devLaunchStaysHidden, reportDevReadiness } from "./dev-readiness.js";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  app, autoUpdater as electronAutoUpdater, BrowserWindow, clipboard, dialog, ipcMain, Menu,
  powerSaveBlocker, MessageChannelMain, nativeTheme, powerMonitor, session, shell,
  type IpcMainInvokeEvent,
} from "electron";
import { appMenuTemplate } from "./app-menu.js";
import { MENU_COMMAND_CHANNEL, type MenuCommandV1 } from "../shared/app-menu.js";
import {
  SOLID_SURFACES, appearanceThemeSource, resolveAppearanceTheme, windowBackground,
  type AppearanceState,
} from "../shared/appearance.js";
import { appearanceState } from "./appearance-state.js";
import { CHANNELS } from "../shared/ipc.js";
import {
  parseWorkInFlight, quitPrompt, WORK_IN_FLIGHT_CHANNEL, type WorkInFlightV1,
} from "../shared/quit-guard.js";
import { waitingCount } from "./badge.js";
import {
  DOCK_ATTENTION_CHANNEL, dockBadgeCount, parseDockAttention, type DockAttentionV1,
} from "../shared/dock-attention.js";
import { chooseRelaunch, crashDialog, type CrashKind } from "./crash-dialog.js";
import { createKeepAwake, NO_KEEP_AWAKE } from "./keep-awake.js";
import { plainObject } from "./json.js";
import {
  MAIN_PROCESS_ERROR_LOG_NAME, recordMainProcessError, runShutdownSteps,
} from "./main-process-errors.js";
import {
  NATIVE_DIRECTORY_PICKER_START_PATH, createNativePayloadHook,
} from "./native-payload.js";
import {
  NativeUpdaterEffects, createNativeUpdaterPort,
} from "./native-updater.js";
import { APPLICATIONS_MOVE_QUESTION, offerApplicationsMove } from "./applications-folder.js";
import { RustHostClient } from "./rust-host-client.js";
import { resolveRustHostPath } from "./rust-host-path.js";
import { installRustIpcBridge } from "./rust-ipc-bridge.js";
import { createStartupGate, withStartupDeadline } from "./startup-lifecycle.js";
import { createTerminalBridgeController } from "./terminal-bridge-controller.js";
import { TerminalDataBridgeGate } from "./terminal-data-bridge.js";
import {
  contentSecurityPolicy, isAllowedRendererNavigation, isAuthorizedIpcSender,
  resolveRendererLocation, validatedExternalUrl, type RendererLocation,
} from "./window-security.js";
import {
  buildChannelArgument, productTitle, resolveBuildChannel,
} from "./build-channel.js";
import { createWindowLifecycleCoordinator } from "./window-lifecycle.js";

const here = dirname(fileURLToPath(import.meta.url));
// Agent test sessions only (HANDSFREE=1): a hidden, never-activating window on a
// throwaway profile. First, before any userData read or the single-instance lock.
const handsfree = startHandsfree(here, app.isPackaged);
const packagedSmoke = startPackagedSmoke();
let appProductTitle = "Interface";
let appWindow: BrowserWindow | null = null;
/** A quiet Dev refresh's window, not yet shown (see devLaunchStaysHidden). */
let hiddenUntilActivated = false;
let rustHost: RustHostClient | null = null;
let removeBridge: (() => void) | null = null;
let removeBadgeListener: (() => void) | null = null;
let removeHostFailureListener: (() => void) | null = null;
let removeTerminalDataBridge: (() => void) | null = null;
let nativeUpdater: NativeUpdaterEffects | null = null;
let appearance: AppearanceState = { preference: "system", resolved: "light" };
let quitting = false;
let shutdownStarted = false;
let shutdownComplete = false;
let shutdownTask: Promise<void> | null = null;
let startupFailureShown = false;
let crashAlertShown = false;
let updateInstallRequested = false;
let discardUnsavedForUpdate = false;
let dockBadge = -1;
let dockBadgeFrozen = false;
/** The renderer's last count of agents still working (shared/quit-guard.ts). */
let workInFlight: WorkInFlightV1 | null = null;
/** A quit nobody needs to be asked about: a signal, or a prompt already answered. */
let quitWithoutAsking = false;
/** Dismisses the quit prompt while it is up. */
let quitPromptAbort: AbortController | null = null;
/** Holds the Mac out of idle sleep while that count says something works.
 *  A hands-free or smoke session must not keep the owner's Mac awake. */
const keepAwake = handsfree.enabled || packagedSmoke ? NO_KEEP_AWAKE : createKeepAwake(powerSaveBlocker);

// Electron's default uncaughtException handler is a modal error box. During a
// quit or update restart that box keeps the process alive until dismissed,
// which stalls Squirrel's install; so during shutdown the error is recorded
// and the shutdown continues. Outside shutdown the box is still shown.
function mainProcessErrorLog(): string {
  try {
    return join(app.getPath("userData"), MAIN_PROCESS_ERROR_LOG_NAME);
  } catch {
    return join(tmpdir(), MAIN_PROCESS_ERROR_LOG_NAME);
  }
}

function handleMainProcessError(
  kind: "uncaughtException" | "unhandledRejection",
  error: unknown,
): void {
  const phase = shutdownStarted || quitting ? "shutdown" : "running";
  const detail = recordMainProcessError(mainProcessErrorLog(), kind, error, phase);
  console.error(`[main] ${kind} (${phase}):`, detail);
  if (phase === "shutdown") return;
  dialog.showErrorBox(
    "A JavaScript error occurred in the main process",
    `${kind === "uncaughtException" ? "Uncaught Exception" : "Unhandled Promise Rejection"}:\n${detail}`,
  );
}

process.on("uncaughtException", (error) => { handleMainProcessError("uncaughtException", error); });
process.on("unhandledRejection", (reason) => { handleMainProcessError("unhandledRejection", reason); });

function updateTerminalDataRetention(host: RustHostClient, payload: unknown): void {
  const event = plainObject(payload);
  const transport = host.terminalDataTransport;
  if (transport === undefined) return;
  if (event?.kind === "upsert") {
    const terminal = plainObject(event.terminal);
    if (typeof terminal?.id === "string") transport.retainTerminal(terminal.id);
  } else if (event?.kind === "removed" && typeof event.id === "string") {
    transport.releaseTerminal(event.id);
  }
}

function resetPendingUpdateAfterFailure(): void {
  if (!updateInstallRequested || shutdownStarted) return;
  updateInstallRequested = false;
  discardUnsavedForUpdate = false;
  quitting = false;
}

/**
 * The backend-stopped and window-stopped alert (crash-dialog.ts). True when
 * the owner chose Relaunch. A harness that answers dialogs itself gets the
 * plain error box, which it logs, and never a relaunch.
 */
function showCrashAlert(kind: CrashKind, detail: string): boolean {
  if (startupFailureShown) return false;
  crashAlertShown = true;
  const alert = crashDialog(kind, appProductTitle, detail, !handsfree.enabled && !packagedSmoke);
  if (alert.relaunch === undefined) {
    dialog.showErrorBox(alert.options.message, alert.options.detail ?? "");
    return false;
  }
  return chooseRelaunch(alert, dialog.showMessageBoxSync(alert.options));
}

/**
 * Quit the ordinary way — the renderer's last writes land and the host is
 * stopped — and have macOS start the app again once it has. The owner just
 * chose this, so the still-working prompt does not ask again.
 */
function relaunchApp(): void {
  app.relaunch();
  quitWithoutPrompt();
}

/** Hands one application-menu request to the window's renderer. */
function sendMenuCommand(command: MenuCommandV1): void {
  const win = appWindow;
  if (win === null || win.isDestroyed() || win.webContents.isDestroyed()) return;
  win.webContents.send(MENU_COMMAND_CHANNEL, command);
}

/** Help → Copy Diagnostics: Rust writes the filtered report; main copies it. */
async function copyDiagnostics(): Promise<void> {
  const host = rustHost;
  try {
    if (host === null) throw new Error("The backend is not running.");
    const result = plainObject(await host.invoke(CHANNELS.diagnostics, { kind: "report" }));
    if (typeof result?.report !== "string") throw new Error("The backend sent no report.");
    clipboard.writeText(result.report);
  } catch {
    sendMenuCommand({ version: 1, command: "error",
      text: `${appProductTitle} could not gather its diagnostics. Try again in a moment.` });
  }
}

/** Help → Reveal Logs: Rust shows host.log in Finder. */
async function revealLogs(): Promise<void> {
  try {
    if (rustHost === null) throw new Error("The backend is not running.");
    await rustHost.invoke(CHANNELS.diagnostics, { kind: "reveal", target: "logs" });
  } catch {
    sendMenuCommand({ version: 1, command: "error", text: "Finder could not show the logs." });
  }
}

/** Replaces Electron's stock menu (docs/APP_MENU.md). Hands-free and
 * packaged-smoke sessions keep none: their hooks make this a no-op. */
let backgroundMode = { enabled: false, available: false };

function applyBackgroundState(value: unknown): void {
  const state = plainObject(value);
  backgroundMode = { enabled: state?.enabled === true, available: state?.available === true };
  installAppMenu();
}

async function setBackgroundMode(enabled: boolean): Promise<void> {
  try {
    if (rustHost === null) throw new Error("The backend is not running.");
    applyBackgroundState(await rustHost.invoke(CHANNELS.backgroundControl, { kind: "set", enabled }));
  } catch {
    sendMenuCommand({ version: 1, command: "error",
      text: "The local runner preference could not be saved. It has not changed." });
  }
}

function installAppMenu(): void {
  Menu.setApplicationMenu(Menu.buildFromTemplate(appMenuTemplate(appProductTitle, {
    openSettings() {
      const win = appWindow;
      if (win !== null && !win.isDestroyed() && !win.isVisible()) {
        win.show();
        syncWindowVisibility(win);
      }
      sendMenuCommand({ version: 1, command: "settings" });
    },
    closePane() {
      // Only the window in front: a hidden or background window must never
      // lose a pane to a chord pressed somewhere else.
      const win = appWindow;
      if (win === null || win.isDestroyed() || !win.isVisible() || !win.isFocused()) return;
      sendMenuCommand({ version: 1, command: "close-pane" });
    },
    revealLogs() { void revealLogs(); },
    copyDiagnostics() { void copyDiagnostics(); },
    background: { ...backgroundMode, setEnabled: enabled => { void setBackgroundMode(enabled); } },
  })));
}

function showStartupFailure(error: unknown): void {
  if (startupFailureShown || crashAlertShown) return;
  startupFailureShown = true;
  const message = error instanceof Error ? error.message : String(error);
  dialog.showErrorBox(
    `${appProductTitle} could not open`,
    `${message}\n\n${appProductTitle} will close so you can reopen it.`,
  );
}

function isRegularFile(target: string): boolean {
  try {
    return statSync(target).isFile();
  } catch {
    return false;
  }
}

/** The window fill for the theme the window paints now. Between host reads
 * main resolves Follow macOS itself, with the host's rule. */
function currentWindowBackground(): string {
  return windowBackground({
    resolved: resolveAppearanceTheme(appearance, nativeTheme.shouldUseDarkColors),
    customThemes: appearance.customThemes,
  });
}

let surfaceTranslucency = SOLID_SURFACES;
function applyAppearance(state?: AppearanceState): void {
  if (state?.translucency) surfaceTranslucency = state.translucency;
  if (state !== undefined) appearance = state;
  nativeTheme.themeSource = appearanceThemeSource(appearance);
  const win = appWindow;
  if (win !== null && !win.isDestroyed()) {
    applyWindowMaterial(win, currentWindowBackground(), surfaceTranslucency);
  }
}

/** Visual replies' documents, served on `iface-visual:` (T3 section 8). */
const visualFrames = new VisualFrames();
/** Registry sessions waiting, from the last registry push. */
let waitingSessions = 0;
/** The renderer's last count of terminal panes waiting (shared/dock-attention.ts). */
let dockAttention: DockAttentionV1 | null = null;

/** Repaints the Dock badge; `listing` is a new registry payload, when one arrived. */
function paintDockBadge(listing?: unknown): void {
  if (dockBadgeFrozen) return;
  if (listing !== undefined) waitingSessions = waitingCount(listing);
  const next = dockBadgeCount(waitingSessions, dockAttention);
  if (next === dockBadge) return;
  dockBadge = next;
  app.setBadgeCount(next);
}

function applyCsp(location: RendererLocation): void {
  const csp = contentSecurityPolicy(location.devServerUrl);
  session.defaultSession.webRequest.onHeadersReceived((details, callback) => {
    const win = appWindow;
    if (win === null || win.isDestroyed() || win.webContents.isDestroyed()
        || details.webContentsId !== win.webContents.id
        || details.resourceType !== "mainFrame"
        || !isAllowedRendererNavigation(details.url, location)) {
      callback({});
      return;
    }
    const headers = Object.fromEntries(
      Object.entries(details.responseHeaders ?? {})
        .filter(([name]) => name.toLowerCase() !== "content-security-policy"),
    );
    callback({ responseHeaders: { ...headers, "Content-Security-Policy": [csp] } });
  });
}

/**
 * The microphone the voice assistant records with and the notifications the
 * renderer posts when an agent finishes or needs input, asked for by this
 * app's own top-level document, and nothing else. Camera, geolocation,
 * display capture, MIDI, pointer lock and the rest are refused here rather
 * than reaching the user as a prompt they would have to judge — an audio+video
 * request is a camera request and is refused with them.
 *
 * Electron asks this handler for every web Notification the page displays,
 * not only for Notification.requestPermission(): refusing "notifications"
 * here drops them silently while Notification.permission still reads
 * "granted" in the renderer.
 */
function applyPermissionPolicy(location: RendererLocation): void {
  session.defaultSession.setPermissionRequestHandler((contents, permission, decide, details) => {
    const current = appWindow;
    const ownDocument = current !== null && !current.isDestroyed()
      && !current.webContents.isDestroyed() && contents === current.webContents
      && details.isMainFrame
      && isAllowedRendererNavigation(details.requestingUrl, location);
    const mediaTypes = "mediaTypes" in details ? details.mediaTypes ?? [] : [];
    const microphoneOnly = permission === "media"
      && mediaTypes.length > 0 && mediaTypes.every(type => type === "audio");
    if (handsfree.enabled || packagedSmoke) {
      // A hands-free session reaches no microphone, notification or clipboard.
      decide(false);
      return;
    }
    decide(ownDocument && (microphoneOnly || permission === "notifications"));
  });
}

function createWindow(
  background: string,
  location: RendererLocation,
  buildChannel: ReturnType<typeof resolveBuildChannel>,
): BrowserWindow {
  const win = new BrowserWindow({
    show: !handsfree.enabled && !packagedSmoke && !hiddenUntilActivated,
    width: 1440,
    height: 900,
    minWidth: 760,
    minHeight: 560,
    title: appProductTitle,
    ...(process.platform === "darwin" ? {
      titleBarStyle: "hiddenInset" as const,
      trafficLightPosition: { x: 16, y: 16 },
      ...MAC_WINDOW_MATERIAL,
    } : {}),
    backgroundColor: background,
    webPreferences: {
      preload: join(here, "preload.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      // Browser panes only: installBrowserPaneGuards pins every guest.
      webviewTag: true,
      // A Browser page's fullscreen fills this window instead of taking the
      // window to a new Space (the guest's own preference is not consulted).
      disableHtmlFullscreenWindowResize: true,
      additionalArguments: [buildChannelArgument(buildChannel)],
    },
  });

  installWindowVisibility(win);
  if (handsfree.enabled || packagedSmoke) paintWhileHidden(win);
  if (process.platform === "darwin" && !packagedSmoke) {
    const recoverStuckRenderer = createStuckRendererRecovery(win, {
      cpuSeconds: pid => app.getAppMetrics().find(metric => metric.pid === pid)?.cpu.cumulativeCPUUsage,
      idleSeconds: () => powerMonitor.getSystemIdleTime(),
      now: () => Date.now(),
    });
    const recoveryTimer = setInterval(() => {
      void recoverStuckRenderer().then(pulsed => {
        if (pulsed) console.warn("[main] reset a stuck renderer compositor by re-showing the window");
      }, () => {
        console.warn("[main] stuck renderer recovery failed");
      });
    }, STUCK_RENDERER_CHECK_MS);
    win.on("closed", () => { clearInterval(recoveryTimer); });
  }
  win.webContents.setWindowOpenHandler(({ url }) => {
    const external = validatedExternalUrl(url);
    if (external !== null) {
      void shell.openExternal(external).catch(() => {
        console.warn("[main] the operating system could not open an external link");
      });
    }
    return { action: "deny" };
  });
  // A visual reply's frame may load its page once and never move again
  // (T3 section 8, visual-frames.ts); nothing else leaves the renderer.
  const guardNavigation = rendererNavigationGuard(url => isAllowedRendererNavigation(url, location));
  win.webContents.on("will-navigate", guardNavigation);
  win.webContents.on("will-frame-navigate", guardNavigation);
  win.webContents.on("will-redirect", guardNavigation);
  return win;
}

function installWindowLifecycle(win: BrowserWindow): void {
  const lifecycle = createWindowLifecycleCoordinator({
    isQuitting: () => quitting,
    setQuitting: value => { quitting = value; if (!value) quitWithoutAsking = false; },
    isShutdownStarted: () => shutdownStarted,
    isWindowDestroyed: () => win.isDestroyed(),
    shouldHideOnClose: () => backgroundMode.enabled,
    hideWindow: () => { hideWindow(win); },
    defer: task => { setImmediate(task); },
    closeWindow: () => { win.close(); },
    beginShutdown,
  });
  win.webContents.on("will-prevent-unload", (event) => {
    // The renderer asks for explicit confirmation before requesting an update.
    // Only bypass its ordinary dirty-editor veto once Electron's native updater
    // announces that it is actually ready to restart; until then a user-driven
    // quit retains the normal Cancel / Discard choice.
    if (discardUnsavedForUpdate) {
      event.preventDefault();
      return;
    }
    const choice = dialog.showMessageBoxSync(win, {
      type: "warning",
      buttons: ["Cancel", "Discard changes"],
      defaultId: 0,
      cancelId: 0,
      title: appProductTitle,
      message: "Discard unsaved changes?",
      detail: "One or more open editors have changes that have not been saved. "
        + "If you continue, those changes will be lost.",
      noLink: true,
    });
    if (choice === 1) event.preventDefault();
    else lifecycle.onUnloadCancelled();
  });
  win.on("close", (event) => {
    if (!quitting && !backgroundMode.enabled) {
      event.preventDefault();
      app.quit(); // The ordinary quit guard asks about active work before stopping it.
      return;
    }
    lifecycle.onCloseRequested(event);
  });
  // Install this before any awaited startup work. WebContents can disappear
  // without BrowserWindow.closed (for example, renderer-side window.close),
  // and the single-instance Rust owner must never survive without its surface.
  win.webContents.on("destroyed", () => {
    lifecycle.onWebContentsDestroyed();
  });
  win.on("closed", () => {
    if (appWindow === win) appWindow = null;
    // An ordinary macOS close is prevented above, so reaching `closed` means
    // an explicit quit or unexpected destruction. Neither may strand the
    // single-instance Rust owner without its desktop surface.
    lifecycle.onWindowClosed();
  });
}

async function start(): Promise<void> {
  if (offerApplicationsMove({
    isPackaged: app.isPackaged,
    platform: process.platform,
    inApplicationsFolder: process.platform === "darwin" && app.isInApplicationsFolder(),
    unattended: handsfree.enabled || packagedSmoke,
  }, {
    ask: () => dialog.showMessageBoxSync({
      ...APPLICATIONS_MOVE_QUESTION, buttons: [...APPLICATIONS_MOVE_QUESTION.buttons],
    }),
    move: () => app.moveToApplicationsFolder(),
  })) return;
  const appName = app.getName();
  const location = resolveRendererLocation({
    isPackaged: app.isPackaged,
    allowPackagedDevServer: appName === "Interface Dev"
      && process.env.INTERFACE_DEV_SUPERVISED === "1",
    devServerUrl: process.env.VITE_DEV_SERVER_URL,
    packagedEntryUrl: pathToFileURL(join(here, "../../dist/index.html")).href,
  });
  const buildChannel = resolveBuildChannel({
    appName,
    viteDevServerUrl: location.devServerUrl,
  });
  appProductTitle = productTitle(buildChannel);
  installAppMenu();
  const command = resolveRustHostPath({
    isPackaged: app.isPackaged,
    mainDirectory: here,
    resourcesPath: process.resourcesPath,
    override: app.isPackaged ? undefined : process.env.INTERFACE_RUST_HOST,
    isFile: isRegularFile,
  });

  // Create the native window before awaiting backend setup. A slow or failed
  // startup remains visible to the user instead of looking like a dock-only,
  // unresponsive application.
  // A quiet Dev refresh is the exception: it stays out of sight until the
  // owner switches to it (devLaunchStaysHidden).
  hiddenUntilActivated = devLaunchStaysHidden(appName, process.env);
  const win = createWindow(currentWindowBackground(), location, buildChannel);
  appWindow = win;
  if (hiddenUntilActivated) {
    console.log("[main] quiet Dev refresh: the window opens when Interface Dev is next activated");
    win.once("show", () => { hiddenUntilActivated = false; });
  }
  installWindowLifecycle(win);
  // Both depend only on the location, and the first document must load under
  // the header CSP and the permission policy.
  applyCsp(location);
  applyPermissionPolicy(location);
  installBrowserPaneGuards(win.webContents, session.fromPartition(BROWSER_PANE_PARTITION),
    browserPaneNativeHooks(() => appWindow, handsfree.enabled || packagedSmoke));
  installVisualFrameProtocol(session.defaultSession.protocol, visualFrames);
  // WebRTC is outside CSP: without this a visual reply's script could reach
  // any address over UDP or TCP (visual-network-fence.ts). Before the first
  // document loads; setting a proxy takes a few milliseconds.
  let networkFenceApplied = false;
  await fenceWindowNetwork(win.webContents, session.defaultSession, location.devServerUrl)
    .then(() => { networkFenceApplied = true; })
    .catch((error: unknown) => { console.error("[main] the window's network fence did not apply", error); });

  // A hands-free `stop` signals Electron's whole process group. The host gets
  // its own, so it still ends through stdin EOF and its terminal teardown.
  const host = new RustHostClient({ ...packagedSmokeHostCommand(packagedSmoke, command), detached: handsfree.enabled, env: {
    ...handsfree.hostEnvironment(process.env),
    INTERFACE_REMOTE_ASSETS: app.isPackaged
      ? join(process.resourcesPath, "remote") : join(here, "../../dist-remote"),
  } });
  rustHost = host;
  // Until the startup reads below succeed, a host failure also fails them, and
  // their catch says so once ("could not open") and closes. The crash alert
  // and its Relaunch are for a backend that had started.
  let hostStarted = false;
  let startupHostFailure: Error | null = null;
  removeHostFailureListener = host.onFailure((error) => {
    if (shutdownStarted) return;
    // The host's agents ended with it; quitting now loses nothing.
    workInFlight = null;
    keepAwake.update(null);
    if (!hostStarted) {
      startupHostFailure ??= error;
      return;
    }
    if (showCrashAlert("backend", error.message)) relaunchApp();
  });
  // The renderer loads while the host boots, so its bundle parses in parallel
  // with the host's login shell and store opens. Every bridge request and the
  // terminal handshake wait for this; main.tsx holds its first render for the
  // persisted appearance. Settled here, rethrown where it is awaited below.
  const hostReady = host.start().then(() => undefined);
  hostReady.catch(() => undefined);

  const onUpdateFailure = (): void => { resetPendingUpdateAfterFailure(); };
  // electron-updater evaluates every platform's updater on import (about 160
  // modules), which at module scope ran before the first window existed. It
  // now loads beside the host's start; configure() waits for it, and every
  // check and install waits for configure().
  const updater = new NativeUpdaterEffects({
    host,
    updater: import("electron-updater").then(({ default: { autoUpdater } }) => {
      // electron-updater's logger can include private artifact URLs. Rust owns the
      // feed policy and all user-visible errors, so the native effect needs no log.
      autoUpdater.logger = null;
      // Registered for the process lifetime: MacUpdater re-emits native errors as 'error', and an unlistened 'error' throws.
      autoUpdater.on("error", onUpdateFailure);
      return createNativeUpdaterPort(autoUpdater);
    }),
  });
  nativeUpdater = updater;
  // Signature discovery can launch multiple bounded codesign processes. Start
  // it promptly, but normalize rejection immediately so the first renderer can
  // load without creating an unhandled deferred startup failure. The result is
  // rethrown below before the native updater can start producing events.
  const updaterIsActive = (): boolean => !shutdownStarted && nativeUpdater === updater;
  const requireUpdaterConfiguration = createStartupGate(
    hostReady.then(() => withStartupDeadline("update service", updater.configure({
      isPackaged: app.isPackaged,
      currentVersion: app.getVersion(),
      appBundlePath: app.isPackaged && process.platform === "darwin"
        ? dirname(dirname(dirname(app.getPath("exe"))))
        : null,
    }))),
    updaterIsActive,
  );

  const nativePayload = createNativePayloadHook({
    systemDark: () => nativeTheme.shouldUseDarkColors,
    async defaultDirectory() {
      const result = await host.invoke(CHANNELS.getDefaultDirectory, {});
      if (typeof result !== "string") {
        throw new TypeError("The Rust host returned an invalid default directory.");
      }
      return result;
    },
    async projectsFolder() {
      const result: unknown = await host.invoke(CHANNELS.createProject, { projectsFolder: "read" });
      const path = typeof result === "object" && result !== null
        ? (result as { path?: unknown }).path : undefined;
      if (typeof path !== "string") {
        throw new TypeError("The Rust host returned an invalid Projects folder.");
      }
      return path;
    },
    async pickDirectory(defaultPath) {
      if (win.isDestroyed()) return null;
      const safeDefaultPath = await host.invoke(NATIVE_DIRECTORY_PICKER_START_PATH, {
        preferredPath: defaultPath,
      });
      if (typeof safeDefaultPath !== "string") {
        throw new TypeError("The Rust host returned an invalid directory picker start path.");
      }
      const result = await dialog.showOpenDialog(win, {
        defaultPath: safeDefaultPath,
        message: "Choose a folder, or use New Folder to create a project directory.",
        properties: ["openDirectory", "createDirectory"],
      });
      return result.canceled ? null : result.filePaths[0] ?? null;
    },
  });
  const terminalBridgeGate = new TerminalDataBridgeGate();
  ipcMain.on(DOCK_ATTENTION_CHANNEL, (event, payload: unknown) => {
    const current = appWindow;
    const contents = current !== null && !current.isDestroyed() ? current.webContents : null;
    if (!isAuthorizedIpcSender(event, contents)
        || !isAllowedRendererNavigation(event.senderFrame?.url ?? "", location)) return;
    const report = parseDockAttention(payload);
    if (report === null) return;
    dockAttention = report;
    paintDockBadge();
  });
  ipcMain.handle(VISUAL_FRAME_CHANNEL, (event, payload: unknown) => {
    const current = appWindow;
    const contents = current !== null && !current.isDestroyed() ? current.webContents : null;
    if (!isAuthorizedIpcSender(event, contents)
        || !isAllowedRendererNavigation(event.senderFrame?.url ?? "", location)) return null;
    const request = parseVisualFrameRequest(payload);
    if (request === null) return null;
    if (request.kind === "register") return networkFenceApplied ? visualFrames.register(request.html) : null;
    visualFrames.release(request.url);
    return null;
  });
  ipcMain.on(WORK_IN_FLIGHT_CHANNEL, (event, payload: unknown) => {
    const current = appWindow;
    const contents = current !== null && !current.isDestroyed() ? current.webContents : null;
    if (!isAuthorizedIpcSender(event, contents)
        || !isAllowedRendererNavigation(event.senderFrame?.url ?? "", location)) return;
    const work = parseWorkInFlight(payload);
    if (work === null) return;
    workInFlight = work;
    keepAwake.update(work);
  });

  removeBridge = installRustIpcBridge({
    host,
    ipcMain,
    targets: () => appWindow === null || appWindow.isDestroyed()
      || appWindow.webContents.isDestroyed() ? [] : [appWindow.webContents],
    authorize(event) {
      const invoke = event as IpcMainInvokeEvent;
      const current = appWindow;
      const contents = current !== null && !current.isDestroyed()
        ? current.webContents : null;
      return isAuthorizedIpcSender(invoke, contents)
        && isAllowedRendererNavigation(invoke.senderFrame?.url ?? "", location);
    },
    async beforeInvoke(operation, payload) {
      // Hands-free sessions refuse what would reach past them: other apps via
      // /usr/bin/open, the microphone, phone remote listeners.
      handsfree.guard(operation, payload);
      guardPackagedSmoke(packagedSmoke, operation, payload);
      // The renderer can ask while the host is still starting; a failed start
      // rejects the request with the host's own error.
      await hostReady;
      if (operation === CHANNELS.updateSessions
          || operation === CHANNELS.getUpdateState
          || operation === CHANNELS.checkForUpdates
          || operation === CHANNELS.restartToUpdate) {
        await requireUpdaterConfiguration();
      }
      if (operation === CHANNELS.createTerminal || operation === CHANNELS.restartTerminal) {
        terminalBridgeGate.requireReady();
      }
      return nativePayload(operation, payload);
    },
    async afterInvoke(operation, result) {
      if (operation === CHANNELS.customizationControl) {
        const source = plainObject(result)?.sourcePath;
        if (typeof source === "string" && !handsfree.enabled && !packagedSmoke) shell.showItemInFolder(source);
      }
      if (operation === CHANNELS.backgroundControl) applyBackgroundState(result);
      if (operation === CHANNELS.getAppearance || operation === CHANNELS.setAppearance) {
        applyAppearance(appearanceState(result));
      }
      if (operation === CHANNELS.checkForUpdates) {
        if (!updaterIsActive()) return undefined;
        await updater.check();
        if (!updaterIsActive()) return undefined;
        return host.invoke(CHANNELS.getUpdateState, {});
      }
      if (operation === CHANNELS.restartToUpdate && result === true) {
        if (updateInstallRequested || !updaterIsActive()) return false;
        // On macOS electron-updater may still be preparing its native Squirrel
        // update after quitAndInstall returns. Keep the window and Rust owner
        // alive until the native before-quit-for-update event proves that the
        // handoff is ready. Closing here would kill the loopback update feed.
        updateInstallRequested = true;
        const accepted = await updater.install();
        if (!accepted) updateInstallRequested = false;
        return accepted;
      }
      return undefined;
    },
    // The one voice UI tool the page cannot serve: the renderer can read its
    // own DOM but not draw itself.
    async capturePage() {
      const current = appWindow;
      if (current === null || current.isDestroyed() || current.webContents.isDestroyed()) {
        throw new Error("The window is not available to capture.");
      }
      const image = await current.webContents.capturePage();
      return image.toPNG().toString("base64");
    },
  });
  removeBadgeListener = host.onPush((frame) => {
    if (frame.operation === CHANNELS.registry) paintDockBadge(frame.payload);
    if (frame.operation === CHANNELS.terminalLifecycle) {
      updateTerminalDataRetention(host, frame.payload);
    }
  });

  const terminalBridge = createTerminalBridgeController({
    win,
    location,
    ipc: ipcMain,
    gate: terminalBridgeGate,
    transport: () => host.terminalDataTransport,
    hostReady,
    isShuttingDown: () => shutdownStarted,
    productTitle: appProductTitle,
    createChannel: () => new MessageChannelMain(),
    onRendererGone() {
      if (quitting || shutdownStarted) return;
      quitting = true;
      // Relaunch starts the new copy once this shutdown has finished.
      if (showCrashAlert("window", "")) app.relaunch();
      if (!win.isDestroyed()) win.destroy();
      beginShutdown();
    },
  });
  removeTerminalDataBridge = () => { terminalBridge.dispose(); };
  const loaded = win.loadURL(location.devServerUrl ?? location.packagedEntryUrl);
  loaded.catch(() => undefined);
  try {
    await hostReady;
    applyBackgroundState(await withStartupDeadline("background preference",
      host.invoke(CHANNELS.backgroundControl, { kind: "read" })));
    applyAppearance(appearanceState(await withStartupDeadline(
      "appearance service",
      nativePayload(CHANNELS.getAppearance, {})
        .then(payload => host.invoke(CHANNELS.getAppearance, payload)),
    )));
  } catch (error) {
    // Startup used to fail before any renderer existed. The document now
    // loads during host start, so take the updater failure's path: destroy
    // the window without a beforeunload veto, then the durable shutdown.
    if (shutdownStarted) return;
    showStartupFailure(error);
    if (!win.isDestroyed()) win.destroy();
    beginShutdown();
    return;
  }
  await loaded;
  // Counted from here, so host startup never spends the handshake deadline.
  terminalBridge.armRequestDeadline();
  await terminalBridge.ready;
  let configuration;
  try {
    configuration = await requireUpdaterConfiguration();
  } catch (error) {
    if (!updaterIsActive()) return;
    // Startup used to fail before the renderer loaded. Preserve that fatal
    // contract after overlapping configuration with first paint: destroy the
    // visible surface without allowing a dirty beforeunload veto to strand a
    // half-started host, then take the same idempotent durable shutdown path.
    showStartupFailure(error);
    if (!win.isDestroyed()) win.destroy();
    beginShutdown();
    return;
  }
  if (startupHostFailure !== null) {
    if (shutdownStarted) return;
    showStartupFailure(startupHostFailure);
    if (!win.isDestroyed()) win.destroy();
    beginShutdown();
    return;
  }
  hostStarted = true;
  updater.start();
  if (configuration.enabled) {
    updater.watchLocalFeed(join(app.getPath("userData"), "update-feed"));
  }
  win.on("focus", () => {
    if (!updaterIsActive()) return;
    void updater.checkOnForeground();
  });
  if (appName === "Interface Dev" && process.env.INTERFACE_DEV_SUPERVISED === "1") {
    void reportDevReadiness({
      launch: process.env.INTERFACE_DEV_LAUNCH,
      revision: process.env.INTERFACE_DEV_REVISION,
      active: () => !shutdownStarted && !win.isDestroyed() && !win.webContents.isDestroyed()
        && terminalBridgeGate.isReady,
      probe: () => win.webContents.executeJavaScript(DEV_READINESS_PROBE),
      emit: line => console.log(line),
    });
  }
}

function prepareShutdown(): Promise<void> {
  if (shutdownTask !== null) return shutdownTask;
  shutdownStarted = true;
  keepAwake.release();
  dockBadgeFrozen = true;
  dockBadge = 0;
  app.setBadgeCount(0);
  // A step that throws (a destroyed window, a half-started bridge) must not
  // strand the host: shutdownStarted is already set, so nothing would retry.
  runShutdownSteps([
    () => { removeBadgeListener?.(); },
    () => { removeHostFailureListener?.(); },
    () => { removeTerminalDataBridge?.(); },
    () => { removeBridge?.(); },
    () => { nativeUpdater?.dispose(); },
  ], error => {
    const detail = recordMainProcessError(mainProcessErrorLog(), "shutdownStep", error, "shutdown");
    console.error("[main] shutdown step failed:", detail);
  });
  removeBadgeListener = null;
  removeHostFailureListener = null;
  removeTerminalDataBridge = null;
  removeBridge = null;
  nativeUpdater = null;
  const host = rustHost;
  rustHost = null;
  shutdownTask = host?.stop(10_000) ?? Promise.resolve();
  return shutdownTask;
}

/**
 * Cmd+Q, the Dock's Quit or a logout while agents are mid-turn asks first
 * (closing a tab does not: owner, 2026-10-01). True while the answer is
 * pending; the quit then waits for it. The sheet is asynchronous so a signal
 * still quits while it is up: the dev supervisor's restart and `kill` have no
 * one to answer, and neither do hands-free or packaged-smoke sessions.
 */
function askBeforeQuit(): boolean {
  const win = appWindow;
  if (quitWithoutAsking || handsfree.enabled || packagedSmoke
      || win === null || win.isDestroyed()) return false;
  const prompt = quitPrompt(workInFlight);
  if (prompt === null) return false;
  if (quitPromptAbort !== null) return true;
  if (!win.isVisible()) {
    // A quiet Dev refresh's window nobody has opened holds nothing to lose.
    if (hiddenUntilActivated) return false;
    // A window closed to the Dock still runs its agents: show what is asking.
    if (win.isMinimized()) win.restore();
    win.show();
    syncWindowVisibility(win);
  }
  const abort = new AbortController();
  quitPromptAbort = abort;
  const settled = (): void => { if (quitPromptAbort === abort) quitPromptAbort = null; };
  void dialog.showMessageBox(win, {
    type: "warning",
    buttons: ["Cancel", "Quit"],
    defaultId: 0,
    cancelId: 0,
    title: appProductTitle,
    message: prompt.message,
    detail: prompt.detail,
    noLink: true,
    signal: abort.signal,
  }).then(({ response }) => {
    settled();
    if (response !== 1 || abort.signal.aborted) return;
    quitWithoutAsking = true;
    app.quit();
  }, settled);
  return true;
}

/** A quit with no one to answer a prompt; dismisses one already up. */
function quitWithoutPrompt(): void {
  quitWithoutAsking = true;
  quitPromptAbort?.abort();
  quitPromptAbort = null;
  app.quit();
}

function beginShutdown(): void {
  if (shutdownStarted) return;
  void prepareShutdown().finally(() => {
    shutdownComplete = true;
    app.quit();
  });
}

nativeTheme.on("updated", () => {
  if (appearanceThemeSource(appearance) === "system") applyAppearance();
});

electronAutoUpdater.on("before-quit-for-update", () => {
  if (!updateInstallRequested) return;
  // From this point the native updater, not the outer download proxy, owns the
  // restart. The renderer already received explicit consent and is blocked
  // from new edits, so let the dirty-editor hook unload without a second,
  // potentially hidden prompt.
  discardUnsavedForUpdate = true;
  quitting = true;
});

electronAutoUpdater.on("error", () => {
  // Native preparation failed while the visible app was still alive. Restore
  // ordinary close semantics; NativeUpdaterEffects publishes the error state
  // that removes the renderer's preparing overlay.
  resetPendingUpdateAfterFailure();
});

const ownsSingleInstance = app.requestSingleInstanceLock();
if (!ownsSingleInstance) {
  app.quit();
} else {
  const revealWindow = (): void => {
    if (packagedSmoke) return;
    const win = appWindow;
    if (win === null || win.isDestroyed()) return;
    if (!win.isVisible() || win.isMinimized()) {
      if (win.isMinimized()) win.restore();
      win.show();
    }
    win.focus();
    syncWindowVisibility(win);
  };
  app.on("second-instance", revealWindow);
  app.on("activate", revealWindow);
  // A Dock click arrives as activate; Cmd-Tab or the app switcher only makes
  // the app active, which must still bring a quiet Dev refresh's window up.
  app.on("did-become-active", () => { if (hiddenUntilActivated) revealWindow(); });
  app.on("before-quit", (event) => {
    if (shutdownComplete) return;
    event.preventDefault();
    if (shutdownStarted) return;
    if (!quitting && askBeforeQuit()) return;
    quitting = true;
    const win = appWindow;
    if (win === null || win.isDestroyed()) {
      beginShutdown();
      return;
    }
    win.close();
  });
  app.whenReady().then(start).catch((error: unknown) => {
    console.error("[main] startup failed:", error);
    if (!shutdownStarted) showStartupFailure(error);
    if (appWindow !== null && !appWindow.isDestroyed()) appWindow.destroy();
    beginShutdown();
  });
  app.on("window-all-closed", () => { app.quit(); });
  // A SIGTERM — the dev supervisor's restart, a plain `kill` — used to end the
  // process where it stood: no before-quit, so no host stop and no time for
  // the renderer's debounced writes or localStorage to land. The Rust host was
  // left behind holding the profile's locks and the next launch died on them.
  // Route the signals through the same quit the menu uses.
  for (const signal of ["SIGTERM", "SIGINT", "SIGHUP"] as const) {
    process.on(signal, () => { quitWithoutPrompt(); });
  }
}
