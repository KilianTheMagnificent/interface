import { describe, expect, it } from "vitest";
import { KeyboardPresses } from "./keyboard-input";

type Input = Parameters<KeyboardPresses["suppress"]>[0];

const down = (key: string, extra: Partial<Input> = {}): Input => ({
  type: "keydown", key, code: /[0-9]/.test(key) ? `Digit${key}` : `Key${key.toUpperCase()}`,
  repeat: false, isComposing: false, shiftKey: false, ctrlKey: false,
  altKey: false, metaKey: false, location: 0, ...extra,
});

describe("physical keyboard press boundaries", () => {
  it("replays the captured two-down, one-up sequence as abc123", () => {
    const presses = new KeyboardPresses();
    let text = "";
    for (const key of "abc123") {
      const input = down(key);
      for (let copy = 0; copy < 2; copy++) if (!presses.suppress(input)) text += key;
      expect(presses.suppress({ ...input, type: "keyup" })).toBe(false);
    }
    expect(text).toBe("abc123");
  });

  it("preserves arbitrarily fast repeated letters when each press is released", () => {
    const presses = new KeyboardPresses();
    for (const key of "aabbcc112233") {
      expect(presses.suppress(down(key))).toBe(false);
      presses.suppress(down(key, { type: "keyup" }));
    }
  });

  it("preserves held-key auto-repeat and overlapping keys", () => {
    const presses = new KeyboardPresses();
    expect(presses.suppress(down("a"))).toBe(false);
    expect(presses.suppress(down("b"))).toBe(false);
    expect(presses.suppress(down("a"))).toBe(true);
    for (let i = 0; i < 5; i++) expect(presses.suppress(down("a", { repeat: true }))).toBe(false);
    presses.suppress(down("b", { type: "keyup" }));
    expect(presses.suppress(down("b"))).toBe(false);
  });

  it("keeps changed modifiers, dead keys, composition and shortcuts native", () => {
    const presses = new KeyboardPresses();
    presses.suppress(down("a"));
    expect(presses.suppress(down("A", { shiftKey: true }))).toBe(false);
    for (const extra of [{ isComposing: true }, { metaKey: true }, { ctrlKey: true },
      { altKey: true }, { code: "" }, { code: "Unidentified" }]) {
      expect(presses.suppress(down("a", extra))).toBe(false);
      expect(presses.suppress(down("a", extra))).toBe(false);
    }
    for (const key of ["Dead", "Process", "Unidentified", "Shift", "F1"]) {
      expect(presses.suppress(down(key))).toBe(false);
      expect(presses.suppress(down(key))).toBe(false);
    }
    expect(presses.suppress(down("a"))).toBe(false);
  });

  it("protects editing and navigation keys while retaining releases and auto-repeat", () => {
    const presses = new KeyboardPresses();
    for (const key of ["Backspace", "Delete", "Enter", "Tab", "Escape", "ArrowLeft",
      "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]) {
      const input = down(key, { code: key });
      expect(presses.suppress(input)).toBe(false);
      expect(presses.suppress(input)).toBe(true);
      expect(presses.suppress({ ...input, repeat: true })).toBe(false);
      presses.suppress({ ...input, type: "keyup" });
      expect(presses.suppress(input)).toBe(false);
      presses.suppress({ ...input, type: "keyup" });
    }
  });

  it("forgets a missing release when native focus or the document changes", () => {
    const presses = new KeyboardPresses();
    presses.suppress(down("a"));
    presses.reset();
    expect(presses.suppress(down("a"))).toBe(false);
  });
});
