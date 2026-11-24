import { Component, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ProductName } from "./components/ProductName";
import { promptDropAccepts } from "./prompt-drop";
import { installKeyboardInputGuard } from "./keyboard-input";
import { installWindowActivity } from "./window-activity";
import { installLoopRest } from "./loop-rest";
import { applyAppearance } from "./appearance";
import "./theme.css";
import "./codicons.css";

function preventFileNavigation(event: DragEvent): void {
  const types = event.dataTransfer === null ? [] : Array.from(event.dataTransfer.types);
  if (!promptDropAccepts(types)) return;
  event.preventDefault();
}

window.addEventListener("dragover", preventFileNavigation);
window.addEventListener("drop", preventFileNavigation);
const removeKeyboardInputGuard = /Mac/.test(navigator.platform)
  ? installKeyboardInputGuard(window) : () => {};
if (import.meta.hot) import.meta.hot.dispose(removeKeyboardInputGuard);
const removeWindowActivity = installWindowActivity(window);
if (import.meta.hot) import.meta.hot.dispose(removeWindowActivity);
const removeLoopRest = installLoopRest(window);
if (import.meta.hot) import.meta.hot.dispose(removeLoopRest);

interface ErrorBoundaryState {
  failed: boolean;
}

/**
 * The safety net under every render.
 *
 * React tears the tree down to the NEAREST error boundary, so with none at all
 * one uncaught render exception unmounts the whole root — a permanently black
 * window with a running main process behind it, which is a shape this app has
 * shipped before. This boundary makes the floor a sentence and a button
 * instead of nothing.
 *
 * Deliberately minimal:
 *  - no error text in the UI. The stack goes to the console, where it is
 *    actionable; a raw exception string in front of the user is noise they
 *    cannot do anything with.
 *  - no reset-and-retry. State that crashed once crashes again on the next
 *    render, so the only honest offer is a clean restart of the RENDERER —
 *    location.reload(), which leaves the main process, every engine child and
 *    every running session untouched.
 *  - styled only by theme.css classes (`crash-fallback`, `empty`,
 *    `btn btn-primary`), never CSS a component injects at render — a crash in
 *    that component could keep its <style> from mounting. This is the one view
 *    at the document root, so it sits under the macOS traffic lights instead of
 *    below a pane's titlebar; `crash-fallback` reserves and drags that strip the
 *    way `.sidebar::before` does, keeping the copy clear of the buttons and the
 *    crashed window movable. The inner copy reuses theme.css's zero-state classes.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, ErrorBoundaryState> {
  state: ErrorBoundaryState = { failed: false };

  static getDerivedStateFromError(): ErrorBoundaryState {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // The componentStack is the half a plain window.onerror never gets: which
    // component threw, not just where in the bundle.
    console.error("[renderer] render error:", error, info.componentStack);
  }

  render(): ReactNode {
    if (!this.state.failed) return this.props.children;
    return (
      <div className="crash-fallback" role="alert">
        <div className="empty">
          <p className="empty-text"><ProductName /> hit a rendering error.</p>
          <p className="empty-hint">
            Reload redraws the window. Engines and sessions keep running — nothing
            is lost.
          </p>
          <div className="empty-actions">
            <button type="button" className="btn btn-primary"
              onClick={() => { location.reload(); }}>
              Reload
            </button>
          </div>
        </div>
      </div>
    );
  }
}

// Main loads this document while the Rust host is still starting, before the
// persisted theme is known; until then only theme.css's system fallback paints,
// the same colour as the native window behind it. Nothing renders until the
// host answers: its appearance is applied to the document here (and to
// nativeTheme by main) first. useAppearance starts from the state applied
// here, so its first paint repeats it instead of a light default, and then
// confirms it. A failed read still renders, and useAppearance reports it.
const root = createRoot(document.getElementById("root")!);
void window.iface.getAppearance().then(applyAppearance, () => undefined).finally(() => {
  root.render(<ErrorBoundary><App /></ErrorBoundary>);
});
