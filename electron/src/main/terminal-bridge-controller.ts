import { randomUUID } from "node:crypto";
import type { MessagePortMain } from "electron";
import { plainObject } from "./json.js";
import type { TerminalDataBridgeGate } from "./terminal-data-bridge.js";
import type { TerminalMessagePort } from "./terminal-data-transport.js";
import {
  isAllowedRendererNavigation, isAuthorizedIpcSender, type RendererLocation,
  type TrustedWebContents,
} from "./window-security.js";

// The per-document terminal MessagePort handshake. The preload asks on the
// request channel, main mints a UUIDv4 and transfers one port with it, and the
// preload echoes that token on the ack channel; `TerminalDataBridgeGate`
// refuses createTerminal/restartTerminal until the echo matches. Every channel
// is authorized twice — sender identity and navigation allowlist — like the
// invoke bridge. These strings are the preload's too (preload.cts).
export const TERMINAL_DATA_PORT_CHANNEL = "iface:terminalDataPort";
export const TERMINAL_DATA_PORT_REQUEST_CHANNEL = "iface:terminalDataPortRequest";
export const TERMINAL_DATA_PORT_ACK_CHANNEL = "iface:terminalDataPortAck";
export const TERMINAL_DATA_FAILURE_CHANNEL = "iface:terminalDataFailure";
export const TERMINAL_DATA_BRIDGE_TIMEOUT_MS = 5_000;
const TERMINAL_DATA_BRIDGE_TOKEN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

/** The IPC event fields the two authorization checks read. */
export interface TerminalBridgeEvent {
  sender: object;
  senderFrame?: { url: string } | null;
}

type TerminalBridgeListener = (event: TerminalBridgeEvent, value: unknown) => void;
type NavigationListener = (event: unknown, url: string, isInPlace: boolean,
  isMainFrame: boolean) => void;

/** `ipcMain`, as far as the handshake uses it. */
export interface TerminalBridgeIpc {
  on(channel: string, listener: TerminalBridgeListener): unknown;
  off(channel: string, listener: TerminalBridgeListener): unknown;
}

/** The window's `webContents`, as far as the handshake uses it. */
export interface TerminalBridgeContents extends TrustedWebContents {
  isDestroyed(): boolean;
  send(channel: string, ...args: unknown[]): void;
  postMessage(channel: string, message: unknown, transfer: MessagePortMain[]): void;
  on(event: "did-start-navigation", listener: NavigationListener): unknown;
  on(event: "render-process-gone", listener: () => void): unknown;
  off(event: "did-start-navigation", listener: NavigationListener): unknown;
  off(event: "render-process-gone", listener: () => void): unknown;
}

export interface TerminalBridgeWindow {
  isDestroyed(): boolean;
  readonly webContents: TerminalBridgeContents;
}

/** The main-process end of a `MessageChannelMain`. */
export type TerminalBridgePort = TerminalMessagePort & {
  once(event: "close", listener: () => void): unknown;
  off(event: "close", listener: () => void): unknown;
  close(): void;
};

/** `TerminalDataTransport`, as far as the handshake uses it. */
export interface TerminalBridgeTransport {
  attachPort(port: TerminalMessagePort): () => void;
  onFailure(listener: (error: { message: string }) => void): () => void;
  close(): void;
}

export interface TerminalBridgeControllerOptions {
  win: TerminalBridgeWindow;
  location: RendererLocation;
  ipc: TerminalBridgeIpc;
  gate: TerminalDataBridgeGate;
  /** The host's data transport; it exists only once the host is ready. */
  transport: () => TerminalBridgeTransport | undefined;
  hostReady: Promise<void>;
  isShuttingDown: () => boolean;
  productTitle: string;
  /** `new MessageChannelMain()`. */
  createChannel: () => { port1: TerminalBridgePort; port2: MessagePortMain };
  /** Called after the bridge is torn down for a renderer that died. */
  onRendererGone: () => void;
  newToken?: () => string;
}

