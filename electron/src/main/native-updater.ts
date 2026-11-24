import { watch as watchDirectory } from "node:fs";
import type {
  AppUpdater, ProgressInfo, UpdateDownloadedEvent, UpdateInfo,
  UpdateCheckResult,
} from "electron-updater";
import { plainObject } from "./json.js";

export const NATIVE_UPDATER_CONFIGURE = "iface:nativeUpdaterConfigure";
export const NATIVE_UPDATER_EVENT = "iface:nativeUpdaterEvent";

export const DEFAULT_UPDATE_STARTUP_DELAY_MS = 15_000;
export const DEFAULT_UPDATE_INTERVAL_MS = 15 * 60_000;
export const FOREGROUND_CHECK_MIN_MS = 30_000;
const MAX_UPDATE_STARTUP_DELAY_MS = 5 * 60_000;
const MIN_UPDATE_INTERVAL_MS = 15 * 60_000;
const MAX_UPDATE_INTERVAL_MS = 24 * 60 * 60_000;

export interface NativeUpdaterHost {
  invoke(operation: string, payload: unknown): Promise<unknown>;
}

export interface NativeUpdaterConfigurationInput {
  readonly isPackaged: boolean;
  readonly currentVersion: string;
  readonly appBundlePath: string | null;
}

export interface NativeUpdaterConfiguration {
  readonly enabled: boolean;
  readonly feedUrl: string | null;
  readonly startupDelayMs: number;
  readonly intervalMs: number;
}

interface NativeUpdateInfo {
  readonly version?: string;
  readonly downloadedFile?: string;
}

interface NativeUpdateCheckResult {
  readonly isUpdateAvailable: boolean;
  readonly updateInfo?: NativeUpdateInfo;
}

export interface NativeUpdaterPort {
  autoDownload: boolean;
  autoInstallOnAppQuit: boolean;
  autoRunAppAfterInstall: boolean;
  disableDifferentialDownload: boolean;
  setFeedUrl(url: string): void;
  checkForUpdates(): Promise<NativeUpdateCheckResult | null>;
  quitAndInstall(isSilent: boolean, forceRunAfter: boolean): void;
  subscribe(handlers: NativeUpdaterEventHandlers): () => void;
}

export interface NativeUpdaterEventHandlers {
  checking(): void;
  available(info: NativeUpdateInfo): void;
  notAvailable(info: NativeUpdateInfo): void;
  progress(info: { readonly percent?: number }): void;
  downloaded(info: NativeUpdateInfo): void;
  error(): void;
}

export interface NativeUpdateScheduler {
  setTimeout(callback: () => void, delayMs: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface NativeFeedWatch {
  watch(path: string, listener: () => void): () => void;
}

export interface NativeUpdaterEffectsOptions {
  readonly host: NativeUpdaterHost;
  /** A promise while electron-updater is still loading: `configure` waits for
   * it, and no check or install can start before `configure` has finished. */
  readonly updater: NativeUpdaterPort | Promise<NativeUpdaterPort>;
  readonly scheduler?: NativeUpdateScheduler;
  readonly watch?: NativeFeedWatch["watch"];
  readonly now?: () => number;
  readonly warn?: (message: string) => void;
}

const systemScheduler: NativeUpdateScheduler = {
  setTimeout(callback, delayMs) { return setTimeout(callback, delayMs); },
  clearTimeout(handle) { clearTimeout(handle as ReturnType<typeof setTimeout>); },
};

const systemFeedWatch: NativeFeedWatch["watch"] = (path, listener) => {
  try {
    const watcher = watchDirectory(path, { persistent: false }, () => { listener(); });
    watcher.on("error", () => { /* the next check interval still covers a dead watch */ });
    return () => { watcher.close(); };
  } catch {
    return () => undefined;
  }
};

function boundedDelay(value: unknown, fallback: number, minimum: number, maximum: number): number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= minimum
    && value <= maximum
    ? value
    : fallback;
}

function validFeedUrl(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0 || value.length > 2_048) return null;
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    return null;
  }
  if (parsed.username.length > 0 || parsed.password.length > 0
      || parsed.search.length > 0 || parsed.hash.length > 0) return null;
  const loopback = parsed.protocol === "http:"
    && ["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname.toLowerCase());
  return parsed.protocol === "https:" || loopback ? parsed.toString() : null;
}

