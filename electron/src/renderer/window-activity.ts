/**
 * Marks the document while Interface is not the app in front, so its looping
 * animations rest (owner, 2026-10-22, battery).
 *
 * Any running loop makes Chromium draw the whole window at the display's
 * refresh rate, 120 Hz on ProMotion: with the update control and a working
 * agent's sidebar ticker showing, a hidden-window measurement put Interface at
 * about 45% CPU against 1% with them still. theme.css turns
 * `data-window-active="false"` into `--ambient-play-state: paused`, which only
 * the looping rules read. One-shot motion (exit fades, notices, pane motion)
 * keeps running, since code waits on its `animationend`.
 *
 * `document.hasFocus()` stays true while a Browser pane's `<webview>` holds
 * focus, so typing in a page is still "in Interface". The window's `blur`
 * fires before focus lands in that webview, hence the re-check a task later.
 */
export function installWindowActivity(win: Window = window): () => void {
  const root = win.document.documentElement;
  const sync = (): void => {
    const active = win.document.visibilityState === "visible" && win.document.hasFocus();
    root.dataset.windowActive = active ? "true" : "false";
  };
  let pending: number | undefined;
  const later = (): void => {
    win.clearTimeout(pending);
    pending = win.setTimeout(sync, 0);
  };
  sync();
  win.addEventListener("focus", later);
  win.addEventListener("blur", later);
  win.document.addEventListener("visibilitychange", later);
  return () => {
    win.clearTimeout(pending);
    win.removeEventListener("focus", later);
    win.removeEventListener("blur", later);
    win.document.removeEventListener("visibilitychange", later);
  };
}
