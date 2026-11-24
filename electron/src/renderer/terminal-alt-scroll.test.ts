import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  altScreenWheelCell, altScreenWheelLines, altScreenWheelSequence,
  type AltScreenWheelState,
} from "./terminal-alt-scroll";

const terminalViewSource = readFileSync(
  new URL("./components/TerminalView.tsx", import.meta.url), "utf8",
);

const alt = (over: Partial<AltScreenWheelState> = {}): AltScreenWheelState => ({
  bufferType: "alternate",
  mouseTrackingMode: "none",
  applicationCursorKeys: false,
  cols: 80,
  rows: 24,
  col: 10,
  row: 5,
  ...over,
});

describe("altScreenWheelSequence", () => {
  it("leaves the normal buffer to xterm so shell scrollback still works", () => {
    expect(altScreenWheelSequence(
      { deltaY: 40 },
      alt({ bufferType: "normal" }),
    )).toBeNull();
  });

  it("encodes one alt-screen line as an arrow when mouse tracking is off", () => {
    // xterm 6 consumeWheelEvent floors pixel deltas under ~50px to 0 lines and
    // then swallows the event. A Grok/vim alt screen still needs the sequence;
    // altScreenWheelLines decides how many, this only picks the direction.
    expect(altScreenWheelSequence({ deltaY: -20 }, alt())).toBe("\x1b[A");
    expect(altScreenWheelSequence({ deltaY: 20 }, alt())).toBe("\x1b[B");
  });

  it("ignores a wheel event with no vertical delta", () => {
    expect(altScreenWheelSequence({ deltaY: 0 }, alt())).toBeNull();
  });

  it("uses application cursor keys when the TUI asked for them", () => {
    expect(altScreenWheelSequence(
      { deltaY: -8 },
      alt({ applicationCursorKeys: true }),
    )).toBe("\x1bOA");
    expect(altScreenWheelSequence(
      { deltaY: 8 },
      alt({ applicationCursorKeys: true }),
    )).toBe("\x1bOB");
  });

  it("sends an SGR wheel report so Grok can hit-test its own scrollback", () => {
    expect(altScreenWheelSequence(
      { deltaY: -12 },
      alt({ mouseTrackingMode: "any", col: 14, row: 7 }),
    )).toBe("\x1b[<64;14;7M");
    expect(altScreenWheelSequence(
      { deltaY: 12 },
      alt({ mouseTrackingMode: "vt200", col: 3, row: 20 }),
    )).toBe("\x1b[<65;3;20M");
    expect(altScreenWheelSequence(
      { deltaY: -12 },
      alt({ mouseTrackingMode: "drag", col: 1, row: 1 }),
    )).toBe("\x1b[<64;1;1M");
  });

  it("does not treat X10 mouse as a wheel protocol", () => {
    expect(altScreenWheelSequence(
      { deltaY: -20 },
      alt({ mouseTrackingMode: "x10" }),
    )).toBe("\x1b[A");
  });

  it("encodes wheel modifiers the same way xterm's SGR encoder does", () => {
    expect(altScreenWheelSequence(
      { deltaY: -12, shiftKey: true },
      alt({ mouseTrackingMode: "any" }),
    )).toBe("\x1b[<68;10;5M");
    expect(altScreenWheelSequence(
      { deltaY: 12, altKey: true, ctrlKey: true },
      alt({ mouseTrackingMode: "any" }),
    )).toBe("\x1b[<89;10;5M");
  });

  it("clamps the SGR cell to the live grid", () => {
    expect(altScreenWheelSequence(
      { deltaY: -4 },
      alt({ mouseTrackingMode: "any", col: 0, row: 99, cols: 80, rows: 24 }),
    )).toBe("\x1b[<64;1;24M");
  });
});

describe("altScreenWheelLines", () => {
  const grid = { rows: 24, cellHeight: 17 };

  it("accumulates trackpad pixels into whole lines the way xterm does", () => {
    // A 20px delta is under xterm's 50px damping threshold: 20/17*0.3 = 0.35
    // lines. Three events make one arrow, not three.
    const acc = { partial: 0 };
    expect(altScreenWheelLines({ deltaY: -20 }, grid, acc)).toBe(0);
    expect(altScreenWheelLines({ deltaY: -20 }, grid, acc)).toBe(0);
    expect(altScreenWheelLines({ deltaY: -20 }, grid, acc)).toBe(-1);
    expect(acc.partial).toBeCloseTo(-0.0588, 3);
  });

  it("does not damp a real wheel notch", () => {
    expect(altScreenWheelLines({ deltaY: 120 }, grid, { partial: 0 })).toBe(7);
  });

  it("takes line and page deltas at face value", () => {
    expect(altScreenWheelLines({ deltaY: -3, deltaMode: 1 }, grid, { partial: 0 })).toBe(-3);
    expect(altScreenWheelLines({ deltaY: 1, deltaMode: 2 }, grid, { partial: 0 })).toBe(24);
  });

  it("survives a host with no size yet", () => {
    expect(altScreenWheelLines({ deltaY: 60 }, { rows: 24, cellHeight: 0 }, { partial: 0 }))
      .toBe(60);
  });
});

describe("altScreenWheelCell", () => {
  it("maps a pointer into a 1-based cell on the emulator grid", () => {
    expect(altScreenWheelCell(
      25, 48,
      { left: 10, top: 16, width: 80, height: 96 },
      8, 6,
    )).toEqual({ col: 2, row: 3 });
  });

  it("falls back to the top-left cell when the host has no size yet", () => {
    expect(altScreenWheelCell(
      0, 0,
      { left: 0, top: 0, width: 0, height: 0 },
      80, 24,
    )).toEqual({ col: 1, row: 1 });
  });
});

describe("TerminalView alt-screen wheel wiring", () => {
  it("installs the custom wheel handler on each emulator", () => {
    expect(terminalViewSource).toContain("attachCustomWheelEventHandler");
    expect(terminalViewSource).toContain("altScreenWheelSequence");
    expect(terminalViewSource).toContain("altScreenWheelCell");
    // Lines, not events: the sequence repeats per whole line xterm would scroll.
    expect(terminalViewSource).toContain("altScreenWheelLines(");
    expect(terminalViewSource).toContain("terminal.input(sequence.repeat(Math.abs(lines)))");
  });
});
