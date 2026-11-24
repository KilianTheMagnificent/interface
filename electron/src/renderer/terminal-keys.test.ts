import { describe, expect, it } from "vitest";
import {
  TERMINAL_DELETE_TO_LINE_START, TERMINAL_PROGRAM_COPY_KEY, TERMINAL_REVERSE_TAB,
  isClosePaneChord, newPaneChordAllowedAt,
  shellOwnsChord, terminalCenterKeyAction, terminalCopyGoesToProgram,
  terminalInputForKey, terminalOpensSearch, terminalReverseTabGoesToProgram,
} from "./terminal-keys";

const chord = (over: {
  key?: string;
  code?: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  repeat?: boolean;
  isComposing?: boolean;
  keyCode?: number;
} = {}) => ({
  key: "d",
  code: "KeyD",
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  repeat: false,
  ...over,
});

describe("terminalCenterKeyAction", () => {
  it("splits the focused pane left/right with ⌘D and top/bottom with ⌘⇧D", () => {
    expect(terminalCenterKeyAction(chord({ metaKey: true }))).toBe("split-horizontal");
    expect(terminalCenterKeyAction(chord({ metaKey: true, key: "D" })))
      .toBe("split-horizontal");
    expect(terminalCenterKeyAction(chord({ metaKey: true, shiftKey: true })))
      .toBe("split-vertical");
  });

  it("opens a pane with ⌘↩ and leaves every other Enter to the program", () => {
    const enter = (over: Parameters<typeof chord>[0] = {}) =>
      chord({ key: "Enter", code: "Enter", ...over });
    expect(terminalCenterKeyAction(enter({ metaKey: true }))).toBe("new-pane");
    expect(terminalCenterKeyAction(enter())).toBeNull();
    expect(terminalCenterKeyAction(enter({ ctrlKey: true }))).toBeNull();
    expect(terminalCenterKeyAction(enter({ shiftKey: true }))).toBeNull();
    expect(terminalCenterKeyAction(enter({ metaKey: true, shiftKey: true }))).toBeNull();
    expect(terminalCenterKeyAction(enter({ metaKey: true, altKey: true }))).toBeNull();
    expect(terminalCenterKeyAction(enter({ metaKey: true, ctrlKey: true }))).toBeNull();
    // A held chord opens one pane, and an IME's confirming Enter opens none.
    expect(terminalCenterKeyAction(enter({ metaKey: true, repeat: true }))).toBeNull();
    expect(terminalCenterKeyAction(enter({ metaKey: true, isComposing: true }))).toBeNull();
    expect(terminalCenterKeyAction(enter({ metaKey: true, keyCode: 229 }))).toBeNull();
  });

  it("keeps ⌘↩ for the launcher and for the app's own fields", () => {
    expect(newPaneChordAllowedAt({ insideLauncher: false, field: "terminal-input" })).toBe(true);
    expect(newPaneChordAllowedAt({ insideLauncher: false, field: "none" })).toBe(true);
    expect(newPaneChordAllowedAt({ insideLauncher: false, field: "other" })).toBe(false);
    expect(newPaneChordAllowedAt({ insideLauncher: true, field: "none" })).toBe(false);
  });

  it("closes the selected pane on ⌘W only, never Control+W or ⇧⌘W", () => {
    const w = (over: Parameters<typeof chord>[0] = {}) => chord({ key: "w", code: "KeyW", ...over });
    expect(isClosePaneChord(w({ metaKey: true }))).toBe(true);
    expect(isClosePaneChord(w({ metaKey: true, key: "W" }))).toBe(true);
    // Control+W deletes a word in a shell; ⇧⌘W is the menu's Close Window.
    expect(isClosePaneChord(w({ ctrlKey: true }))).toBe(false);
    expect(isClosePaneChord(w({ metaKey: true, ctrlKey: true }))).toBe(false);
    expect(isClosePaneChord(w({ metaKey: true, shiftKey: true }))).toBe(false);
    expect(isClosePaneChord(w({ metaKey: true, altKey: true }))).toBe(false);
    expect(isClosePaneChord(w())).toBe(false);
    expect(isClosePaneChord(chord({ metaKey: true }))).toBe(false);
  });

  it("leaves Ctrl+D for the PTY so EOF still works", () => {
    expect(terminalCenterKeyAction(chord({ ctrlKey: true }))).toBeNull();
    expect(terminalCenterKeyAction(chord({ ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(terminalCenterKeyAction(chord({ metaKey: true, ctrlKey: true }))).toBeNull();
    expect(terminalCenterKeyAction(chord({ metaKey: true, altKey: true }))).toBeNull();
    expect(terminalCenterKeyAction(chord())).toBeNull();
  });

  it("does not spawn extra shells from a held ⌘D key-repeat", () => {
    expect(terminalCenterKeyAction(chord({ metaKey: true, repeat: true }))).toBeNull();
    expect(terminalCenterKeyAction(chord({
      metaKey: true, shiftKey: true, repeat: true,
    }))).toBeNull();
    expect(terminalCenterKeyAction(chord({
      metaKey: true, shiftKey: true, key: "]", code: "BracketRight", repeat: true,
    }))).toBe("next-terminal");
  });

  it("keeps ⌘⇧[ / ⌘⇧] as terminal-focus cycling, including Control on Windows", () => {
    expect(terminalCenterKeyAction(chord({
      metaKey: true, shiftKey: true, key: "{", code: "BracketLeft",
    }))).toBe("previous-terminal");
    expect(terminalCenterKeyAction(chord({
      ctrlKey: true, shiftKey: true, key: "]", code: "BracketRight",
    }))).toBe("next-terminal");
    expect(terminalCenterKeyAction(chord({
      metaKey: true, key: "[", code: "BracketLeft",
    }))).toBeNull();
  });
});

describe("terminal input ergonomics", () => {
  it("maps strict Command+Backspace to the terminal line-kill byte", () => {
    expect(terminalInputForKey(chord({
      key: "Backspace", code: "Backspace", metaKey: true,
    }))).toBe(TERMINAL_DELETE_TO_LINE_START);
    expect(TERMINAL_DELETE_TO_LINE_START).toBe("\u0015");
  });

  it("maps strict ⌘← / ⌘→ to readline's line-start / line-end bytes", () => {
    expect(terminalInputForKey(chord({ key: "ArrowLeft", code: "ArrowLeft", metaKey: true })))
      .toBe("\u0001");
    expect(terminalInputForKey(chord({ key: "ArrowRight", code: "ArrowRight", metaKey: true })))
      .toBe("\u0005");
  });

  it("leaves every other arrow chord with xterm (⌥ word motion, ⇧ selection, ⌘↑/↓)", () => {
    expect(terminalInputForKey(chord({ key: "ArrowLeft", code: "ArrowLeft" }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "ArrowLeft", code: "ArrowLeft", altKey: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "ArrowLeft", code: "ArrowLeft", metaKey: true, shiftKey: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "ArrowRight", code: "ArrowRight", metaKey: true, ctrlKey: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "ArrowLeft", code: "ArrowLeft", metaKey: true, isComposing: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({ key: "ArrowUp", code: "ArrowUp", metaKey: true })))
      .toBeNull();
    expect(terminalInputForKey(chord({ key: "ArrowDown", code: "ArrowDown", metaKey: true })))
      .toBeNull();
  });

  it("leaves every other Backspace chord with xterm and the child process", () => {
    expect(terminalInputForKey(chord({ key: "Backspace", code: "Backspace" }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "Backspace", code: "Backspace", ctrlKey: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "Backspace", code: "Backspace", metaKey: true, ctrlKey: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "Backspace", code: "Backspace", metaKey: true, altKey: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "Backspace", code: "Backspace", metaKey: true, shiftKey: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "Backspace", code: "Backspace", metaKey: true, isComposing: true,
    }))).toBeNull();
    expect(terminalInputForKey(chord({
      key: "Backspace", code: "Backspace", metaKey: true, keyCode: 229,
    }))).toBeNull();
    expect(terminalInputForKey(chord({ metaKey: true }))).toBeNull();
  });
});

describe("terminalOpensSearch", () => {
  it("opens the terminal search box on ⌘F and leaves Ctrl+F to the program", () => {
    expect(terminalOpensSearch(chord({ metaKey: true, key: "f", code: "KeyF" }))).toBe(true);
    expect(terminalOpensSearch(chord({ metaKey: true, key: "F", code: "KeyF" }))).toBe(true);
    expect(terminalOpensSearch(chord({ ctrlKey: true, key: "f", code: "KeyF" }))).toBe(false);
    expect(terminalOpensSearch(chord({ ctrlKey: true, shiftKey: true, key: "F", code: "KeyF" })))
      .toBe(false);
    expect(terminalOpensSearch(chord({ key: "f", code: "KeyF" }))).toBe(false);
  });

  it("refuses ⌃⌘F, ⇧⌘F, ⌥⌘F and IME composition on macOS, and uses Ctrl+Shift+F elsewhere", () => {
    expect(terminalOpensSearch(chord({ metaKey: true, ctrlKey: true, key: "f", code: "KeyF" }), true))
      .toBe(false);
    expect(terminalOpensSearch(chord({ metaKey: true, shiftKey: true, key: "F", code: "KeyF" }), true))
      .toBe(false);
    expect(terminalOpensSearch(chord({ metaKey: true, altKey: true, key: "f", code: "KeyF" }), true))
      .toBe(false);
    expect(terminalOpensSearch(chord({ metaKey: true, isComposing: true, key: "f", code: "KeyF" }), true))
      .toBe(false);
    expect(terminalOpensSearch(chord({ metaKey: true, keyCode: 229, key: "f", code: "KeyF" }), true))
      .toBe(false);

    expect(terminalOpensSearch(chord({ ctrlKey: true, shiftKey: true, key: "F", code: "KeyF" }), false))
      .toBe(true);
    expect(terminalOpensSearch(chord({ ctrlKey: true, shiftKey: true, key: "f", code: "KeyF" }), false))
      .toBe(true);
    expect(terminalOpensSearch(chord({ ctrlKey: true, key: "f", code: "KeyF" }), false)).toBe(false);
    expect(terminalOpensSearch(chord({ metaKey: true, key: "f", code: "KeyF" }), false)).toBe(false);
    expect(terminalOpensSearch(chord({
      ctrlKey: true, shiftKey: true, altKey: true, key: "F", code: "KeyF",
    }), false)).toBe(false);
    expect(terminalOpensSearch(chord({
      ctrlKey: true, shiftKey: true, metaKey: true, key: "F", code: "KeyF",
    }), false)).toBe(false);
    expect(terminalOpensSearch(chord({
      ctrlKey: true, shiftKey: true, isComposing: true, key: "F", code: "KeyF",
    }), false)).toBe(false);
  });
});

describe("shellOwnsChord", () => {
  it("gives a focused shell readline's Ctrl+P / Ctrl+N / Ctrl+K and Ctrl+Tab", () => {
    expect(shellOwnsChord(chord({ ctrlKey: true, key: "p", code: "KeyP" }), true)).toBe(true);
    expect(shellOwnsChord(chord({ ctrlKey: true, key: "n", code: "KeyN" }), true)).toBe(true);
    expect(shellOwnsChord(chord({ ctrlKey: true, key: "k", code: "KeyK" }), true)).toBe(true);
    expect(shellOwnsChord(chord({ ctrlKey: true, key: "K", code: "KeyK" }), true)).toBe(true);
    expect(shellOwnsChord(chord({ ctrlKey: true, key: "Tab", code: "Tab" }), true)).toBe(true);
    expect(shellOwnsChord(chord({
      ctrlKey: true, shiftKey: true, key: "Tab", code: "Tab",
    }), true)).toBe(true);
  });

  it("claims nothing when focus is outside a terminal pane", () => {
    expect(shellOwnsChord(chord({ ctrlKey: true, key: "p", code: "KeyP" }), false)).toBe(false);
    expect(shellOwnsChord(chord({ ctrlKey: true, key: "Tab", code: "Tab" }), false)).toBe(false);
  });

  it("leaves the ⌘ spellings, ⌘⇧P / ⌃⇧P and Ctrl+` with the app", () => {
    expect(shellOwnsChord(chord({ metaKey: true, key: "p", code: "KeyP" }), true)).toBe(false);
    expect(shellOwnsChord(chord({ metaKey: true, key: "n", code: "KeyN" }), true)).toBe(false);
    expect(shellOwnsChord(chord({ metaKey: true, key: "k", code: "KeyK" }), true)).toBe(false);
    expect(shellOwnsChord(chord({
      metaKey: true, ctrlKey: true, key: "p", code: "KeyP",
    }), true)).toBe(false);
    expect(shellOwnsChord(chord({
      ctrlKey: true, altKey: true, key: "p", code: "KeyP",
    }), true)).toBe(false);
    expect(shellOwnsChord(chord({
      ctrlKey: true, shiftKey: true, key: "P", code: "KeyP",
    }), true)).toBe(false);
    expect(shellOwnsChord(chord({ ctrlKey: true, key: "`", code: "Backquote" }), true))
      .toBe(false);
  });
});

describe("terminalReverseTabGoesToProgram", () => {
  it("claims plain Shift+Tab so it reaches the program instead of moving pane focus", () => {
    expect(terminalReverseTabGoesToProgram(chord({
      key: "Tab", code: "Tab", shiftKey: true,
    }))).toBe(true);
    // CSI Z is the reverse-tab report provider TUIs read as Shift+Tab.
    expect(TERMINAL_REVERSE_TAB).toBe("\x1b[Z");
  });

  it("leaves plain Tab and every other Tab chord with xterm and the app", () => {
    expect(terminalReverseTabGoesToProgram(chord({ key: "Tab", code: "Tab" }))).toBe(false);
    for (const over of [
      { ctrlKey: true }, { metaKey: true }, { altKey: true },
      { ctrlKey: true, metaKey: true }, { isComposing: true }, { keyCode: 229 },
    ]) {
      expect(terminalReverseTabGoesToProgram(chord({
        key: "Tab", code: "Tab", shiftKey: true, ...over,
      }))).toBe(false);
    }
    expect(terminalReverseTabGoesToProgram(chord({ shiftKey: true }))).toBe(false);
  });
});

describe("terminalCopyGoesToProgram", () => {
  const codex = { hasSelection: false, mouseTracking: true, readsKeyReports: true };
  const commandC = chord({ metaKey: true, key: "c", code: "KeyC" });

  it("sends ⌘C to a program that owns the pointer and reads kitty key reports", () => {
    expect(terminalCopyGoesToProgram(commandC, codex)).toBe(true);
    expect(terminalCopyGoesToProgram({ ...commandC, key: "C" }, codex)).toBe(true);
    // Kitty's CSI u spelling of Super+C: codepoint 99, modifiers 1 + 8.
    expect(TERMINAL_PROGRAM_COPY_KEY).toBe("\x1b[99;9u");
  });

  it("keeps ⌘C with xterm when xterm holds the selection", () => {
    expect(terminalCopyGoesToProgram(commandC, { ...codex, hasSelection: true })).toBe(false);
  });

  it("keeps ⌘C when the program has not taken the pointer or cannot read key reports", () => {
    expect(terminalCopyGoesToProgram(commandC, { ...codex, mouseTracking: false })).toBe(false);
    // vim, htop and tmux capture the mouse without kitty reports; ESC [ … u would be keystrokes.
    expect(terminalCopyGoesToProgram(commandC, { ...codex, readsKeyReports: false })).toBe(false);
  });

  it("leaves every other chord alone", () => {
    for (const over of [
      { metaKey: false, ctrlKey: true }, { shiftKey: true }, { altKey: true }, { ctrlKey: true },
      { isComposing: true }, { key: "v", code: "KeyV" }, { key: "x", code: "KeyX" },
    ]) {
      expect(terminalCopyGoesToProgram({ ...commandC, ...over }, codex)).toBe(false);
    }
  });
});