export interface TerminalBridgeController {
  /** Settles on the first document's acknowledgement or on the first failure. */
  readonly ready: Promise<void>;
  /**
   * Fails the bridge if no handshake has settled or started within the
   * deadline. Armed once the first document has loaded, so host startup never
   * spends it.
   */
  armRequestDeadline(): void;
  /** Removes every listener and the active port. */
  dispose(): void;
}

function installTerminalDataBridge(
  contents: TerminalBridgeContents,
  transport: TerminalBridgeTransport | undefined,
  createChannel: () => { port1: TerminalBridgePort; port2: MessagePortMain },
  token: string,
  onFailure: (message: string) => void,
  onUnexpectedClose: () => void,
): () => void {
  if (transport === undefined) {
    throw new Error("The Rust host did not open its integrated terminal data channel.");
  }
  const { port1, port2 } = createChannel();
  const detach = transport.attachPort(port1);
  const removeFailure = transport.onFailure(error => {
    onFailure(error.message);
  });
  let expectedClose = false;
  const portClosed = (): void => {
    if (!expectedClose) onUnexpectedClose();
  };
  port1.once("close", portClosed);
  contents.postMessage(TERMINAL_DATA_PORT_CHANNEL, { version: 1, token }, [port2]);
  return () => {
    expectedClose = true;
    port1.off("close", portClosed);
    detach();
    removeFailure();
    port1.close();
  };
}

