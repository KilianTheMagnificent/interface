import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const source = readFileSync(new URL("./preload.cts", import.meta.url), "utf8");

describe("preload terminal data failure contract", () => {
  it("retains one bounded failure and closes the private port fail-closed", () => {
    expect(source).toContain("let retainedTerminalDataFailure: string | null = null;");
    expect(source).toContain(".slice(0, TERMINAL_DATA_MAX_FAILURE_CHARS)");
    expect(source).toContain("terminalDataPort?.close();");
    expect(source).toContain("if (retainedTerminalDataFailure !== null) return;");
    expect(source).toContain("if (retainedTerminalDataFailure !== null) cb(retainedTerminalDataFailure);");
    expect(source).toContain("port.onmessageerror = () => {");
  });

  it("prunes completed generation state and bounds an unattended data backlog", () => {
    expect(source).toContain("terminalInputSequences.delete(key);");
    expect(source).toContain("terminalInputSequences.clear();");
    expect(source).toContain("terminalDataBacklog.length > TERMINAL_DATA_MAX_BACKLOG_EVENTS");
  });

  it("seals terminal input at output EOF without raising a shared transport failure", () => {
    const eof = source.indexOf('if (event.kind === "eof")');
    const retire = source.indexOf("retireTerminalInput(event.terminalId, event.generation)", eof);
    const send = source.indexOf("function sendTerminalInput");
    const discard = source.indexOf(
      "if ((retiredTerminalInputGenerations.get(id) ?? 0) >= generation) return",
      send,
    );
    const post = source.indexOf("port.postMessage({", discard);
    expect(eof).toBeGreaterThan(-1);
    expect(retire).toBeGreaterThan(eof);
    expect(discard).toBeGreaterThan(send);
    expect(post).toBeGreaterThan(discard);
  });

  it("bounds retired terminal ids with deterministic oldest eviction", () => {
    expect(source).toContain("const TERMINAL_DATA_MAX_RETIRED_TERMINALS = 32;");
    expect(source).toContain(
      "while (retiredTerminalInputGenerations.size > TERMINAL_DATA_MAX_RETIRED_TERMINALS)",
    );
    const oldest = source.indexOf("retiredTerminalInputGenerations.keys().next().value");
    const remove = source.indexOf("retiredTerminalInputGenerations.delete(oldest)", oldest);
    expect(oldest).toBeGreaterThan(-1);
    expect(remove).toBeGreaterThan(oldest);
  });

  it("requests and acknowledges the private port only after its listeners exist", () => {
    const portListener = source.indexOf("ipcRenderer.on(TERMINAL_DATA_PORT_CHANNEL");
    const failureListener = source.indexOf("ipcRenderer.on(TERMINAL_DATA_FAILURE_CHANNEL");
    const request = source.indexOf("ipcRenderer.send(TERMINAL_DATA_PORT_REQUEST_CHANNEL");
    const start = source.indexOf("port.start()", portListener);
    const acknowledge = source.indexOf(
      "ipcRenderer.send(TERMINAL_DATA_PORT_ACK_CHANNEL, { version: 1, token })",
      start,
    );
    expect(portListener).toBeGreaterThan(-1);
    expect(start).toBeGreaterThan(portListener);
    expect(acknowledge).toBeGreaterThan(start);
    expect(failureListener).toBeGreaterThan(acknowledge);
    expect(request).toBeGreaterThan(failureListener);
  });

  it("wraps the already-private MessagePort payload without copying it again", () => {
    expect(source).toContain("data: new Uint8Array(message.payload),");
    expect(source).not.toContain("message.payload.slice(0)");
  });

  it("structured-clones renderer input instead of transferring its ArrayBuffer", () => {
    const send = source.indexOf("function sendTerminalInput");
    const post = source.indexOf("port.postMessage({", send);
    const afterPost = source.indexOf("});", post);
    const call = source.slice(post, afterPost + 3);
    expect(post).toBeGreaterThan(send);
    expect(call).toContain("payload,");
    expect(call).not.toContain("[payload]");
    expect(source).not.toContain("}, [payload]);");
  });

  it("throws the retained root failure before a derived not-ready error", () => {
    const send = source.indexOf("function sendTerminalInput");
    const retained = source.indexOf("if (retainedTerminalDataFailure !== null)", send);
    const port = source.indexOf("const port = terminalDataPort", retained);
    const connecting = source.indexOf("still connecting", port);
    expect(send).toBeGreaterThan(-1);
    expect(retained).toBeGreaterThan(send);
    expect(port).toBeGreaterThan(retained);
    expect(connecting).toBeGreaterThan(port);
  });
});
