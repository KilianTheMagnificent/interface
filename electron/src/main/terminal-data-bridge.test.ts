import { describe, expect, it } from "vitest";
import { TerminalDataBridgeGate } from "./terminal-data-bridge.js";

describe("terminal data bridge readiness gate", () => {
  it("requires the exact current-document acknowledgement before launch", () => {
    const gate = new TerminalDataBridgeGate();
    expect(() => { gate.requireReady(); }).toThrow(/still connecting/u);
    expect(gate.begin("document-a")).toBe(true);
    expect(gate.acknowledge("stale-document")).toBe(false);
    expect(() => { gate.requireReady(); }).toThrow(/still connecting/u);
    expect(gate.acknowledge("document-a")).toBe(true);
    expect(gate.isReady).toBe(true);
    expect(() => { gate.requireReady(); }).not.toThrow();
  });

  it("fences a stale acknowledgement across reload", () => {
    const gate = new TerminalDataBridgeGate();
    expect(gate.begin("document-a")).toBe(true);
    gate.resetDocument();
    expect(gate.begin("document-b")).toBe(true);
    expect(gate.acknowledge("document-a")).toBe(false);
    expect(gate.acknowledge("document-b")).toBe(true);
    expect(gate.isReady).toBe(true);
  });

  it("retains the first lost-port failure and refuses in-document recovery", () => {
    const gate = new TerminalDataBridgeGate();
    expect(gate.begin("document-a")).toBe(true);
    expect(gate.acknowledge("document-a")).toBe(true);
    expect(gate.fail("root pipe failure")).toBe("root pipe failure");
    expect(gate.fail("derived not-ready error")).toBe("root pipe failure");
    expect(gate.failure).toBe("root pipe failure");
    expect(gate.isReady).toBe(false);
    expect(gate.begin("same-document-retry")).toBe(false);
    expect(() => { gate.requireReady(); }).toThrow("root pipe failure");
  });
});
