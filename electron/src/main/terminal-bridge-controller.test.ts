import type { MessagePortMain } from "electron";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  TERMINAL_DATA_BRIDGE_TIMEOUT_MS, createTerminalBridgeController,
  type TerminalBridgeEvent, type TerminalBridgeTransport,
} from "./terminal-bridge-controller.js";
import { TerminalDataBridgeGate } from "./terminal-data-bridge.js";

// The per-document terminal MessagePort handshake is the fence in front of
// createTerminal/restartTerminal. These tests drive it with fake Electron
// objects and pin what it accepts, every refusal, and its ordering.

const PAGE = "file:///Applications/Interface.app/Contents/Resources/app.asar/dist/index.html";
const TOKEN_A = "0a1b2c3d-0000-4000-8000-00000000000a";
const TOKEN_B = "0a1b2c3d-0000-4000-9000-00000000000b";
const TOKEN_C = "0a1b2c3d-0000-4000-a000-00000000000c";
const FAILURE = "iface:terminalDataFailure";
const REQUEST = "iface:terminalDataPortRequest";
const ACK = "iface:terminalDataPortAck";

type Listener = (...args: any[]) => void;

class FakePort {
  readonly closeListeners = new Set<Listener>();
  closed = false;
  postMessage(): void { /* frames are the transport's business */ }
  on(): void { /* the transport attaches its own listener */ }
  off(event: string, listener: Listener): void {
    if (event === "close") this.closeListeners.delete(listener);
  }
  once(event: string, listener: Listener): void {
    if (event === "close") this.closeListeners.add(listener);
  }
  start(): void { /* started by the transport */ }
  close(): void {
    this.closed = true;
  }
  /** The peer went away without main closing its end. */
  lose(): void {
    for (const listener of [...this.closeListeners]) listener();
  }
}

class FakeTransport implements TerminalBridgeTransport {
  readonly attached: FakePort[] = [];
  readonly failureListeners = new Set<(error: { message: string }) => void>();
  detached = 0;
  closed = 0;
  attachError: Error | null = null;
  constructor(private readonly log: string[]) {}
  attachPort(port: unknown): () => void {
    if (this.attachError !== null) throw this.attachError;
    this.attached.push(port as FakePort);
    return () => { this.detached += 1; };
  }
  onFailure(listener: (error: { message: string }) => void): () => void {
    this.failureListeners.add(listener);
    return () => { this.failureListeners.delete(listener); };
  }
  close(): void {
    this.closed += 1;
    this.log.push("transport closed");
  }
  fail(message: string): void {
    for (const listener of [...this.failureListeners]) listener(new Error(message));
  }
}

interface HarnessOptions {
  tokens?: string[];
  transport?: boolean;
}

