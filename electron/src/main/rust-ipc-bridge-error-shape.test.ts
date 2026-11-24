import { readFileSync } from "node:fs";
import { transformWithOxc } from "vite";
import { describe, expect, it } from "vitest";
import { installRustIpcBridge, rendererError } from "./rust-ipc-bridge.js";
import { RustHostError } from "./rust-host-client.js";
import { CHANNELS, type IfaceApi } from "../shared/ipc.js";
import {
  EFFECT_DEFINITELY_REJECTED_CODE, EFFECT_DEFINITELY_UNSENT_CODE, EFFECT_OUTCOME_UNKNOWN_CODE,
  encodeIpcErrorMessage,
} from "../shared/ipc-error.js";
import { ipcErrorShape, ipcMessage } from "../renderer/ipc-message.js";
import { transmissionOutcomeUnknown } from "../renderer/outbound-lane.js";
import { reopenWithRetry } from "../renderer/terminal-reopen-retry.js";

/* The v1 error shape end to end: a Rust host error rejected by the IPC bridge,
   stringified the way Electron replies to an invoke (`error.toString()` behind
   its "Error invoking remote method" wrapper), cleaned by the real preload's
   `invoke`, and classified by the renderer. contextBridge copies only an
   Error's message, which is why the message is the whole carrier. */

type Handler = (event: unknown, ...args: unknown[]) => unknown;

/** The bridge's handler for `channel` over a host whose invoke rejects with `failure`. */
function bridgeHandler(channel: string, failure: unknown): Handler {
  const handlers = new Map<string, Handler>();
  installRustIpcBridge({
    host: { invoke: () => Promise.reject(failure), onPush: () => () => undefined },
    ipcMain: {
      handle(channel, listener) { handlers.set(channel, listener); },
      removeHandler(channel) { handlers.delete(channel); },
    },
    targets: () => [],
  });
  const handler = handlers.get(channel);
  if (handler === undefined) throw new Error(`the bridge registered no ${channel} handler`);
  return handler;
}

/** What the bridge's handler for `channel` rejects with, as Electron replies it. */
async function bridgeReply(channel: string, failure: unknown, ...args: unknown[]): Promise<Error> {
  let thrown: unknown;
  try {
    await bridgeHandler(channel, failure)({}, ...args);
  } catch (error) {
    thrown = error;
  }
  expect(thrown).toBeDefined();
  return electronReply(channel, thrown);
}

/** What `ipcRenderer.invoke` rejects with for a handler that threw `error`. */
function electronReply(channel: string, error: unknown): Error {
  return new Error(`Error invoking remote method '${channel}': ${String(error)}`);
}

/** The real preload, loaded against a fake Electron whose invoke always rejects. */
async function preloadApi(reject: (channel: string) => Error): Promise<IfaceApi> {
  const source = readFileSync(new URL("./preload.cts", import.meta.url), "utf8");
  const { code } = await transformWithOxc(source, "preload.ts", { lang: "ts" });
  const electronImport = 'import { contextBridge, ipcRenderer, webUtils } from "electron";';
  expect(code).toContain(electronImport);
  const body = code.replace(electronImport,
    "const { contextBridge, ipcRenderer, webUtils } = electron;");
  expect(body).not.toMatch(/^\s*import\b/mu);
  const exposed = new Map<string, unknown>();
  const electron = {
    contextBridge: { exposeInMainWorld: (name: string, value: unknown) => { exposed.set(name, value); } },
    ipcRenderer: {
      invoke: (channel: string) => Promise.reject(reject(channel)),
      on: () => undefined, send: () => undefined, removeListener: () => undefined,
    },
    webUtils: { getPathForFile: () => "" },
  };
  new Function("electron", "process", `"use strict";\n${body}`)(electron, { argv: [] });
  return exposed.get("iface") as IfaceApi;
}

