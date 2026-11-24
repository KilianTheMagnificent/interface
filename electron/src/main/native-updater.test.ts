import { describe, expect, it, vi } from "vitest";
import {
  DEFAULT_UPDATE_INTERVAL_MS,
  DEFAULT_UPDATE_STARTUP_DELAY_MS,
  FOREGROUND_CHECK_MIN_MS,
  NATIVE_UPDATER_CONFIGURE,
  NATIVE_UPDATER_EVENT,
  NativeUpdaterEffects,
  parseNativeUpdaterConfiguration,
  type NativeUpdateScheduler,
  type NativeUpdaterEventHandlers,
  type NativeUpdaterPort,
} from "./native-updater.js";

class FakeUpdater implements NativeUpdaterPort {
  autoDownload = false;
  autoInstallOnAppQuit = true;
  autoRunAppAfterInstall = false;
  disableDifferentialDownload = false;
  handlers: NativeUpdaterEventHandlers | null = null;
  readonly feeds: string[] = [];
  readonly installs: Array<[boolean, boolean]> = [];
  checks = 0;
  result: Awaited<ReturnType<NativeUpdaterPort["checkForUpdates"]>> = {
    isUpdateAvailable: false,
    updateInfo: { version: "0.2.0" },
  };
  setFeedUrl(url: string): void { this.feeds.push(url); }
  async checkForUpdates() { this.checks++; return this.result; }
  quitAndInstall(silent: boolean, forceRunAfter: boolean): void {
    this.installs.push([silent, forceRunAfter]);
  }
  subscribe(handlers: NativeUpdaterEventHandlers): () => void {
    this.handlers = handlers;
    return () => { this.handlers = null; };
  }
}

interface Timer { callback(): void; delay: number; cancelled: boolean }
class FakeScheduler implements NativeUpdateScheduler {
  readonly timers: Timer[] = [];
  setTimeout(callback: () => void, delay: number): unknown {
    const timer: Timer = {
      callback: () => { timer.cancelled = true; callback(); },
      delay,
      cancelled: false,
    };
    this.timers.push(timer);
    return timer;
  }
  clearTimeout(handle: unknown): void { (handle as Timer).cancelled = true; }
  active(): Timer[] { return this.timers.filter((timer) => !timer.cancelled); }
}

const CONFIG = {
  enabled: true,
  feedUrl: "http://127.0.0.1:43119/",
  startupDelayMs: 15_000,
  intervalMs: 21_600_000,
};

