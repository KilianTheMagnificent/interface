import { describe, expect, it } from "vitest";
import { createWindowLifecycleCoordinator } from "./window-lifecycle.js";

function lifecycleHarness(options: { hideOnClose?: boolean } = {}) {
  const state = {
    quitting: false,
    shutdownStarted: false,
    windowDestroyed: false,
    hideCalls: 0,
    closeCalls: 0,
    shutdownCalls: 0,
  };
  const deferred: Array<() => void> = [];
  const lifecycle = createWindowLifecycleCoordinator({
    isQuitting: () => state.quitting,
    setQuitting: value => { state.quitting = value; },
    isShutdownStarted: () => state.shutdownStarted,
    isWindowDestroyed: () => state.windowDestroyed,
    shouldHideOnClose: () => options.hideOnClose === true,
    hideWindow: () => { state.hideCalls += 1; },
    defer: task => { deferred.push(task); },
    closeWindow: () => { state.closeCalls += 1; },
    beginShutdown: () => {
      state.shutdownCalls += 1;
      state.shutdownStarted = true;
    },
  });
  let prevented = 0;

  return {
    state,
    lifecycle,
    deferred,
    closeEvent: { preventDefault: () => { prevented += 1; } },
    prevented: () => prevented,
    runDeferred: () => { deferred.shift()?.(); },
  };
}

describe("window lifecycle", () => {
  it("hides an ordinary background close without starting shutdown", () => {
    const harness = lifecycleHarness({ hideOnClose: true });

    harness.lifecycle.onCloseRequested(harness.closeEvent);

    expect(harness.prevented()).toBe(1);
    expect(harness.state.hideCalls).toBe(1);
    expect(harness.state.quitting).toBe(false);
    expect(harness.state.closeCalls).toBe(0);
    expect(harness.state.shutdownCalls).toBe(0);
    expect(harness.deferred).toHaveLength(0);
  });

  it("defers an ordinary close without starting shutdown", () => {
    const harness = lifecycleHarness();

    harness.lifecycle.onCloseRequested(harness.closeEvent);

    expect(harness.prevented()).toBe(1);
    expect(harness.state.quitting).toBe(false);
    expect(harness.state.closeCalls).toBe(0);
    expect(harness.state.shutdownCalls).toBe(0);
    expect(harness.deferred).toHaveLength(1);

    harness.runDeferred();
    expect(harness.state.quitting).toBe(true);
    expect(harness.state.closeCalls).toBe(1);
    expect(harness.state.shutdownCalls).toBe(0);
  });

  it("keeps the backend alive when dirty unload is cancelled", () => {
    const harness = lifecycleHarness();
    harness.lifecycle.onCloseRequested(harness.closeEvent);
    harness.runDeferred();

    harness.lifecycle.onUnloadCancelled();

    expect(harness.state.quitting).toBe(false);
    expect(harness.state.shutdownCalls).toBe(0);
  });

  it("allows dirty unload discard after the deferred retry and shuts down once", () => {
    const harness = lifecycleHarness();
    harness.lifecycle.onCloseRequested(harness.closeEvent);
    harness.runDeferred();

    // The retry re-enters BrowserWindow.close with quitting set, so the second
    // close reaches beforeunload without another native-close prevention.
    harness.lifecycle.onCloseRequested(harness.closeEvent);
    harness.lifecycle.onWebContentsDestroyed();
    harness.lifecycle.onWindowClosed();

    expect(harness.prevented()).toBe(1);
    expect(harness.state.closeCalls).toBe(1);
    expect(harness.state.shutdownCalls).toBe(1);
  });

  it("invalidates a queued retry when another close attempt is cancelled", () => {
    const harness = lifecycleHarness();
    harness.lifecycle.onCloseRequested(harness.closeEvent);

    harness.lifecycle.onUnloadCancelled();
    harness.runDeferred();

    expect(harness.state.quitting).toBe(false);
    expect(harness.state.closeCalls).toBe(0);
    expect(harness.state.shutdownCalls).toBe(0);
  });

  it("begins shutdown once after WebContents destruction and window close", () => {
    const harness = lifecycleHarness();

    harness.lifecycle.onWebContentsDestroyed();
    harness.lifecycle.onWindowClosed();

    expect(harness.state.quitting).toBe(true);
    expect(harness.state.shutdownCalls).toBe(1);
  });

  it("handles WebContents destruction before any close request", () => {
    const harness = lifecycleHarness();

    harness.lifecycle.onWebContentsDestroyed();

    expect(harness.state.quitting).toBe(true);
    expect(harness.state.closeCalls).toBe(0);
    expect(harness.state.shutdownCalls).toBe(1);
  });

  it("allows an updater-owned close and still shuts down exactly once", () => {
    const harness = lifecycleHarness({ hideOnClose: true });
    harness.state.quitting = true;

    harness.lifecycle.onCloseRequested(harness.closeEvent);
    harness.lifecycle.onWebContentsDestroyed();
    harness.lifecycle.onWindowClosed();

    expect(harness.prevented()).toBe(0);
    expect(harness.state.hideCalls).toBe(0);
    expect(harness.deferred).toHaveLength(0);
    expect(harness.state.shutdownCalls).toBe(1);
  });

  it("shuts down instead of retrying when the native window is already gone", () => {
    const harness = lifecycleHarness();
    harness.lifecycle.onCloseRequested(harness.closeEvent);
    harness.state.windowDestroyed = true;

    harness.runDeferred();

    expect(harness.state.quitting).toBe(true);
    expect(harness.state.closeCalls).toBe(0);
    expect(harness.state.shutdownCalls).toBe(1);
  });
});