export function parseNativeUpdaterConfiguration(value: unknown): NativeUpdaterConfiguration {
  const input = plainObject(value);
  if (input === undefined || typeof input.enabled !== "boolean") {
    throw new TypeError("The Rust host returned an invalid updater configuration.");
  }
  const feedUrl = input.feedUrl === null ? null : validFeedUrl(input.feedUrl);
  if (input.enabled && feedUrl === null) {
    throw new TypeError("The Rust host enabled updates without a valid native feed.");
  }
  if (!input.enabled && input.feedUrl !== null) {
    throw new TypeError("The Rust host returned a feed for a disabled updater.");
  }
  return {
    enabled: input.enabled,
    feedUrl,
    startupDelayMs: boundedDelay(input.startupDelayMs, DEFAULT_UPDATE_STARTUP_DELAY_MS,
      0, MAX_UPDATE_STARTUP_DELAY_MS),
    intervalMs: boundedDelay(input.intervalMs, DEFAULT_UPDATE_INTERVAL_MS,
      MIN_UPDATE_INTERVAL_MS, MAX_UPDATE_INTERVAL_MS),
  };
}

/**
 * Adapts electron-updater to the deliberately small native-effects boundary.
 * Rust owns feed policy, state transitions, signature verification and all
 * renderer-visible results; this adapter only performs Electron-only effects.
 */
export function createNativeUpdaterPort(updater: AppUpdater): NativeUpdaterPort {
  return {
    get autoDownload() { return updater.autoDownload; },
    set autoDownload(value: boolean) { updater.autoDownload = value; },
    get autoInstallOnAppQuit() { return updater.autoInstallOnAppQuit; },
    set autoInstallOnAppQuit(value: boolean) { updater.autoInstallOnAppQuit = value; },
    get autoRunAppAfterInstall() { return updater.autoRunAppAfterInstall; },
    set autoRunAppAfterInstall(value: boolean) { updater.autoRunAppAfterInstall = value; },
    get disableDifferentialDownload() { return updater.disableDifferentialDownload; },
    set disableDifferentialDownload(value: boolean) { updater.disableDifferentialDownload = value; },
    setFeedUrl(url) { updater.setFeedURL({ provider: "generic", url }); },
    async checkForUpdates() {
      const result: UpdateCheckResult | null = await updater.checkForUpdates();
      return result === null ? null : {
        isUpdateAvailable: result.isUpdateAvailable,
        updateInfo: { version: result.updateInfo.version },
      };
    },
    quitAndInstall(isSilent, forceRunAfter) {
      updater.quitAndInstall(isSilent, forceRunAfter);
    },
    subscribe(handlers) {
      const checking = (): void => { handlers.checking(); };
      const available = (info: UpdateInfo): void => { handlers.available(info); };
      const notAvailable = (info: UpdateInfo): void => { handlers.notAvailable(info); };
      const progress = (info: ProgressInfo): void => { handlers.progress(info); };
      const downloaded = (info: UpdateDownloadedEvent): void => {
        handlers.downloaded({ version: info.version, downloadedFile: info.downloadedFile });
      };
      const error = (): void => { handlers.error(); };
      updater.on("checking-for-update", checking);
      updater.on("update-available", available);
      updater.on("update-not-available", notAvailable);
      updater.on("download-progress", progress);
      updater.on("update-downloaded", downloaded);
      updater.on("error", error);
      return () => {
        updater.removeListener("checking-for-update", checking);
        updater.removeListener("update-available", available);
        updater.removeListener("update-not-available", notAvailable);
        updater.removeListener("download-progress", progress);
        updater.removeListener("update-downloaded", downloaded);
        updater.removeListener("error", error);
      };
    },
  };
}

