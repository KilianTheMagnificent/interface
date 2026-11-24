import { describe, expect, it } from "vitest";
import {
  createStuckRendererRecovery, hideWindow, installWindowVisibility, paintWhileHidden, syncWindowVisibility,
} from "./window-visibility.js";

type VisibilityListener = () => void;

interface WebContentsLike {
  isDestroyed(): boolean;
  setBackgroundThrottling(value: boolean): void;
}

interface WindowLike {
  isDestroyed(): boolean;
  isVisible(): boolean;
  isMinimized(): boolean;
  on(event: string, callback: VisibilityListener): void;
  webContents: WebContentsLike;
}

interface FakeHandle {
  window: WindowLike;
  throttling: { value: boolean | undefined };
  emit(event: "show" | "hide" | "minimize" | "restore"): void;
  destroyWindow(): void;
  destroyWebContents(): void;
}

function createFakeWindow(options: { visible?: boolean; minimized?: boolean } = {}): FakeHandle {
  const state = {
    destroyed: false,
    visible: options.visible ?? true,
    minimized: options.minimized ?? false,
  };
  const throttling: { value: boolean | undefined } = { value: undefined };
  const wcDestroyed = { value: false };
  const listeners = new Map<string, Set<VisibilityListener>>();

  const webContents: WebContentsLike = {
    isDestroyed: () => wcDestroyed.value,
    setBackgroundThrottling: (value: boolean) => {
      if (wcDestroyed.value) throw new Error("Called destroyed webContents");
      throttling.value = value;
    },
  };

  const window: WindowLike = {
    isDestroyed: () => state.destroyed,
    isVisible: () => { if (state.destroyed || wcDestroyed.value) throw new Error("Read disposed window"); return state.visible; },
    isMinimized: () => { if (state.destroyed || wcDestroyed.value) throw new Error("Read disposed window"); return state.minimized; },
    on: (event: string, callback: VisibilityListener) => {
      const existing = listeners.get(event);
      if (existing) {
        existing.add(callback);
      } else {
        const set = new Set<VisibilityListener>();
        set.add(callback);
        listeners.set(event, set);
      }
    },
    webContents,
  };

  return {
    window,
    throttling,
    emit(event) {
      // Electron mutates the relevant flag before firing the event, so listeners
      // observe the post-transition state.
      if (event === "show") state.visible = true;
      else if (event === "hide") state.visible = false;
      else if (event === "minimize") state.minimized = true;
      else if (event === "restore") state.minimized = false;
      const set = listeners.get(event);
      if (set) {
        for (const cb of [...set]) cb();
      }
    },
    destroyWindow() {
      state.destroyed = true;
    },
    destroyWebContents() {
      wcDestroyed.value = true;
    },
  };
}

describe("installWindowVisibility", () => {
  it("unthrottles a window that starts visible and not minimized", () => {
    const h = createFakeWindow({ visible: true, minimized: false });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(false);
  });

  it("throttles a window that starts hidden", () => {
    const h = createFakeWindow({ visible: false });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(true);
  });

  it("synchronizes explicit native actions even when no event is delivered", () => {
    let visible = true;
    let throttled: boolean | undefined;
    const window = {
      isDestroyed: () => false, isVisible: () => visible, isMinimized: () => false,
      on: () => {}, webContents: { isDestroyed: () => false,
        setBackgroundThrottling: (value: boolean) => { throttled = value; } },
    };
    installWindowVisibility(window);
    expect(throttled).toBe(false);
    visible = false;
    syncWindowVisibility(window);
    expect(throttled).toBe(true);
    visible = true;
    syncWindowVisibility(window);
    expect(throttled).toBe(false);
  });

  it("throttles a window that starts minimized even while visible", () => {
    const h = createFakeWindow({ visible: true, minimized: true });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(true);
  });

  it("keeps a hands-free window painting although it is never shown", () => {
    const h = createFakeWindow({ visible: false });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(true);
    paintWhileHidden(h.window);
    expect(h.throttling.value).toBe(false);
    h.emit("hide");
    expect(h.throttling.value).toBe(false);
    h.emit("minimize");
    expect(h.throttling.value).toBe(false);
    const ordinary = createFakeWindow({ visible: false });
    installWindowVisibility(ordinary.window);
    expect(ordinary.throttling.value).toBe(true);
  });

  it("throttles on hide and unthrottles on show across repeated cycles", () => {
    const h = createFakeWindow({ visible: true });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(false);

    h.emit("hide");
    expect(h.throttling.value).toBe(true);

    h.emit("show");
    expect(h.throttling.value).toBe(false);

    h.emit("hide");
    expect(h.throttling.value).toBe(true);

    h.emit("show");
    expect(h.throttling.value).toBe(false);
  });
  it("throttles on minimize and unthrottles on restore", () => {
    const h = createFakeWindow({ visible: true });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(false);

    h.emit("minimize");
    expect(h.throttling.value).toBe(true);

    h.emit("restore");
    expect(h.throttling.value).toBe(false);
  });

  it("keeps a minimized window throttled if a show event arrives", () => {
    const h = createFakeWindow({ visible: true });
    installWindowVisibility(h.window);
    h.emit("minimize");
    expect(h.throttling.value).toBe(true);

    h.emit("show");
    expect(h.throttling.value).toBe(true);

    h.emit("restore");
    expect(h.throttling.value).toBe(false);
  });

  it("unthrottles on show without requiring any focus event", () => {
    const h = createFakeWindow({ visible: false });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(true);

    h.emit("show");
    expect(h.throttling.value).toBe(false);
  });

  it("does not touch throttling after the window is destroyed", () => {
    const h = createFakeWindow({ visible: false });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(true);

    h.destroyWindow();

    // The implementation must guard against reading state from a destroyed
    // window; subsequent show/hide/minimize/restore events must not change
    // the throttling value (a buggy impl would call setBackgroundThrottling).
    h.emit("show");
    expect(h.throttling.value).toBe(true);

    h.emit("hide");
    expect(h.throttling.value).toBe(true);

    h.emit("minimize");
    expect(h.throttling.value).toBe(true);

    h.emit("restore");
    expect(h.throttling.value).toBe(true);
  });

  it("does not touch throttling after webContents is destroyed", () => {
    const h = createFakeWindow({ visible: true });
    installWindowVisibility(h.window);
    expect(h.throttling.value).toBe(false);

    h.destroyWebContents();

    h.emit("hide");
    expect(h.throttling.value).toBe(false);

    h.emit("show");
    expect(h.throttling.value).toBe(false);
  });
});