function harness(options: HarnessOptions = {}) {
  const log: string[] = [];
  const tokens = [...(options.tokens ?? [TOKEN_A, TOKEN_B, TOKEN_C])];
  const mainFrame = { url: PAGE };
  const ipcListeners = new Map<string, Set<Listener>>();
  const contentsListeners = new Map<string, Set<Listener>>();
  const sent: unknown[][] = [];
  const posted: Array<{ channel: string; message: unknown; transfer: unknown[] }> = [];
  const state = { windowDestroyed: false, contentsDestroyed: false, shuttingDown: false };
  const add = (map: Map<string, Set<Listener>>, name: string, listener: Listener): void => {
    const set = map.get(name) ?? new Set<Listener>();
    set.add(listener);
    map.set(name, set);
  };
  const contents = {
    mainFrame,
    isDestroyed: () => state.contentsDestroyed,
    send(channel: string, ...args: unknown[]) { sent.push([channel, ...args]); },
    postMessage(channel: string, message: unknown, transfer: MessagePortMain[]) {
      posted.push({ channel, message, transfer });
    },
    on(event: string, listener: Listener) { add(contentsListeners, event, listener); },
    off(event: string, listener: Listener) { contentsListeners.get(event)?.delete(listener); },
  };
  // Like Electron's BrowserWindow, a destroyed window throws on any read of
  // its webContents.
  const win = {
    isDestroyed: () => state.windowDestroyed,
    get webContents() {
      if (state.windowDestroyed) throw new TypeError("Object has been destroyed");
      return contents;
    },
  };
  const transport = new FakeTransport(log);
  const channels: Array<{ port1: FakePort; port2: MessagePortMain }> = [];
  let resolveHost!: () => void;
  let rejectHost!: (error: Error) => void;
  const hostReady = new Promise<void>((resolve, reject) => {
    resolveHost = resolve;
    rejectHost = reject;
  });
  hostReady.catch(() => undefined);
  const gate = new TerminalDataBridgeGate();
  const rendererGone = vi.fn(() => { log.push("renderer gone handled"); });
  const controller = createTerminalBridgeController({
    win,
    location: { packagedEntryUrl: PAGE },
    ipc: {
      on(channel, listener) { add(ipcListeners, channel, listener); },
      off(channel, listener) { ipcListeners.get(channel)?.delete(listener); },
    },
    gate,
    transport: () => options.transport === false ? undefined : transport,
    hostReady,
    isShuttingDown: () => state.shuttingDown,
    productTitle: "Interface",
    createChannel: () => {
      const channel = {
        port1: new FakePort(),
        port2: { transferred: channels.length } as unknown as MessagePortMain,
      };
      channels.push(channel);
      return channel;
    },
    onRendererGone: rendererGone,
    newToken: () => tokens.shift() ?? "",
  });
  let readySettled = false;
  void controller.ready.then(() => { readySettled = true; });
  const trusted = (): TerminalBridgeEvent => ({ sender: contents, senderFrame: mainFrame });
  const emit = (map: Map<string, Set<Listener>>, name: string, ...args: unknown[]): void => {
    for (const listener of [...(map.get(name) ?? [])]) listener(...args);
  };
  const flush = async (): Promise<void> => {
    for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
  };
  return {
    controller, gate, state, mainFrame, contents, transport, channels, sent, posted, log,
    rendererGone, ipcListeners, contentsListeners, trusted,
    get readySettled() { return readySettled; },
    request(value: unknown = { version: 1 }, event: TerminalBridgeEvent = trusted()) {
      emit(ipcListeners, REQUEST, event, value);
    },
    ack(token: unknown, event: TerminalBridgeEvent = trusted(), version: unknown = 1) {
      emit(ipcListeners, ACK, event, { version, token });
    },
    navigate({ isInPlace = false, isMainFrame = true } = {}) {
      emit(contentsListeners, "did-start-navigation", {}, PAGE, isInPlace, isMainFrame);
    },
    killRenderer() { emit(contentsListeners, "render-process-gone", {}, {}); },
    async hostStarts() { resolveHost(); await flush(); },
    async hostFails() { rejectHost(new Error("host failed")); await flush(); },
    flush,
    /** A handshake through to the transferred port, not yet acknowledged. */
    async transfer() {
      this.request();
      await this.hostStarts();
      expect(this.channels.length).toBeGreaterThan(0);
      return this.channels[this.channels.length - 1]!;
    },
  };
}

type Harness = ReturnType<typeof harness>;

async function expectFailedClosed(h: Harness, message: string, closes = 1): Promise<void> {
  await h.flush();
  expect(h.gate.failure).toBe(message);
  expect(h.gate.isReady).toBe(false);
  expect(h.transport.closed).toBe(closes);
  expect(h.sent.at(-1)).toEqual([FAILURE, message]);
  expect(h.readySettled).toBe(true);
  expect(() => { h.gate.requireReady(); }).toThrow(message);
}