export class NativeUpdaterEffects {
  private readonly scheduler: NativeUpdateScheduler;
  private readonly now: () => number;
  private readonly warn: (message: string) => void;
  private configuration: NativeUpdaterConfiguration | null = null;
  private readonly portReady: Promise<NativeUpdaterPort>;
  private port: NativeUpdaterPort | null = null;
  private disposed = false;
  private removeListeners: (() => void) | null = null;
  private timer: unknown | null = null;
  private started = false;
  private lastCheckAt: number | null = null;
  private checkInFlight: Promise<void> | null = null;
  private eventTail: Promise<void> = Promise.resolve();
  private removeFeedWatch: (() => void) | null = null;
  private feedChangedDuringCheck = false;
  private readonly watch: NativeFeedWatch["watch"];

  constructor(private readonly options: NativeUpdaterEffectsOptions) {
    this.scheduler = options.scheduler ?? systemScheduler;
    this.watch = options.watch ?? systemFeedWatch;
    this.now = options.now ?? Date.now;
    this.warn = options.warn ?? ((message) => { console.warn(message); });
    const updater = options.updater;
    if (updater instanceof Promise) {
      this.portReady = updater.then((port) => {
        if (!this.disposed) this.bind(port);
        return port;
      });
      // Rethrown by configure(), which turns it into a startup failure.
      this.portReady.catch(() => undefined);
    } else {
      this.bind(updater);
      this.portReady = Promise.resolve(updater);
    }
  }

  private bind(port: NativeUpdaterPort): void {
    port.autoDownload = true;
    port.autoInstallOnAppQuit = false;
    port.autoRunAppAfterInstall = true;
    // The feed is the loopback host, so a differential cache saves no download. With it on, every
    // check that finds an already-staged update copies the whole zip into the cache again (about
    // 133 MiB of writes every few minutes until the app restarts).
    port.disableDifferentialDownload = true;
    this.port = port;
    this.removeListeners = port.subscribe({
      checking: () => { this.enqueueEvent({ kind: "checking" }); },
      available: (info) => {
        this.enqueueEvent({ kind: "available", version: info.version ?? null });
      },
      notAvailable: () => { this.enqueueEvent({ kind: "notAvailable" }); },
      progress: (info) => {
        this.enqueueEvent({ kind: "progress", percent: info.percent ?? null });
      },
      downloaded: (info) => {
        this.enqueueEvent({
          kind: "downloaded",
          version: info.version ?? null,
          downloadedFile: info.downloadedFile ?? null,
        });
      },
      error: () => { this.enqueueEvent({ kind: "error" }); },
    });
  }

  async configure(input: NativeUpdaterConfigurationInput): Promise<NativeUpdaterConfiguration> {
    const value = await this.options.host.invoke(NATIVE_UPDATER_CONFIGURE, input);
    const configuration = parseNativeUpdaterConfiguration(value);
    const port = await this.portReady;
    this.cancelTimer();
    this.configuration = configuration;
    if (configuration.enabled && configuration.feedUrl !== null) {
      port.setFeedUrl(configuration.feedUrl);
    }
    if (this.started) this.schedule(configuration.startupDelayMs);
    return configuration;
  }

  start(): void {
    if (this.started) return;
    this.started = true;
    this.lastCheckAt = this.now();
    const configuration = this.configuration;
    if (configuration !== null) this.schedule(configuration.startupDelayMs);
  }

  stop(): void {
    this.started = false;
    this.cancelTimer();
    this.stopFeedWatch();
  }

  watchLocalFeed(directory: string): void {
    this.stopFeedWatch();
    if (this.configuration?.enabled !== true || directory.length === 0) return;
    this.removeFeedWatch = this.watch(directory, () => { this.feedChanged(); });
  }

