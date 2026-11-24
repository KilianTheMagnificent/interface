import { RustHostError, type HostPushFrame } from "./rust-host-client.js";
import { plainObject } from "./json.js";
import {
  RENDERER_REQUEST_CHANNELS, encodeRendererPayload,
} from "./ipc-payload.js";
import { CHANNELS } from "../shared/ipc.js";
import { encodeIpcErrorMessage } from "../shared/ipc-error.js";

export interface RustHostInvoker {
  invoke(operation: string, payload: unknown): Promise<unknown>;
  onPush(listener: (frame: HostPushFrame) => void): () => void;
}

export interface IpcMainRegistrar {
  handle(channel: string, listener: (event: unknown, ...args: unknown[]) => unknown): void;
  removeHandler(channel: string): void;
}

export interface RendererTarget {
  isDestroyed(): boolean;
  send(channel: string, ...args: unknown[]): void;
}

export type RustIpcBridgeOptions = {
  host: RustHostInvoker;
  ipcMain: IpcMainRegistrar;
  targets(): readonly RendererTarget[];
  authorize?(event: unknown): boolean;
  beforeInvoke?(operation: string, payload: unknown): unknown | Promise<unknown>;
  afterInvoke?(operation: string, result: unknown): unknown | Promise<unknown>;
  /** Base64 PNG of the live window, for the one voice UI tool the page cannot serve. */
  capturePage?(): Promise<string>;
};

/**
 * The call_id of a `ui_screenshot` voice tool call, which main answers itself.
 * Every other voice UI tool is the renderer's — it reads and drives the DOM,
 * which is exactly what a main-process capture cannot do.
 */
function voiceScreenshotCallId(frame: HostPushFrame): string | undefined {
  if (frame.operation !== CHANNELS.voiceUiCall) return undefined;
  const payload = plainObject(frame.payload);
  if (payload === undefined || payload.tool !== "ui_screenshot") return undefined;
  return typeof payload.call_id === "string" ? payload.call_id : undefined;
}

export function rendererPushArguments(frame: HostPushFrame): unknown[] {
  if (frame.operation !== CHANNELS.event) return [frame.payload];
  const payload = plainObject(frame.payload);
  if (payload === undefined || typeof payload.key !== "string" || !("event" in payload)) {
    throw new TypeError("iface:event requires a {key,event} Rust host payload.");
  }
  return [payload.key, payload.event];
}

/**
 * What a renderer receives for a failed request. Electron passes it only the
 * text, so a Rust host error's code, retryable and replaySafe travel in it as
 * the v1 error shape (`shared/ipc-error.ts`); the renderer decides provider
 * outcomes from them. Every other failure (main's own refusals) keeps its
 * message alone, the untyped shape a renderer must treat as unproven.
 */
export function rendererError(error: unknown): unknown {
  if (!(error instanceof RustHostError)) return error;
  return new Error(encodeIpcErrorMessage(error.message, error), { cause: error });
}

export function installRustIpcBridge(options: RustIpcBridgeOptions): () => void {
  const { host, ipcMain } = options;
  for (const operation of RENDERER_REQUEST_CHANNELS) {
    ipcMain.handle(operation, async (event, ...args: unknown[]) => {
      try {
        if (options.authorize !== undefined && !options.authorize(event)) {
          throw new Error("Unauthorized renderer IPC.");
        }
        const encoded = encodeRendererPayload(operation, args);
        const payload = options.beforeInvoke === undefined
          ? encoded
          : await options.beforeInvoke(operation, encoded);
        const result = await host.invoke(operation, payload);
        if (options.afterInvoke === undefined) return result;
        const replacement = await options.afterInvoke(operation, result);
        return replacement === undefined ? result : replacement;
      } catch (error) {
        throw rendererError(error);
      }
    });
  }

  // Total by construction: a throw in a push listener is an uncaughtException,
  // so the screenshot answer swallows both the capture failure and the failure
  // to report it. The host's own 10 s timeout covers a silent host.
  const answerScreenshot = async (callId: string): Promise<void> => {
    let ok = false;
    let output = "This build cannot capture the window.";
    try {
      if (options.capturePage !== undefined) {
        output = await options.capturePage();
        ok = true;
      }
    } catch (error) {
      output = error instanceof Error ? error.message : String(error);
    }
    try {
      await host.invoke(CHANNELS.voiceUiResult, { call_id: callId, ok, output });
    } catch { /* the host is gone; there is nobody left to answer. */ }
  };

  const removePush = host.onPush((frame) => {
    const screenshot = voiceScreenshotCallId(frame);
    if (screenshot !== undefined) {
      void answerScreenshot(screenshot);
      return;
    }
    const args = rendererPushArguments(frame);
    for (const target of options.targets()) {
      if (!target.isDestroyed()) target.send(frame.operation, ...args);
    }
  });

  return () => {
    removePush();
    for (const operation of RENDERER_REQUEST_CHANNELS) ipcMain.removeHandler(operation);
  };
}