export function createTerminalBridgeController(
  options: TerminalBridgeControllerOptions,
): TerminalBridgeController {
  const {
    win, location, ipc, gate: terminalBridgeGate, transport, hostReady, isShuttingDown,
    productTitle: appProductTitle, createChannel, onRendererGone, newToken = randomUUID,
  } = options;
  let removeActiveTerminalBridge: (() => void) | null = null;
  let terminalBridgeTimer: NodeJS.Timeout | null = null;
  let firstTerminalBridgeSettled = false;
  let resolveFirstTerminalBridge: (() => void) | null = null;
  const terminalBridgeReady = new Promise<void>(resolve => {
    resolveFirstTerminalBridge = resolve;
  });
  const settleFirstTerminalBridge = (): void => {
    if (firstTerminalBridgeSettled) return;
    firstTerminalBridgeSettled = true;
    resolveFirstTerminalBridge?.();
    resolveFirstTerminalBridge = null;
  };
  const clearTerminalBridgeTimer = (): void => {
    if (terminalBridgeTimer === null) return;
    globalThis.clearTimeout(terminalBridgeTimer);
    terminalBridgeTimer = null;
  };
  const disconnectTerminalBridgeForDocument = (): void => {
    clearTerminalBridgeTimer();
    removeActiveTerminalBridge?.();
    removeActiveTerminalBridge = null;
  };
  const publishTerminalBridgeFailure = (message: string): void => {
    if (!win.isDestroyed() && !win.webContents.isDestroyed()) {
      win.webContents.send(TERMINAL_DATA_FAILURE_CHANNEL, message);
    }
  };
  const failTerminalBridge = (message: string, closeTransport: boolean): void => {
    const rootFailure = terminalBridgeGate.fail(message);
    disconnectTerminalBridgeForDocument();
    if (closeTransport) transport()?.close();
    publishTerminalBridgeFailure(rootFailure);
    settleFirstTerminalBridge();
  };
  const authorizeTerminalBridge = (event: TerminalBridgeEvent): boolean => {
    const contents = !win.isDestroyed() && !win.webContents.isDestroyed()
      ? win.webContents : null;
    return isAuthorizedIpcSender(event, contents)
      && isAllowedRendererNavigation(event.senderFrame?.url ?? "", location);
  };
  // Bumped when the main frame navigates away, so a handshake request that
  // waited out host startup is dropped if its document is gone.
  let terminalDocument = 0;
  const onTerminalBridgeRequest = (event: TerminalBridgeEvent, value: unknown): void => {
    if (!authorizeTerminalBridge(event)) return;
    // The data transport exists only once the host is ready, and a failed
    // bridge is sticky for the session: wait for the host rather than fail.
    const requestedIn = terminalDocument;
    void hostReady.then(() => {
      if (isShuttingDown() || requestedIn !== terminalDocument || !authorizeTerminalBridge(event)) return;
      beginTerminalBridge(value);
    }, () => undefined);
  };
  const beginTerminalBridge = (value: unknown): void => {
    const request = plainObject(value);
    if (request?.version !== 1) {
      failTerminalBridge("The integrated terminal data channel received an invalid handshake.", true);
      return;
    }
    disconnectTerminalBridgeForDocument();
    const token = newToken();
    if (!terminalBridgeGate.begin(token)) {
      publishTerminalBridgeFailure(terminalBridgeGate.failure
        ?? "The integrated terminal data channel is unavailable.");
      settleFirstTerminalBridge();
      return;
    }
    try {
      removeActiveTerminalBridge = installTerminalDataBridge(
        win.webContents,
        transport(),
        createChannel,
        token,
        message => { failTerminalBridge(message, true); },
        () => {
          failTerminalBridge(
            `The integrated terminal data channel closed before ${appProductTitle} finished with it.`,
            true,
          );
        },
      );
      // Electron IPC is asynchronous, but keep this correct even if a future
      // implementation can deliver the acknowledgement before install returns.
      if (!terminalBridgeGate.isReady) {
        terminalBridgeTimer = globalThis.setTimeout(() => {
          failTerminalBridge(
            "The integrated terminal data channel did not complete its secure handshake.",
            true,
          );
        }, TERMINAL_DATA_BRIDGE_TIMEOUT_MS);
      }
    } catch (error) {
      failTerminalBridge(error instanceof Error ? error.message : String(error), true);
    }
  };
  const onTerminalBridgeAck = (event: TerminalBridgeEvent, value: unknown): void => {
    if (!authorizeTerminalBridge(event)) return;
    const acknowledgement = plainObject(value);
    const token = acknowledgement?.token;
    if (acknowledgement?.version !== 1 || typeof token !== "string"
        || !TERMINAL_DATA_BRIDGE_TOKEN.test(token)
        || !terminalBridgeGate.acknowledge(token)) return;
    clearTerminalBridgeTimer();
    settleFirstTerminalBridge();
  };
  const onTerminalDocumentLeaving = (_event: unknown, _url: string,
      isInPlace: boolean, isMainFrame: boolean): void => {
    if (!isMainFrame || isInPlace) return;
    terminalDocument++;
    disconnectTerminalBridgeForDocument();
    terminalBridgeGate.resetDocument();
  };
  const onTerminalRendererGone = (): void => {
    disconnectTerminalBridgeForDocument();
    terminalBridgeGate.fail(`The ${appProductTitle} renderer stopped while terminals were active.`);
    transport()?.close();
    onRendererGone();
  };
  ipc.on(TERMINAL_DATA_PORT_REQUEST_CHANNEL, onTerminalBridgeRequest);
  ipc.on(TERMINAL_DATA_PORT_ACK_CHANNEL, onTerminalBridgeAck);
  win.webContents.on("did-start-navigation", onTerminalDocumentLeaving);
  win.webContents.on("render-process-gone", onTerminalRendererGone);
  return {
    ready: terminalBridgeReady,
    armRequestDeadline() {
      if (!firstTerminalBridgeSettled && terminalBridgeTimer === null) {
        terminalBridgeTimer = globalThis.setTimeout(() => {
          failTerminalBridge(
            "The integrated terminal data channel did not request its secure handshake.",
            true,
          );
        }, TERMINAL_DATA_BRIDGE_TIMEOUT_MS);
      }
    },
    dispose() {
      ipc.off(TERMINAL_DATA_PORT_REQUEST_CHANNEL, onTerminalBridgeRequest);
      ipc.off(TERMINAL_DATA_PORT_ACK_CHANNEL, onTerminalBridgeAck);
      // Shutdown after a renderer crash or a startup failure destroys the
      // window first, and reading a destroyed window's webContents throws.
      // Its listeners can never fire again, so there is nothing to remove.
      if (!win.isDestroyed()) {
        win.webContents.off("did-start-navigation", onTerminalDocumentLeaving);
        win.webContents.off("render-process-gone", onTerminalRendererGone);
      }
      disconnectTerminalBridgeForDocument();
    },
  };
}