/** A host failure as the renderer receives it from `window.iface.send`. */
async function receivedSendFailure(failure: unknown): Promise<unknown> {
  const reply = await bridgeReply(CHANNELS.send, failure, "codex#one", "hello");
  const api = await preloadApi(() => reply);
  return api.send("codex#one", "hello").then(
    () => { throw new Error("the send resolved"); },
    (error: unknown) => error,
  );
}

describe("the main → renderer error shape", () => {
  it("names the Rust host's own delivery codes", () => {
    const dispatcher = readFileSync(
      new URL("../../../apps/interface-host/src/dispatcher.rs", import.meta.url), "utf8");
    const constant = (name: string) =>
      new RegExp(`pub const ${name}: &str = "([^"]+)";`, "u").exec(dispatcher)?.[1];
    expect(constant("DEFINITELY_UNSENT_CODE")).toBe(EFFECT_DEFINITELY_UNSENT_CODE);
    expect(constant("DEFINITELY_REJECTED_CODE")).toBe(EFFECT_DEFINITELY_REJECTED_CODE);
    expect(constant("OUTCOME_UNKNOWN_CODE")).toBe(EFFECT_OUTCOME_UNKNOWN_CODE);
  });

  it("encodes a Rust host error and leaves main's own refusals untyped", () => {
    const encoded = rendererError(new RustHostError(
      EFFECT_OUTCOME_UNKNOWN_CODE, "Codex writer did not confirm the accepted frame", false, false));
    expect(encoded).toBeInstanceOf(Error);
    expect((encoded as Error).name).toBe("Error");
    expect((encoded as Error).message).toBe("Codex writer did not confirm the accepted frame "
      + "[iface-error/1 code=effect-outcome-unknown retryable=false replay-safe=false]");
    const refusal = new Error("Unauthorized renderer IPC.");
    expect(rendererError(refusal)).toBe(refusal);
  });

  // The flags are the ones each code is raised with: dispatcher.rs
  // delivery_result and emit_registry_snapshot, the host's protocol and worker
  // errors, and rust-host-client.ts for the main-side host codes.
  it.each([
    [EFFECT_OUTCOME_UNKNOWN_CODE, "Claude's input transport closed during delivery", false, false, true],
    [EFFECT_OUTCOME_UNKNOWN_CODE, "The session changed, but its registry update could not reach the desktop.", false, false, true],
    ["host-exited", "The Rust host stopped unexpectedly (exit 1).", true, false, true],
    ["host-stopped", "The Rust host was stopped by the application.", true, false, true],
    ["host-protocol", "A failed Rust host response omitted its typed error.", false, false, true],
    ["desktop-worker-failed", "The desktop worker stopped.", false, false, true],
    ["response-too-large", "The Rust host response exceeded its limit.", false, false, true],
    ["runtime-response-mismatch", "The Rust runtime returned an incompatible response.", false, false, true],
    ["collaboration-projection-unavailable", "Collaboration source projection is unavailable.", false, false, true],
    [EFFECT_DEFINITELY_UNSENT_CODE, "Codex is not running.", false, true, false],
    [EFFECT_DEFINITELY_REJECTED_CODE, "Codex rejected the turn.", false, false, false],
    ["invalid-payload", "The request payload is invalid.", false, true, false],
    ["host-not-ready", "The Rust host is not ready.", true, true, false],
    ["host-backpressure", "The Rust host already has the maximum number of requests in flight.", true, true, false],
    ["request-too-large", "The Rust host request exceeded its limit.", false, true, false],
    ["host-write", "write EPIPE", true, true, false],
  ])("carries %s across the hop (%s)", async (code, message, retryable, replaySafe, unknown) => {
    const received = await receivedSendFailure(new RustHostError(code, message, retryable, replaySafe));
    expect(ipcMessage(received)).toBe(message);
    expect(ipcErrorShape(received)).toEqual({ kind: "typed", code, retryable, replaySafe });
    expect(transmissionOutcomeUnknown(received)).toBe(unknown);
  });

  it("keeps a message-less host error's facts behind the preload's fallback words", async () => {
    const received = await receivedSendFailure(
      new RustHostError(EFFECT_OUTCOME_UNKNOWN_CODE, "", false, false));
    expect(ipcMessage(received)).toBe("Interface could not complete that request.");
    expect(transmissionOutcomeUnknown(received)).toBe(true);
  });

  it("delivers main's own refusal exactly as before, as the untyped shape", async () => {
    const received = await receivedSendFailure(new Error("Unauthorized renderer IPC."));
    expect((received as Error).message).toBe("Unauthorized renderer IPC.");
    expect(ipcErrorShape(received)).toEqual({ kind: "untyped" });
    expect(transmissionOutcomeUnknown(received)).toBe(true);
  });

  it("classifies a pre-change rejection, without the shape, as unknown", async () => {
    // What a build before the shape delivered for a Rust host error: the
    // class name survived the preload and nothing else did.
    const api = await preloadApi(channel => new Error(`Error invoking remote method '${channel}': `
      + "RustHostError: Claude's input transport closed during delivery"));
    const received = await api.send("claude#one", "hello").catch((error: unknown) => error);
    expect(ipcErrorShape(received)).toEqual({ kind: "untyped" });
    expect(transmissionOutcomeUnknown(received)).toBe(true);
  });

  it("passes a marker of another version through for the renderer to refuse", async () => {
    const api = await preloadApi(channel => new Error(`Error invoking remote method '${channel}': `
      + "Error: Something newer. [iface-error/2 code=effect-definitely-unsent later=fields]"));
    const received = await api.send("claude#one", "hello").catch((error: unknown) => error);
    expect(ipcMessage(received)).toBe("Something newer.");
    expect(ipcErrorShape(received)).toEqual({ kind: "unrecognized" });
    expect(transmissionOutcomeUnknown(received)).toBe(true);
  });

  it("lets the terminal reopen retry read the host's real codes", async () => {
    const request = { id: "pane", cwd: "/repo", title: "Codex" };
    const attempts = async (failure: RustHostError): Promise<number> => {
      const reply = await bridgeReply(CHANNELS.createTerminal, failure, request);
      const api = await preloadApi(() => reply);
      let calls = 0;
      await reopenWithRetry(request, {
        createTerminal: async candidate => {
          calls += 1;
          if (calls === 1) return api.createTerminal(candidate);
          return { id: "pane", generation: 1, title: "Codex", root: "/repo", cwd: "/repo",
            state: "running", size: { columns: 80, rows: 24 }, createdAt: "", updatedAt: "" };
        },
        rebuild: async () => undefined, shellFallbacks: () => [],
        sleep: () => Promise.resolve(), cancelled: () => false, backoff: [0],
      });
      return calls;
    };
    // terminal.rs raises it replay-safe and not retryable: its code alone
    // marks the host-restart window a retry can outlast.
    expect(await attempts(new RustHostError("terminal-reopen-authority-unproven",
      "That saved terminal has no host restart proof yet.", false, true))).toBe(2);
    expect(await attempts(new RustHostError("terminal-id-conflict",
      "That terminal id is already in use.", false, true))).toBe(1);
    // A launch the host died under is never replayed.
    expect(await attempts(new RustHostError("host-exited",
      "The Rust host stopped unexpectedly (exit 1).", true, false))).toBe(1);
  });

  it("cleans only the words in front of the marker", async () => {
    const api = await preloadApi(channel => new Error(`Error invoking remote method '${channel}': `
      + `Error: ${encodeIpcErrorMessage("That peer target is host-controlled.\nWait.", {
        code: EFFECT_DEFINITELY_UNSENT_CODE, retryable: false, replaySafe: true })}`));
    const received = await api.send("claude#one", "hello").catch((error: unknown) => error);
    expect((received as Error).message).toBe("That peer target is host-controlled.\nWait. "
      + "[iface-error/1 code=effect-definitely-unsent retryable=false replay-safe=true]");
    expect(transmissionOutcomeUnknown(received)).toBe(false);
  });
});