function createRecoverableWindow() {
  const state = { visible: true, minimized: false, focused: true, pageHidden: false, throttled: false };
  const calls: string[] = [];
  const window = {
    isDestroyed: () => false,
    isVisible: () => state.visible,
    isMinimized: () => state.minimized,
    isFocused: () => state.focused,
    on: () => {},
    hide: () => { calls.push("hide"); state.visible = false; if (state.throttled) state.pageHidden = true; },
    show: () => { calls.push("show"); state.visible = true; state.pageHidden = false; },
    webContents: {
      isDestroyed: () => false,
      getOSProcessId: () => 42,
      setBackgroundThrottling: (value: boolean) => { calls.push(`throttle:${value}`); state.throttled = value; },
      executeJavaScript: async () => (state.pageHidden ? "hidden" : "visible"),
    },
  };
  return { window, state, calls };
}

describe("hideWindow", () => {
  it("allows throttling before hiding so Chromium actually hides the page", () => {
    const h = createRecoverableWindow();
    hideWindow(h.window);
    expect(h.calls).toEqual(["throttle:true", "hide", "throttle:true"]);
  });
});

describe("createStuckRendererRecovery", () => {
  function probeFor(load: { cores: number; idle: number }) {
    let at = 0;
    let seconds = 0;
    return {
      tick() { at += 60_000; seconds += load.cores * 60; },
      probe: { cpuSeconds: () => seconds, idleSeconds: () => load.idle, now: () => at },
    };
  }

  it("pulses the focused window once the renderer stays busy for ten checks", async () => {
    const h = createRecoverableWindow();
    const load = { cores: 1.05, idle: 5 };
    const clock = probeFor(load);
    const recover = createStuckRendererRecovery(h.window, clock.probe);
    expect(await recover()).toBe(false); // first check only records a baseline
    for (let check = 1; check < 10; check += 1) {
      clock.tick();
      expect(await recover()).toBe(false);
    }
    clock.tick();
    expect(await recover()).toBe(true);
    expect(h.calls).toEqual(["throttle:true", "hide", "throttle:true", "show", "throttle:false"]);
    expect(h.state).toMatchObject({ visible: true, pageHidden: false, throttled: false });

    // Cooldown: a still-busy renderer is not pulsed again for thirty minutes.
    h.calls.length = 0;
    for (let check = 0; check < 20; check += 1) {
      clock.tick();
      expect(await recover()).toBe(false);
    }
    expect(h.calls).toEqual([]);
  });

  it("waits for focus and idle input, and forgets load that stops", async () => {
    const h = createRecoverableWindow();
    const load = { cores: 0.9, idle: 0 };
    const clock = probeFor(load);
    const recover = createStuckRendererRecovery(h.window, clock.probe);
    await recover();
    h.state.focused = false;
    for (let check = 0; check < 12; check += 1) {
      clock.tick();
      expect(await recover()).toBe(false);
    }
    h.state.focused = true; // typing: still idle 0
    clock.tick();
    expect(await recover()).toBe(false);
    load.idle = 3;
    clock.tick();
    expect(await recover()).toBe(true);

    const quiet = createRecoverableWindow();
    const quietLoad = { cores: 0.9, idle: 10 };
    const quietClock = probeFor(quietLoad);
    const recoverQuiet = createStuckRendererRecovery(quiet.window, quietClock.probe);
    await recoverQuiet();
    for (let check = 0; check < 9; check += 1) { quietClock.tick(); await recoverQuiet(); }
    quietLoad.cores = 0.1;
    quietClock.tick();
    expect(await recoverQuiet()).toBe(false);
    quietLoad.cores = 0.9;
    quietClock.tick();
    expect(await recoverQuiet()).toBe(false);
    expect(quiet.calls).toEqual([]);
  });
});
