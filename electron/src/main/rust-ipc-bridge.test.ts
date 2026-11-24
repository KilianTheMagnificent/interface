import { describe, expect, it, vi } from "vitest";
import { installRustIpcBridge, rendererPushArguments } from "./rust-ipc-bridge.js";
import type { HostPushFrame } from "./rust-host-client.js";
import { CHANNELS } from "../shared/ipc.js";

describe("Rust IPC bridge", () => {
  it("registers all requests, translates arguments, and removes every handler", async () => {
    const handlers = new Map<string, (event: unknown, ...args: unknown[]) => unknown>();
    const removed: string[] = [];
    let push: ((frame: HostPushFrame) => void) | undefined;
    const invoke = vi.fn(async () => ({ key: "codex#one" }));
    const removePush = vi.fn();
    const cleanup = installRustIpcBridge({
      host: {
        invoke,
        onPush(listener) { push = listener; return removePush; },
      },
      ipcMain: {
        handle(channel, listener) { handlers.set(channel, listener); },
        removeHandler(channel) { removed.push(channel); handlers.delete(channel); },
      },
      targets: () => [],
    });

    expect(handlers.size).toBe(127);
    const listProfiles = handlers.get(CHANNELS.listTerminalLaunchProfiles);
    expect(listProfiles).toBeDefined();
    await expect(listProfiles?.({})).resolves.toEqual({ key: "codex#one" });
    expect(invoke).toHaveBeenCalledWith(CHANNELS.listTerminalLaunchProfiles, {});
    const send = handlers.get(CHANNELS.send);
    expect(send).toBeDefined();
    await expect(send?.({}, "codex#one", "hello")).resolves.toEqual({ key: "codex#one" });
    expect(invoke).toHaveBeenCalledWith(CHANNELS.send,
      { sessionKey: "codex#one", text: "hello" });
    expect(push).toBeTypeOf("function");

    cleanup();
    expect(removePush).toHaveBeenCalledOnce();
    expect(removed).toHaveLength(127);
    expect(handlers.size).toBe(0);
  });

  it("forwards event pushes with legacy two-argument callback arity", () => {
    expect(rendererPushArguments({
      kind: "push",
      version: 9,
      operation: CHANNELS.event,
      payload: { key: "claude#one", event: { kind: "status.changed", status: "idle" } },
    })).toEqual([
      "claude#one",
      { kind: "status.changed", status: "idle" },
    ]);
    expect(() => rendererPushArguments({
      kind: "push", version: 9, operation: CHANNELS.event, payload: { key: "claude#one" },
    })).toThrow(/requires a \{key,event\}/);
  });

  it("rejects an untrusted renderer before encoding or invoking Rust", async () => {
    const handlers = new Map<string, (event: unknown, ...args: unknown[]) => unknown>();
    const invoke = vi.fn(async () => undefined);
    const cleanup = installRustIpcBridge({
      host: { invoke, onPush: () => () => undefined },
      ipcMain: {
        handle(channel, listener) { handlers.set(channel, listener); },
        removeHandler(channel) { handlers.delete(channel); },
      },
      targets: () => [],
      authorize: () => false,
    });
    await expect(handlers.get(CHANNELS.send)?.({}, "codex#one", "hello"))
      .rejects.toThrow("Unauthorized renderer IPC.");
    expect(invoke).not.toHaveBeenCalled();
    cleanup();
  });

  it("lets the Electron shell inject native context without changing preload calls", async () => {
    const handlers = new Map<string, (event: unknown, ...args: unknown[]) => unknown>();
    const invoke = vi.fn(async () => ({ preference: "dark", resolved: "dark" }));
    const cleanup = installRustIpcBridge({
      host: { invoke, onPush: () => () => undefined },
      ipcMain: {
        handle(channel, listener) { handlers.set(channel, listener); },
        removeHandler(channel) { handlers.delete(channel); },
      },
      targets: () => [],
      beforeInvoke: (operation, payload) => operation === CHANNELS.setAppearance
        ? { ...(payload as object), systemDark: true }
        : payload,
    });
    await handlers.get(CHANNELS.setAppearance)?.({}, "dark");
    expect(invoke).toHaveBeenCalledWith(CHANNELS.setAppearance,
      { preference: "dark", systemDark: true });
    cleanup();
  });

  it.each([
    CHANNELS.getUpdateState,
    CHANNELS.checkForUpdates,
    CHANNELS.restartToUpdate,
  ])("waits for a native gate before invoking Rust for %s", async (operation) => {
    const handlers = new Map<string, (event: unknown, ...args: unknown[]) => unknown>();
    let openGate!: () => void;
    const gate = new Promise<void>((resolve) => { openGate = resolve; });
    const invoke = vi.fn(async () => ({ phase: "disabled" }));
    const beforeInvoke = vi.fn(async (_operation: string, payload: unknown) => {
      await gate;
      return payload;
    });
    const cleanup = installRustIpcBridge({
      host: { invoke, onPush: () => () => undefined },
      ipcMain: {
        handle(channel, listener) { handlers.set(channel, listener); },
        removeHandler(channel) { handlers.delete(channel); },
      },
      targets: () => [],
      beforeInvoke,
    });

    const request = Promise.resolve(handlers.get(operation)?.({}));
    await Promise.resolve();
    expect(beforeInvoke).toHaveBeenCalledWith(operation, {});
    expect(invoke).not.toHaveBeenCalled();
    openGate();
    await expect(request).resolves.toEqual({ phase: "disabled" });
    expect(invoke).toHaveBeenCalledWith(operation, {});
    cleanup();
  });

  it("waits for the native post-invoke effect before resolving to the renderer", async () => {
    const handlers = new Map<string, (event: unknown, ...args: unknown[]) => unknown>();
    let finishEffect!: () => void;
    const effect = new Promise<void>((resolve) => { finishEffect = resolve; });
    const afterInvoke = vi.fn(() => effect);
    const cleanup = installRustIpcBridge({
      host: {
        async invoke() { return { phase: "checking" }; },
        onPush: () => () => undefined,
      },
      ipcMain: {
        handle(channel, listener) { handlers.set(channel, listener); },
        removeHandler(channel) { handlers.delete(channel); },
      },
      targets: () => [],
      afterInvoke,
    });

    let settled = false;
    const request = Promise.resolve(handlers.get(CHANNELS.checkForUpdates)?.({}))
      .then((value) => { settled = true; return value; });
    await Promise.resolve();
    expect(afterInvoke).toHaveBeenCalledWith(CHANNELS.checkForUpdates, { phase: "checking" });
    expect(settled).toBe(false);
    finishEffect();
    await expect(request).resolves.toEqual({ phase: "checking" });
    expect(settled).toBe(true);
    cleanup();
  });

  it("lets a native post-invoke effect replace a public result", async () => {
    const handlers = new Map<string, (event: unknown, ...args: unknown[]) => unknown>();
    const cleanup = installRustIpcBridge({
      host: {
        async invoke() { return { phase: "checking" }; },
        onPush: () => () => undefined,
      },
      ipcMain: {
        handle(channel, listener) { handlers.set(channel, listener); },
        removeHandler(channel) { handlers.delete(channel); },
      },
      targets: () => [],
      afterInvoke: async (operation) => operation === CHANNELS.checkForUpdates
        ? { phase: "up-to-date" }
        : undefined,
    });
    await expect(handlers.get(CHANNELS.checkForUpdates)?.({}))
      .resolves.toEqual({ phase: "up-to-date" });
    cleanup();
  });

  it("broadcasts one-argument pushes only to live renderer targets", () => {
    let push: ((frame: HostPushFrame) => void) | undefined;
    const liveSend = vi.fn();
    const deadSend = vi.fn();
    const cleanup = installRustIpcBridge({
      host: {
        async invoke() { return undefined; },
        onPush(listener) { push = listener; return () => undefined; },
      },
      ipcMain: { handle() {}, removeHandler() {} },
      targets: () => [
        { isDestroyed: () => false, send: liveSend },
        { isDestroyed: () => true, send: deadSend },
      ],
    });
    push?.({
      kind: "push", version: 9, operation: CHANNELS.registry, payload: [{ key: "codex#one" }],
    });
    expect(liveSend).toHaveBeenCalledWith(CHANNELS.registry, [{ key: "codex#one" }]);
    expect(deadSend).not.toHaveBeenCalled();
    cleanup();
  });

  it("answers a ui_screenshot voice call from main without waking the renderer", async () => {
    let push: ((frame: HostPushFrame) => void) | undefined;
    const send = vi.fn();
    const invoke = vi.fn(async () => ({}));
    const cleanup = installRustIpcBridge({
      host: { invoke, onPush(listener) { push = listener; return () => undefined; } },
      ipcMain: { handle() {}, removeHandler() {} },
      targets: () => [{ isDestroyed: () => false, send }],
      capturePage: async () => "iVBORw0KGgo=",
    });
    push?.({
      kind: "push",
      version: 9,
      operation: CHANNELS.voiceUiCall,
      payload: { call_id: "c1", tool: "ui_screenshot", input: {} },
    });
    await vi.waitFor(() => { expect(invoke).toHaveBeenCalled(); });
    expect(invoke).toHaveBeenCalledWith(CHANNELS.voiceUiResult,
      { call_id: "c1", ok: true, output: "iVBORw0KGgo=" });
    expect(send).not.toHaveBeenCalled();
    cleanup();
  });

  it("forwards every other voice UI call to the renderer that owns the DOM", () => {
    let push: ((frame: HostPushFrame) => void) | undefined;
    const send = vi.fn();
    const invoke = vi.fn(async () => ({}));
    const cleanup = installRustIpcBridge({
      host: { invoke, onPush(listener) { push = listener; return () => undefined; } },
      ipcMain: { handle() {}, removeHandler() {} },
      targets: () => [{ isDestroyed: () => false, send }],
      capturePage: async () => "unused",
    });
    const call = { call_id: "c2", tool: "ui_snapshot", input: {} };
    push?.({ kind: "push", version: 9, operation: CHANNELS.voiceUiCall, payload: call });
    expect(send).toHaveBeenCalledWith(CHANNELS.voiceUiCall, call);
    expect(invoke).not.toHaveBeenCalled();
    cleanup();
  });
});