describe("native updater effects", () => {
  it("accepts only a bounded private native configuration", () => {
    expect(parseNativeUpdaterConfiguration(CONFIG)).toEqual(CONFIG);
    expect(parseNativeUpdaterConfiguration({
      enabled: false, feedUrl: null, startupDelayMs: -1, intervalMs: Infinity,
    })).toEqual({
      enabled: false,
      feedUrl: null,
      startupDelayMs: DEFAULT_UPDATE_STARTUP_DELAY_MS,
      intervalMs: DEFAULT_UPDATE_INTERVAL_MS,
    });
    for (const feedUrl of [
      "http://updates.example.test/",
      "https://user:secret@example.test/",
      "https://example.test/?token=secret",
    ]) {
      expect(() => parseNativeUpdaterConfiguration({ ...CONFIG, feedUrl })).toThrow(/valid/);
    }
    expect(() => parseNativeUpdaterConfiguration({ ...CONFIG, enabled: false }))
      .toThrow(/disabled/);
    expect(DEFAULT_UPDATE_INTERVAL_MS).toBe(15 * 60_000);
    expect(FOREGROUND_CHECK_MIN_MS).toBe(30_000);
  });

  it("never re-copies a staged update into the differential cache", () => {
    const updater = new FakeUpdater();
    const invoke = vi.fn(async () => CONFIG);
    new NativeUpdaterEffects({ host: { invoke }, updater });
    expect(updater.disableDifferentialDownload).toBe(true);
  });

  it("keeps policy in Rust and applies only the three native updater flags and feed", async () => {
    const updater = new FakeUpdater();
    const invoke = vi.fn(async (operation: string) => {
      expect(operation).toBe(NATIVE_UPDATER_CONFIGURE);
      return CONFIG;
    });
    const effects = new NativeUpdaterEffects({ host: { invoke }, updater });
    expect(updater.autoDownload).toBe(true);
    expect(updater.autoInstallOnAppQuit).toBe(false);
    expect(updater.autoRunAppAfterInstall).toBe(true);
    await expect(effects.configure({
      isPackaged: true,
      currentVersion: "0.2.0",
      appBundlePath: "/Applications/Interface.app",
    })).resolves.toEqual(CONFIG);
    expect(updater.feeds).toEqual([CONFIG.feedUrl]);
    effects.dispose();
  });

  it("asks Rust for its configuration at once while electron-updater is still loading", async () => {
    const updater = new FakeUpdater();
    let load!: (port: NativeUpdaterPort) => void;
    const operations: string[] = [];
    const effects = new NativeUpdaterEffects({
      host: {
        async invoke(operation) {
          operations.push(operation);
          return CONFIG;
        },
      },
      updater: new Promise<NativeUpdaterPort>((resolve) => { load = resolve; }),
    });
    let configured = false;
    const configuring = effects.configure({
      isPackaged: true, currentVersion: "0.2.0", appBundlePath: "/Applications/Interface.app",
    }).then((value) => { configured = true; return value; });
    await new Promise(resolve => { setTimeout(resolve, 0); });
    // Rust's signature discovery starts on this call; it does not wait for the module.
    expect(operations).toEqual([NATIVE_UPDATER_CONFIGURE]);
    expect(configured).toBe(false);
    expect(updater.handlers).toBeNull();
    await effects.check();
    await expect(effects.install()).resolves.toBe(false);
    expect(updater.checks).toBe(0);
    expect(updater.installs).toEqual([]);

    load(updater);
    await expect(configuring).resolves.toEqual(CONFIG);
    expect(updater.autoDownload).toBe(true);
    expect(updater.autoInstallOnAppQuit).toBe(false);
    expect(updater.autoRunAppAfterInstall).toBe(true);
    expect(updater.handlers).not.toBeNull();
    expect(updater.feeds).toEqual([CONFIG.feedUrl]);
    await effects.check();
    expect(updater.checks).toBe(1);
    effects.dispose();
    expect(updater.handlers).toBeNull();
  });

  it("fails configuration when electron-updater cannot load, and binds nothing after dispose", async () => {
    const host = { async invoke(): Promise<unknown> { return CONFIG; } };
    const failed = new NativeUpdaterEffects({
      host, updater: Promise.reject(new Error("Cannot find package 'electron-updater'")),
    });
    await expect(failed.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null }))
      .rejects.toThrow("Cannot find package 'electron-updater'");
    await expect(failed.check()).resolves.toBeUndefined();
    failed.dispose();

    const updater = new FakeUpdater();
    let load!: (port: NativeUpdaterPort) => void;
    const disposed = new NativeUpdaterEffects({
      host, updater: new Promise<NativeUpdaterPort>((resolve) => { load = resolve; }),
    });
    disposed.dispose();
    load(updater);
    await new Promise(resolve => { setTimeout(resolve, 0); });
    expect(updater.handlers).toBeNull();
    expect(updater.autoDownload).toBe(false);
  });

  it("serializes native events back through the Rust owner without leaking errors", async () => {
    const updater = new FakeUpdater();
    const events: unknown[] = [];
    const host = {
      async invoke(operation: string, payload: unknown): Promise<unknown> {
        if (operation === NATIVE_UPDATER_CONFIGURE) return CONFIG;
        expect(operation).toBe(NATIVE_UPDATER_EVENT);
        events.push(payload);
        return null;
      },
    };
    const effects = new NativeUpdaterEffects({ host, updater });
    await effects.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null });
    updater.handlers?.checking();
    updater.handlers?.available({ version: "0.3.0" });
    updater.handlers?.progress({ percent: 42.5 });
    updater.handlers?.downloaded({ version: "0.3.0", downloadedFile: "/tmp/update.zip" });
    updater.handlers?.error();
    await effects.check();
    expect(events).toEqual([
      { kind: "checking" },
      { kind: "available", version: "0.3.0" },
      { kind: "progress", percent: 42.5 },
      { kind: "downloaded", version: "0.3.0", downloadedFile: "/tmp/update.zip" },
      { kind: "error" },
      { kind: "checkResult", outcome: "notAvailable", version: "0.2.0" },
    ]);
    effects.dispose();
  });

  it("deduplicates checks, reschedules periodically, and preserves explicit install flags", async () => {
    const updater = new FakeUpdater();
    const scheduler = new FakeScheduler();
    let resolve!: (value: typeof updater.result) => void;
    updater.checkForUpdates = vi.fn(() => new Promise<typeof updater.result>(
      (done) => { resolve = done; },
    ));
    const host = {
      async invoke(operation: string): Promise<unknown> {
        return operation === NATIVE_UPDATER_CONFIGURE ? CONFIG : null;
      },
    };
    const effects = new NativeUpdaterEffects({ host, updater, scheduler });
    await effects.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null });
    effects.start();
    expect(scheduler.active().map((timer) => timer.delay)).toEqual([CONFIG.startupDelayMs]);
    scheduler.active()[0]?.callback();
    const concurrent = effects.check();
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(1);
    resolve(updater.result);
    await concurrent;
    await Promise.resolve();
    expect(scheduler.active().map((timer) => timer.delay)).toEqual([CONFIG.intervalMs]);
    await expect(effects.install()).resolves.toBe(true);
    expect(updater.installs).toEqual([[false, true]]);
    effects.dispose();
  });

  it("does not check on focus until the foreground cooldown has elapsed", async () => {
    const updater = new FakeUpdater();
    const scheduler = new FakeScheduler();
    let now = 1_000;
    const host = {
      async invoke(operation: string): Promise<unknown> {
        return operation === NATIVE_UPDATER_CONFIGURE ? CONFIG : null;
      },
    };
    const effects = new NativeUpdaterEffects({
      host, updater, scheduler, now: () => now,
    });
    await effects.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null });
    effects.start();
    await effects.checkOnForeground();
    expect(updater.checks).toBe(0);

    now += FOREGROUND_CHECK_MIN_MS - 1;
    await effects.checkOnForeground();
    expect(updater.checks).toBe(0);

    now += 1;
    await effects.checkOnForeground();
    expect(updater.checks).toBe(1);
    effects.dispose();
  });

  it("checks immediately when the local feed directory changes, even inside the cooldown", async () => {
    const updater = new FakeUpdater();
    const scheduler = new FakeScheduler();
    let now = 1_000;
    const watched: { fire: (() => void) | null } = { fire: null };
    const host = {
      async invoke(operation: string): Promise<unknown> {
        return operation === NATIVE_UPDATER_CONFIGURE ? CONFIG : null;
      },
    };
    const effects = new NativeUpdaterEffects({
      host, updater, scheduler, now: () => now,
      watch: (_path, listener) => {
        watched.fire = listener;
        return () => { watched.fire = null; };
      },
    });
    await effects.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null });
    effects.start();
    effects.watchLocalFeed("/tmp/update-feed");
    expect(updater.checks).toBe(0);
    watched.fire?.();
    await Promise.resolve();
    expect(updater.checks).toBe(1);
    effects.dispose();
    expect(watched.fire).toBeNull();
  });

  it("checks once more when the feed changes during an in-flight check", async () => {
    const updater = new FakeUpdater();
    const scheduler = new FakeScheduler();
    const pending: Array<(value: typeof updater.result) => void> = [];
    updater.checkForUpdates = vi.fn(() => new Promise<typeof updater.result>(
      (done) => { pending.push(done); },
    ));
    const watched: { fire: (() => void) | null } = { fire: null };
    const host = {
      async invoke(operation: string): Promise<unknown> {
        return operation === NATIVE_UPDATER_CONFIGURE ? CONFIG : null;
      },
    };
    const effects = new NativeUpdaterEffects({
      host, updater, scheduler,
      watch: (_path, listener) => {
        watched.fire = listener;
        return () => { watched.fire = null; };
      },
    });
    await effects.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null });
    effects.start();
    effects.watchLocalFeed("/tmp/update-feed");
    // A stage renames each artifact into the feed, the manifest last.
    watched.fire?.();
    watched.fire?.();
    watched.fire?.();
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(1);
    pending.shift()?.(updater.result);
    await vi.waitFor(() => { expect(updater.checkForUpdates).toHaveBeenCalledTimes(2); });
    pending.shift()?.(updater.result);
    await new Promise(resolve => { setTimeout(resolve, 0); });
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(2);

    // Other callers still share the in-flight check without a follow-up.
    const shared = effects.check();
    void effects.check();
    pending.shift()?.(updater.result);
    await shared;
    await new Promise(resolve => { setTimeout(resolve, 0); });
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(3);

    // A feed change pending when the watch stops is dropped with it.
    const stopped = effects.check();
    watched.fire?.();
    effects.stop();
    pending.shift()?.(updater.result);
    await stopped;
    await new Promise(resolve => { setTimeout(resolve, 0); });
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(4);
    effects.dispose();
  });

  it("settles background checks after the Rust owner stops, without an unhandled rejection", async () => {
    const updater = new FakeUpdater();
    const scheduler = new FakeScheduler();
    let now = 1_000;
    let hostStopped = false;
    const watched: { fire: (() => void) | null } = { fire: null };
    const warnings: string[] = [];
    const unhandled: unknown[] = [];
    const onUnhandled = (reason: unknown): void => { unhandled.push(reason); };
    process.on("unhandledRejection", onUnhandled);
    try {
      const effects = new NativeUpdaterEffects({
        updater, scheduler, now: () => now,
        warn: message => { warnings.push(message); },
        watch: (_path, listener) => {
          watched.fire = listener;
          return () => { watched.fire = null; };
        },
        host: {
          async invoke(operation) {
            if (operation === NATIVE_UPDATER_CONFIGURE) return CONFIG;
            if (hostStopped) throw new Error("The Rust host stopped.");
            return null;
          },
        },
      });
      await effects.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null });
      effects.start();
      effects.watchLocalFeed("/tmp/update-feed");
      hostStopped = true;

      // The scheduled tick settles and still reschedules.
      scheduler.active()[0]?.callback();
      await vi.waitFor(() => {
        expect(scheduler.active().map(timer => timer.delay)).toEqual([CONFIG.intervalMs]);
      });
      // A focus after the cooldown, and one sharing an in-flight check.
      now += FOREGROUND_CHECK_MIN_MS;
      await expect(effects.checkOnForeground()).resolves.toBeUndefined();
      now += FOREGROUND_CHECK_MIN_MS;
      watched.fire?.();
      await expect(effects.checkOnForeground()).resolves.toBeUndefined();
      await new Promise(resolve => { setTimeout(resolve, 10); });
      expect(updater.checks).toBe(3);
      expect(warnings.length).toBe(3);
      expect(unhandled).toEqual([]);

      // The renderer's own check still reports the failure to its caller.
      await expect(effects.check()).rejects.toThrow("The Rust host stopped.");
      effects.dispose();
    } finally {
      process.off("unhandledRejection", onUnhandled);
    }
  });

  it("shares an in-flight check between a timer and a foreground nudge", async () => {
    const updater = new FakeUpdater();
    const scheduler = new FakeScheduler();
    let resolve!: (value: typeof updater.result) => void;
    updater.checkForUpdates = vi.fn(() => new Promise<typeof updater.result>(
      (done) => { resolve = done; },
    ));
    const host = {
      async invoke(operation: string): Promise<unknown> {
        return operation === NATIVE_UPDATER_CONFIGURE ? CONFIG : null;
      },
    };
    const effects = new NativeUpdaterEffects({ host, updater, scheduler });
    await effects.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null });
    effects.start();
    scheduler.active()[0]?.callback();
    const foreground = effects.checkOnForeground();
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(1);
    resolve(updater.result);
    await foreground;
    effects.dispose();
  });

  it("reports a synchronous installer failure to Rust and returns false", async () => {
    const updater = new FakeUpdater();
    updater.quitAndInstall = () => { throw new Error("native details must not cross IPC"); };
    const invoked: Array<[string, unknown]> = [];
    const effects = new NativeUpdaterEffects({
      updater,
      host: {
        async invoke(operation, payload) {
          if (operation === NATIVE_UPDATER_CONFIGURE) return CONFIG;
          invoked.push([operation, payload]);
          return null;
        },
      },
    });
    await effects.configure({ isPackaged: true, currentVersion: "0.2.0", appBundlePath: null });
    await expect(effects.install()).resolves.toBe(false);
    expect(invoked).toEqual([[NATIVE_UPDATER_EVENT, { kind: "error" }]]);
    effects.dispose();
  });

  it("still returns false when both the installer and stopped Rust owner reject", async () => {
    const updater = new FakeUpdater();
    updater.quitAndInstall = () => { throw new Error("installer failed"); };
    const warnings: string[] = [];
    const effects = new NativeUpdaterEffects({
      updater,
      host: { async invoke() { throw new Error("host stopped"); } },
      warn: message => { warnings.push(message); },
    });
    await expect(effects.install()).resolves.toBe(false);
    expect(warnings).toEqual([
      "[main] the Rust updater owner rejected a native updater event",
    ]);
    effects.dispose();
  });
});
