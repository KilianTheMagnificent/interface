import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { TerminalOutputWindow, TERMINAL_OUTPUT_WINDOW_BYTES, TERMINAL_OUTPUT_WINDOW_FRAMES } from "./terminal-output-window.js";
import { encodeTerminalDataFrame, TERMINAL_DATA_VERSION, type TerminalDataFrame } from "./terminal-data-transport.js";

describe("terminal output credits", () => {
  const frame = (sequence: bigint, generation = 1n, terminalId = "pane") => ({ terminalId, generation, sequence });
  it("pins Rust and Electron to the same v2 bytes and credit ceilings", () => {
    const fixture = JSON.parse(readFileSync(new URL("../fixtures/terminal-data-v2.json", import.meta.url), "utf8"));
    expect(TERMINAL_DATA_VERSION).toBe(fixture.version);
    expect(TERMINAL_OUTPUT_WINDOW_BYTES).toBe(fixture.windowBytes);
    expect(TERMINAL_OUTPUT_WINDOW_FRAMES).toBe(fixture.windowFrames);
    for (const item of fixture.frames) {
      const value: TerminalDataFrame = { terminalId: fixture.terminalId,
        generation: BigInt(fixture.generation), sequence: BigInt(item.sequence),
        direction: item.direction, kind: item.kind, payload: Uint8Array.from(item.payload) };
      expect(encodeTerminalDataFrame(value).toString("hex")).toBe(item.hex);
    }
  });
  it("advances only a contiguous consumed prefix and ignores stale acknowledgements", () => {
    const window = new TerminalOutputWindow();
    window.sent(frame(0n), 10); window.sent(frame(1n), 20);
    expect(window.complete(frame(1n))).toBeUndefined();
    expect(window.complete(frame(0n, 2n))).toBeUndefined();
    expect(window.complete(frame(100n))).toBeUndefined();
    expect(window.outstandingBytes).toBe(118);
    expect(window.complete(frame(0n))).toBe(1n);
    expect(window.complete(frame(0n))).toBeUndefined();
    expect(window.outstandingBytes).toBe(0);
  });
  it("bounds tiny frames and bytes independently without blocking another pane", () => {
    const window = new TerminalOutputWindow();
    for (let sequence = 0n; sequence < 64n; sequence++) window.sent(frame(sequence), 1);
    expect(() => window.sent(frame(64n), 1)).toThrow();
    window.sent(frame(0n, 1n, "sibling"), 1);
    const large = new TerminalOutputWindow();
    for (let sequence = 0n; sequence < 3n; sequence++) large.sent(frame(sequence), 65536);
    expect(() => large.sent(frame(3n), 65536)).toThrow();
    expect(large.outstandingBytes).toBeLessThanOrEqual(TERMINAL_OUTPUT_WINDOW_BYTES);
  });
  it("releases abandoned document watermarks once across reload", () => {
    const window = new TerminalOutputWindow();
    window.sent(frame(12n), 10); window.sent(frame(13n), 20);
    expect(window.abandon()).toEqual([frame(13n)]);
    expect(window.abandon()).toEqual([]);
    expect(window.complete(frame(13n))).toBeUndefined();
    window.sent(frame(14n), 30);
    expect(window.complete(frame(13n))).toBeUndefined();
    expect(window.complete(frame(14n))).toBe(14n);
  });
});