  dispose(): void {
    this.disposed = true;
    this.stop();
    this.removeListeners?.();
    this.removeListeners = null;
  }

  private stopFeedWatch(): void {
    this.removeFeedWatch?.();
    this.removeFeedWatch = null;
    this.feedChangedDuringCheck = false;
  }

  /** A stage renames each artifact into the feed, the manifest last, so a
   * change can land while a check begun by an earlier rename is still reading
   * the old manifest. That check cannot see it, so one more follows it. */
  private feedChanged(): void {
    if (this.checkInFlight !== null) this.feedChangedDuringCheck = true;
    else void this.checkInBackground();
  }

  checkOnForeground(): Promise<void> {
    if (!this.started || this.configuration?.enabled !== true) return Promise.resolve();
    if (this.checkInFlight !== null) return this.checkInFlight.catch(() => undefined);
    if (this.lastCheckAt !== null && this.now() - this.lastCheckAt < FOREGROUND_CHECK_MIN_MS) {
      return Promise.resolve();
    }
    return this.checkInBackground();
  }

  check(): Promise<void> {
    if (this.checkInFlight !== null) return this.checkInFlight;
    // A configuration exists only once configure() has bound the port.
    const port = this.port;
    if (port === null || this.configuration?.enabled !== true) return Promise.resolve();
    this.lastCheckAt = this.now();
    let task: Promise<void>;
    task = port.checkForUpdates()
      .then(async (result) => {
        await this.sendEvent(result === null
          ? { kind: "checkResult", outcome: "error" }
          : {
              kind: "checkResult",
              outcome: result.isUpdateAvailable ? "available" : "notAvailable",
              version: result.updateInfo?.version ?? null,
            });
      }, async () => {
        await this.sendEvent({ kind: "checkResult", outcome: "error" });
      })
      .finally(() => {
        if (this.checkInFlight !== task) return;
        this.checkInFlight = null;
        if (!this.feedChangedDuringCheck) return;
        this.feedChangedDuringCheck = false;
        void this.checkInBackground();
      });
    this.checkInFlight = task;
    return task;
  }

  /** Only the renderer's own check has a caller to hand a failure to. For the
   * timer, focus and feed checks the event tail has already warned; letting
   * them reject would reach the main process's unhandledRejection handler and
   * its modal error box on every focus and tick once the host has stopped. */
  private checkInBackground(): Promise<void> {
    return this.check().catch(() => undefined);
  }

  async install(): Promise<boolean> {
    try {
      if (this.port === null) throw new Error("The native updater has not loaded.");
      this.port.quitAndInstall(false, true);
      return true;
    } catch {
      try {
        await this.sendEvent({ kind: "error" });
      } catch {
        // sendEvent's serialized tail already records the stopped owner; the
        // native install result must still settle for the shutdown controller.
      }
      return false;
    }
  }

  private enqueueEvent(payload: Record<string, unknown>): void {
    void this.sendEvent(payload);
  }

  private sendEvent(payload: Record<string, unknown>): Promise<void> {
    const next = this.eventTail.then(async () => {
      await this.options.host.invoke(NATIVE_UPDATER_EVENT, payload);
    });
    this.eventTail = next.catch(() => {
      this.warn("[main] the Rust updater owner rejected a native updater event");
    });
    return next;
  }

  private schedule(delayMs: number): void {
    this.cancelTimer();
    const configuration = this.configuration;
    if (!this.started || configuration?.enabled !== true) return;
    this.timer = this.scheduler.setTimeout(() => {
      this.timer = null;
      void this.checkInBackground().finally(() => {
        if (this.started && this.configuration?.enabled === true) {
          this.schedule(this.configuration.intervalMs);
        }
      });
    }, delayMs);
  }

  private cancelTimer(): void {
    if (this.timer === null) return;
    this.scheduler.clearTimeout(this.timer);
    this.timer = null;
  }
}
