import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { TERMINAL_CARET_HEIGHT, terminalCaretBox, terminalCursorStyleAfter } from "./terminal-caret";

const terminalViewSource = readFileSync(new URL("./components/TerminalView.tsx", import.meta.url), "utf8");
const terminalCss = readFileSync(new URL("./terminal.css", import.meta.url), "utf8");

describe("terminal caret", () => {
  it("follows xterm's DECSCUSR styles", () => {
    expect(terminalCursorStyleAfter(undefined, [6])).toBe("bar");
    expect(terminalCursorStyleAfter(undefined, [5])).toBe("bar");
    expect(terminalCursorStyleAfter("bar", [2])).toBe("block");
    expect(terminalCursorStyleAfter("bar", [1])).toBe("block");
    expect(terminalCursorStyleAfter("bar", [4])).toBe("underline");
    // CSI SP q with no parameter is 0: back to the configured style.
    expect(terminalCursorStyleAfter("bar", [0])).toBeUndefined();
    expect(terminalCursorStyleAfter("bar", [])).toBe("block");
    expect(terminalCursorStyleAfter("bar", [7])).toBe("bar");
    expect(terminalCursorStyleAfter(undefined, [[6, 1]])).toBe("bar");
  });

  it("keeps the reference width on Retina and standard displays, centred within the row", () => {
    const cell = { width: 12, height: 24 };
    const retina = terminalCaretBox(cell, 3, 2, 2);
    expect(retina.width).toBe(2);
    expect(retina.height).toBeLessThan(cell.height);
    expect(retina.height).toBe(Math.round(cell.height * TERMINAL_CARET_HEIGHT * 2) / 2);
    expect(retina.left).toBe(36);
    expect(retina.top - 2 * cell.height).toBeCloseTo((cell.height - retina.height) / 2, 0);
    expect(retina.top + retina.height).toBeLessThanOrEqual(3 * cell.height);
    const standard = terminalCaretBox(cell, 0, 0, 1);
    expect(standard.width).toBe(2);
    expect(Number.isInteger(standard.top) && Number.isInteger(standard.height)).toBe(true);
  });

  it("is the default cursor, since Claude never asks for a shape and Codex asks for the default", () => {
    expect(terminalViewSource).toContain('cursorStyle: "bar",');
  });

  it("owns the theme while it draws xterm's bar", () => {
    expect(terminalViewSource).toContain("const caret = installTerminalCaret(terminal);");
    expect(terminalViewSource).toMatch(/caret\.dispose\(\);[\s\S]*?terminal\.dispose\(\);/u);
    expect(terminalViewSource).toContain("else caretRef.current.setTheme(next.theme ?? {});");
    expect(terminalCss).toMatch(/\.terminal-xterm \.terminal-caret \{[^}]*background: var\(--terminal-caret-color\);/u);
  });
});
