interface VisibilityWindow {
  isDestroyed(): boolean;
  isVisible(): boolean;
  isMinimized(): boolean;
  on(event: "show" | "hide" | "minimize" | "restore" | "focus" | "blur", listener: () => void): unknown;
  webContents: {
    isDestroyed(): boolean;
    setBackgroundThrottling(allowed: boolean): void;
  };
}

interface HideableWindow extends VisibilityWindow {
  hide(): void;
}

interface RecoverableWindow extends HideableWindow {
  show(): void;
  isFocused(): boolean;
  webContents: VisibilityWindow["webContents"] & {
    getOSProcessId(): number;
    executeJavaScript(code: string): Promise<unknown>;
  };
}

export interface RendererLoadProbe {
  /** Cumulative CPU seconds used by a process, when the platform reports it. */
  cpuSeconds(pid: number): number | undefined;
  /** Seconds since the last keyboard or pointer input anywhere on the system. */
  idleSeconds(): number;
  now(): number;
}

export const STUCK_RENDERER_CHECK_MS = 60_000;
const STUCK_RENDERER_CORES = 0.4;
const STUCK_RENDERER_CHECKS = 10;
const RECOVERY_IDLE_SECONDS = 2;
const RECOVERY_COOLDOWN_MS = 30 * 60_000;
const HIDDEN_PAGE_POLLS = 10;

/** Native occlusion can incorrectly park a shown macOS window's frames. Keep
 * shown windows painting, but retain timer throttling on hide-on-close/minimize.
 * Focus is deliberately irrelevant: a visible background pane still renders. */
export function installWindowVisibility(win: VisibilityWindow): void {
  const sync = (): void => syncWindowVisibility(win);
  for (const event of ["show", "hide", "minimize", "restore", "focus", "blur"] as const) win.on(event, sync);
  sync();
}

const paintedWhileHidden = new WeakSet<VisibilityWindow>();

/** Hands-free agent sessions (`HANDSFREE=1`, see `handsfree.ts`) never show
 * their window, yet drive and capture it over CDP, so it paints and runs its
 * timers as if shown for its whole life. Ordinary windows never enter here. */
export function paintWhileHidden(win: VisibilityWindow): void {
  paintedWhileHidden.add(win);
  syncWindowVisibility(win);
}

/** Also called after our explicit hide/reveal actions. On macOS, native
 * occlusion notifications can arrive later, or omit hide for an already
 * occluded window. The command's resulting native state is authoritative. */
export function syncWindowVisibility(win: VisibilityWindow): void {
  if (win.isDestroyed() || win.webContents.isDestroyed()) return;
  const shown = paintedWhileHidden.has(win) || (win.isVisible() && !win.isMinimized());
  win.webContents.setBackgroundThrottling(!shown);
}

/** Throttling must be allowed before hide(). Allowing it afterwards leaves the
 * hidden page "visible" and drawing: Chromium ignored the hide while throttling
 * was off and does not revisit it (measured on Electron 43.3.0, 2026-09-17). */
export function hideWindow(win: HideableWindow): void {
  if (win.isDestroyed() || win.webContents.isDestroyed()) return;
  win.webContents.setBackgroundThrottling(true);
  win.hide();
  syncWindowVisibility(win);
}

/** Chromium's compositor can wedge its frame-timing bookkeeping (cc::FrameSorter
 * stops releasing frames; the code is unchanged on Chromium main as of
 * 2026-09-17). Its tracker list then grows until the renderer burns a whole
 * core, and only a real renderer hide clears it. Shown windows never hide, so
 * once the renderer stays busy for ten minutes, hide and re-show the focused
 * window while the user is idle: a few-millisecond pulse. Returns whether it
 * pulsed. */
export function createStuckRendererRecovery(
  win: RecoverableWindow,
  probe: RendererLoadProbe,
): () => Promise<boolean> {
  let previous: { at: number; seconds: number } | undefined;
  let busyChecks = 0;
  let lastRecoveryAt = Number.NEGATIVE_INFINITY;
  return async () => {
    if (win.isDestroyed() || win.webContents.isDestroyed()) return false;
    const at = probe.now();
    const seconds = probe.cpuSeconds(win.webContents.getOSProcessId());
    const cores = previous !== undefined && seconds !== undefined && at > previous.at
      ? (seconds - previous.seconds) / ((at - previous.at) / 1000)
      : 0;
    previous = seconds === undefined ? undefined : { at, seconds };
    busyChecks = cores >= STUCK_RENDERER_CORES ? busyChecks + 1 : 0;
    if (busyChecks < STUCK_RENDERER_CHECKS || at - lastRecoveryAt < RECOVERY_COOLDOWN_MS) return false;
    // Only a focused window is already frontmost on the active Space, so
    // showing it again cannot pull it over another app or switch Spaces.
    if (!win.isFocused() || !win.isVisible() || win.isMinimized()) return false;
    if (probe.idleSeconds() < RECOVERY_IDLE_SECONDS) return false;
    busyChecks = 0;
    lastRecoveryAt = at;
    hideWindow(win);
    for (let poll = 0; poll < HIDDEN_PAGE_POLLS && !await pageHidden(win); poll += 1) await delay(10);
    if (win.isDestroyed()) return true;
    win.show();
    syncWindowVisibility(win);
    return true;
  };
}

/** Bounded: a renderer that never answers must not leave the window hidden. */
function pageHidden(win: RecoverableWindow): Promise<boolean> {
  if (win.webContents.isDestroyed()) return Promise.resolve(true);
  return Promise.race([
    win.webContents.executeJavaScript("document.visibilityState").then(state => state === "hidden", () => false),
    delay(50).then(() => false),
  ]);
}

function delay(ms: number): Promise<void> {
  return new Promise(resolve => { setTimeout(resolve, ms); });
}