function expectUntouched(h: Harness): void {
  expect(h.channels).toHaveLength(0);
  expect(h.posted).toHaveLength(0);
  expect(h.sent).toHaveLength(0);
  expect(h.transport.closed).toBe(0);
  expect(h.gate.failure).toBeNull();
  expect(h.gate.isReady).toBe(false);
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("terminal bridge handshake", () => {
  it("listens on the request and ack channels and on the window's navigation and crash", () => {
    const h = harness();
    expect([...h.ipcListeners.keys()]).toEqual([REQUEST, ACK]);
    expect([...h.contentsListeners.keys()]).toEqual(["did-start-navigation", "render-process-gone"]);
  });

  it("transfers one port with a fresh token after the host starts, and is ready on its exact echo", async () => {
    const h = harness();
    h.request();
    await h.flush();
    // The data transport exists only once the host is ready.
    expect(h.channels).toHaveLength(0);
    await h.hostStarts();
    expect(h.channels).toHaveLength(1);
    const [{ port1, port2 }] = h.channels as [typeof h.channels[0]];
    expect(h.transport.attached).toEqual([port1]);
    expect(h.posted).toEqual([{
      channel: "iface:terminalDataPort", message: { version: 1, token: TOKEN_A }, transfer: [port2],
    }]);
    expect(h.gate.isReady).toBe(false);
    expect(() => { h.gate.requireReady(); }).toThrow(/still connecting/u);
    expect(h.readySettled).toBe(false);

    h.ack(TOKEN_A);
    await h.flush();
    expect(h.gate.isReady).toBe(true);
    expect(() => { h.gate.requireReady(); }).not.toThrow();
    expect(h.readySettled).toBe(true);
    // The acknowledgement cleared the handshake deadline.
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS * 2);
    expect(h.sent).toHaveLength(0);
    expect(h.transport.closed).toBe(0);
    expect(port1.closed).toBe(false);
  });

  it("ignores a request from anything but the window's own main frame at its own URL", async () => {
    const refusals: Array<(h: Harness) => TerminalBridgeEvent> = [
      h => ({ sender: {}, senderFrame: h.mainFrame }),
      h => ({ sender: h.contents, senderFrame: { url: PAGE } }),
      h => ({ sender: h.contents, senderFrame: null }),
      h => ({ sender: h.contents }),
      h => {
        h.mainFrame.url = "https://example.com/";
        return h.trusted();
      },
      h => {
        h.state.windowDestroyed = true;
        return h.trusted();
      },
      h => {
        h.state.contentsDestroyed = true;
        return h.trusted();
      },
    ];
    for (const refusal of refusals) {
      const h = harness();
      h.request({ version: 1 }, refusal(h));
      await h.hostStarts();
      expectUntouched(h);
      expect(h.readySettled).toBe(false);
    }
  });

  it("checks authorization when a request arrives, not only after host start", async () => {
    const h = harness();
    h.mainFrame.url = "https://example.com/";
    h.request();
    h.mainFrame.url = PAGE;
    await h.hostStarts();
    expectUntouched(h);
  });

  it("drops a request that waited out host start once its document or authorization is gone", async () => {
    const leftTheDocument = harness();
    leftTheDocument.request();
    leftTheDocument.navigate();
    await leftTheDocument.hostStarts();
    expectUntouched(leftTheDocument);

    const lostAuthorization = harness();
    lostAuthorization.request();
    lostAuthorization.mainFrame.url = "https://example.com/";
    await lostAuthorization.hostStarts();
    expectUntouched(lostAuthorization);

    const shuttingDown = harness();
    shuttingDown.request();
    shuttingDown.state.shuttingDown = true;
    await shuttingDown.hostStarts();
    expectUntouched(shuttingDown);

    // A failed host start is reported by startup itself, not by the bridge.
    const hostFailed = harness();
    hostFailed.request();
    await hostFailed.hostFails();
    expectUntouched(hostFailed);
    expect(hostFailed.readySettled).toBe(false);
  });

  it("keeps a waiting request across in-place and subframe navigation", async () => {
    const h = harness();
    h.request();
    h.navigate({ isInPlace: true });
    h.navigate({ isMainFrame: false });
    await h.hostStarts();
    expect(h.channels).toHaveLength(1);
    h.ack(TOKEN_A);
    expect(h.gate.isReady).toBe(true);
  });

  it("fails closed on a malformed handshake request", async () => {
    for (const value of [undefined, null, [], "1", 1, {}, { version: 2 }, { version: "1" }]) {
      const h = harness();
      for (const listener of h.ipcListeners.get(REQUEST) ?? []) listener(h.trusted(), value);
      await h.hostStarts();
      expect(h.channels).toHaveLength(0);
      await expectFailedClosed(h, "The integrated terminal data channel received an invalid handshake.");
    }
  });

  it("ignores an acknowledgement that is not the current token from the trusted frame", async () => {
    const h = harness();
    await h.transfer();
    h.ack(TOKEN_A, { sender: {}, senderFrame: h.mainFrame });
    h.ack(TOKEN_A, { sender: h.contents, senderFrame: { url: PAGE } });
    h.ack(TOKEN_A, h.trusted(), 2);
    h.ack(TOKEN_A, h.trusted(), "1");
    h.ack(TOKEN_A.toUpperCase());
    h.ack(TOKEN_B);
    h.ack(1);
    h.ack(null);
    for (const value of [undefined, null, [], TOKEN_A, { token: TOKEN_A }]) {
      for (const listener of h.ipcListeners.get(ACK) ?? []) listener(h.trusted(), value);
    }
    await h.flush();
    expect(h.gate.isReady).toBe(false);
    expect(h.readySettled).toBe(false);
    expect(h.sent).toHaveLength(0);
    h.ack(TOKEN_A);
    expect(h.gate.isReady).toBe(true);
  });

  it("ignores an acknowledgement that arrives before any port was transferred", async () => {
    const h = harness();
    await h.hostStarts();
    h.ack(TOKEN_A);
    expect(h.gate.isReady).toBe(false);
    expect(h.readySettled).toBe(false);
  });

  it("accepts only a version-4 UUID even when it is the token the gate holds", async () => {
    const h = harness({ tokens: ["not-a-version-4-uuid"] });
    const { port1 } = await h.transfer();
    h.ack("not-a-version-4-uuid");
    expect(h.gate.isReady).toBe(false);
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS);
    await expectFailedClosed(h,
      "The integrated terminal data channel did not complete its secure handshake.");
    expect(port1.closed).toBe(true);
  });

  it("fences the previous document's acknowledgement across a reload", async () => {
    const h = harness();
    const first = await h.transfer();
    h.navigate();
    // Main closed its own end, so the lost port is not a failure.
    expect(first.port1.closed).toBe(true);
    expect(h.transport.detached).toBe(1);
    expect(h.sent).toHaveLength(0);
    // Navigation also cleared the first document's deadline.
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS * 2);
    expect(h.gate.failure).toBeNull();

    h.request();
    await h.flush();
    expect(h.posted.at(-1)?.message).toEqual({ version: 1, token: TOKEN_B });
    h.ack(TOKEN_A);
    expect(h.gate.isReady).toBe(false);
    h.ack(TOKEN_B);
    expect(h.gate.isReady).toBe(true);
  });

  it("resets a ready gate on reload so the next document must shake hands again", async () => {
    const h = harness();
    await h.transfer();
    h.ack(TOKEN_A);
    expect(h.gate.isReady).toBe(true);
    h.navigate();
    expect(h.gate.isReady).toBe(false);
    expect(() => { h.gate.requireReady(); }).toThrow(/still connecting/u);
  });

  it("replaces the port and retires the first token when one document asks twice", async () => {
    const h = harness();
    const first = await h.transfer();
    h.request();
    await h.flush();
    expect(h.channels).toHaveLength(2);
    expect(first.port1.closed).toBe(true);
    expect(h.sent).toHaveLength(0);
    h.ack(TOKEN_A);
    expect(h.gate.isReady).toBe(false);
    h.ack(TOKEN_B);
    expect(h.gate.isReady).toBe(true);
  });

  it("fails closed when the transferred port is not acknowledged in time", async () => {
    const h = harness();
    const { port1 } = await h.transfer();
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS - 1);
    expect(h.gate.failure).toBeNull();
    vi.advanceTimersByTime(1);
    await expectFailedClosed(h,
      "The integrated terminal data channel did not complete its secure handshake.");
    expect(port1.closed).toBe(true);
    expect(h.transport.detached).toBe(1);
    // A late echo cannot revive the document.
    h.ack(TOKEN_A);
    expect(h.gate.isReady).toBe(false);
  });

  it("fails closed when the first document never asks for its port", async () => {
    const h = harness();
    await h.hostStarts();
    h.controller.armRequestDeadline();
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS - 1);
    expect(h.gate.failure).toBeNull();
    vi.advanceTimersByTime(1);
    await expectFailedClosed(h,
      "The integrated terminal data channel did not request its secure handshake.");
  });

  it("arms the request deadline only while nothing has settled or started", async () => {
    const settled = harness();
    await settled.transfer();
    settled.ack(TOKEN_A);
    settled.controller.armRequestDeadline();
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS * 2);
    expect(settled.gate.isReady).toBe(true);
    expect(settled.sent).toHaveLength(0);

    // A transfer already under way keeps its own acknowledgement deadline.
    const started = harness();
    await started.transfer();
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS - 1_000);
    started.controller.armRequestDeadline();
    vi.advanceTimersByTime(1_000);
    await expectFailedClosed(started,
      "The integrated terminal data channel did not complete its secure handshake.");
  });

  it("keeps the transfer's own deadline when the request deadline is armed after it", async () => {
    const h = harness();
    await h.transfer();
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS - 1_000);
    h.controller.armRequestDeadline();
    h.ack(TOKEN_A);
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS * 2);
    expect(h.gate.isReady).toBe(true);
    expect(h.sent).toHaveLength(0);
  });

  it("reports an unavailable channel and settles when the gate refuses a new transfer", async () => {
    const h = harness({ tokens: [""] });
    h.request();
    await h.hostStarts();
    expect(h.channels).toHaveLength(0);
    expect(h.sent).toEqual([[FAILURE, "The integrated terminal data channel is unavailable."]]);
    expect(h.transport.closed).toBe(0);
    expect(h.gate.failure).toBeNull();
    expect(h.readySettled).toBe(true);
  });

  it("keeps a failed document failed, naming its first failure, until the next document", async () => {
    const h = harness();
    await h.transfer();
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS);
    const root = "The integrated terminal data channel did not complete its secure handshake.";
    await expectFailedClosed(h, root);

    h.request();
    await h.flush();
    expect(h.channels).toHaveLength(1);
    expect(h.sent.at(-1)).toEqual([FAILURE, root]);
    expect(h.transport.closed).toBe(1);

    h.request({ version: 2 });
    await h.flush();
    // The derived failure is published under the root one.
    expect(h.sent.at(-1)).toEqual([FAILURE, root]);

    h.navigate();
    expect(h.gate.failure).toBeNull();
    h.request();
    await h.flush();
    expect(h.channels).toHaveLength(2);
    // The refused request above still minted (and dropped) the second token.
    h.ack(TOKEN_B);
    expect(h.gate.isReady).toBe(false);
    h.ack(TOKEN_C);
    expect(h.gate.isReady).toBe(true);
  });

  it("fails closed when the port is lost before main closed it", async () => {
    const h = harness();
    const { port1 } = await h.transfer();
    h.ack(TOKEN_A);
    port1.lose();
    await expectFailedClosed(h,
      "The integrated terminal data channel closed before Interface finished with it.");
    expect(h.transport.detached).toBe(1);
  });

  it("fails closed when the data transport fails", async () => {
    const h = harness();
    const { port1 } = await h.transfer();
    h.ack(TOKEN_A);
    h.transport.fail("The Rust host terminal output pipe closed unexpectedly.");
    await expectFailedClosed(h, "The Rust host terminal output pipe closed unexpectedly.");
    expect(port1.closed).toBe(true);
    expect(h.transport.failureListeners.size).toBe(0);
  });

  it("fails closed when the host has no data transport or refuses the port", async () => {
    const missing = harness({ transport: false });
    missing.request();
    await missing.hostStarts();
    expect(missing.channels).toHaveLength(0);
    expect(missing.gate.failure)
      .toBe("The Rust host did not open its integrated terminal data channel.");
    expect(missing.sent).toEqual([
      [FAILURE, "The Rust host did not open its integrated terminal data channel."],
    ]);
    expect(missing.readySettled).toBe(true);

    const refused = harness();
    refused.transport.attachError = new Error("A terminal document already owns this transport.");
    refused.request();
    await refused.hostStarts();
    await expectFailedClosed(refused, "A terminal document already owns this transport.");
    expect(refused.posted).toHaveLength(0);
  });

  it("publishes a failure only to a window that still exists", async () => {
    const h = harness();
    await h.transfer();
    h.state.contentsDestroyed = true;
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS);
    expect(h.gate.failure)
      .toBe("The integrated terminal data channel did not complete its secure handshake.");
    expect(h.transport.closed).toBe(1);
    expect(h.sent).toHaveLength(0);
  });

  it("tears the bridge down and closes the transport before the app handles a dead renderer", async () => {
    const h = harness();
    const { port1 } = await h.transfer();
    h.ack(TOKEN_A);
    h.killRenderer();
    expect(port1.closed).toBe(true);
    expect(h.gate.failure).toBe("The Interface renderer stopped while terminals were active.");
    expect(h.gate.isReady).toBe(false);
    expect(h.log).toEqual(["transport closed", "renderer gone handled"]);
    expect(h.rendererGone).toHaveBeenCalledOnce();
    // The renderer is gone, so nothing is sent to it.
    expect(h.sent).toHaveLength(0);
  });

  it("removes every listener, the active port and its deadline on dispose", async () => {
    const h = harness();
    const { port1 } = await h.transfer();
    h.controller.dispose();
    expect([...h.ipcListeners.values()].every(set => set.size === 0)).toBe(true);
    expect([...h.contentsListeners.values()].every(set => set.size === 0)).toBe(true);
    expect(port1.closed).toBe(true);
    expect(h.transport.detached).toBe(1);
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS * 2);
    expect(h.sent).toHaveLength(0);
    expect(h.gate.failure).toBeNull();
  });

  // A renderer crash or a startup failure destroys the window, then its
  // 'closed' handler starts shutdown, which disposes the bridge.
  it("disposes without throwing after the window is already destroyed", async () => {
    const h = harness();
    const { port1 } = await h.transfer();
    h.state.windowDestroyed = true;
    h.state.contentsDestroyed = true;
    expect(() => { h.controller.dispose(); }).not.toThrow();
    expect([...h.ipcListeners.values()].every(set => set.size === 0)).toBe(true);
    expect(port1.closed).toBe(true);
    expect(h.transport.detached).toBe(1);
    vi.advanceTimersByTime(TERMINAL_DATA_BRIDGE_TIMEOUT_MS * 2);
    expect(h.gate.failure).toBeNull();
  });
});
