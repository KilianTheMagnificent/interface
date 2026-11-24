import { describe, expect, it } from "vitest";
import {
  EMPTY_WORKSPACE_STATE,
  MAX_WORKSPACE_TILES,
  hasWorkspacePaneCapacity,
  mayOpenWorkspaceKeyWithReservation,
  closeWorkspaceKey,
  cycleWorkspaceFocus,
  openWorkspaceKey,
  parseWorkspaceState,
  reconcileWorkspaceState,
  switchWorkspaceKey,
  toggleWorkspaceMaximized,
  closeChordTarget,
  workspaceOverlayShortcut,
  type WorkspaceState,
} from "./workspace";

const state = (openKeys: string[], focusedKey: string | null = openKeys[0] ?? null,
    maximizedKey: string | null = null): WorkspaceState =>
  ({ openKeys, focusedKey, maximizedKey });

describe("parseWorkspaceState", () => {
  it.each([null, undefined, "", "not json", "null", "[]", "42", "{}"])(
    "turns malformed or wrong-shaped persistence into empty state (%s)", (raw) => {
      expect(parseWorkspaceState(raw)).toEqual(EMPTY_WORKSPACE_STATE);
    });

  it("deduplicates keys, ignores malformed entries, and applies the four-tile cap", () => {
    const parsed = parseWorkspaceState(JSON.stringify({
      openKeys: ["a", "b", "a", "", null, 7, "c", "d", "e"],
      focusedKey: "c",
      maximizedKey: null,
    }));
    expect(parsed).toEqual({
      openKeys: ["a", "b", "c", "d"],
      focusedKey: "c",
      maximizedKey: null,
    });
    expect(parsed.openKeys).toHaveLength(MAX_WORKSPACE_TILES);
  });

  it("falls back to the first open key and clears a dangling maximized key", () => {
    expect(parseWorkspaceState(JSON.stringify({
      openKeys: ["a", "b"], focusedKey: "missing", maximizedKey: "also-missing",
    }))).toEqual({ openKeys: ["a", "b"], focusedKey: "a", maximizedKey: null });
  });

  it("makes a valid maximized key the focus", () => {
    expect(parseWorkspaceState(JSON.stringify({
      openKeys: ["a", "b"], focusedKey: "a", maximizedKey: "b",
    }))).toEqual({ openKeys: ["a", "b"], focusedKey: "b", maximizedKey: "b" });
  });
});

describe("reconcileWorkspaceState", () => {
  it("would erase a saved layout against an empty cached registry, so only the listing prunes", () => {
    // The host paints its cached registry before the first listing answers;
    // after a quarantined cache that push is empty. Reconciling against it
    // would drop every saved tile, which is why App prunes only once the first
    // listing has answered (pinned in workspace-ui.test.ts).
    const saved = state(["codex#a", "claude#b"], "claude#b");
    expect(reconcileWorkspaceState(saved, [])).toEqual(EMPTY_WORKSPACE_STATE);
    expect(reconcileWorkspaceState(saved, ["claude#b", "codex#a"])).toBe(saved);
  });

  it("drops unknown keys while preserving the surviving order and focus", () => {
    expect(reconcileWorkspaceState(state(["a", "b", "c"], "c"), ["c", "a", "a"]))
      .toEqual({ openKeys: ["a", "c"], focusedKey: "c", maximizedKey: null });
  });

  it("chooses the first survivor when the focused key disappeared", () => {
    expect(reconcileWorkspaceState(state(["a", "b", "c"], "b"), ["a", "c"]))
      .toEqual({ openKeys: ["a", "c"], focusedKey: "a", maximizedKey: null });
  });

  it("clears unknown maximized state and returns empty when nothing is known", () => {
    expect(reconcileWorkspaceState(state(["a", "b"], "b", "b"), ["a"]))
      .toEqual({ openKeys: ["a"], focusedKey: "a", maximizedKey: null });
    expect(reconcileWorkspaceState(state(["a"]), [])).toEqual(EMPTY_WORKSPACE_STATE);
  });

  it("returns the same object when nothing was dropped, so effects can bail out", () => {
    // Every registry push reconciles all three slots; a fresh object per call
    // meant a state write, an App render and three localStorage writes with
    // nothing moved.
    const current = state(["a", "b"], "b");
    expect(reconcileWorkspaceState(current, ["b", "a", "c"])).toBe(current);
    const maximized = state(["a", "b"], "b", "b");
    expect(reconcileWorkspaceState(maximized, ["a", "b"])).toBe(maximized);
    expect(reconcileWorkspaceState(EMPTY_WORKSPACE_STATE, ["a"])).toBe(EMPTY_WORKSPACE_STATE);
  });

  it("normalizes a corrupt in-memory state before reconciling it", () => {
    const corrupt = {
      openKeys: ["a", "a", 3, "b", "c", "d", "e"],
      focusedKey: 3,
      maximizedKey: "missing",
    } as unknown as WorkspaceState;
    expect(reconcileWorkspaceState(corrupt, ["a", "b", "c", "d", "e"]))
      .toEqual({ openKeys: ["a", "b", "c", "d"], focusedKey: "a", maximizedKey: null });
  });
});

describe("openWorkspaceKey", () => {
  it("appends and focuses a new key", () => {
    expect(openWorkspaceKey(state(["a", "b"]), "c"))
      .toEqual({ openKeys: ["a", "b", "c"], focusedKey: "c", maximizedKey: null });
  });

  it("focuses an existing key without reordering it", () => {
    expect(openWorkspaceKey(state(["a", "b", "c"], "c"), "b"))
      .toEqual({ openKeys: ["a", "b", "c"], focusedKey: "b", maximizedKey: null });
  });

  it("refuses a fifth key rather than silently evicting a mounted draft", () => {
    expect(openWorkspaceKey(state(["a", "b", "c", "d"], "d"), "e"))
      .toEqual({ openKeys: ["a", "b", "c", "d"], focusedKey: "d", maximizedKey: null });
  });

  it("restores the grid when explicitly appending from maximized mode", () => {
    expect(openWorkspaceKey(state(["a", "b"], "a", "a"), "c"))
      .toEqual({ openKeys: ["a", "b", "c"], focusedKey: "c", maximizedKey: null });
  });

  it("retains maximized mode when focusing a pane that is already open", () => {
    expect(openWorkspaceKey(state(["a", "b"], "a", "a"), "b"))
      .toEqual({ openKeys: ["a", "b"], focusedKey: "b", maximizedKey: "b" });
  });

  it("ignores an empty key", () => {
    expect(openWorkspaceKey(state(["a"]), "")).toEqual(state(["a"]));
  });
});

describe("hasWorkspacePaneCapacity", () => {
  it("preflights an atomic two-pane workflow without mutating the workspace", () => {
    const two = state(["a", "b"]);
    expect(hasWorkspacePaneCapacity(two, 2)).toBe(true);
    expect(hasWorkspacePaneCapacity(two, 3)).toBe(false);
    expect(hasWorkspacePaneCapacity(state(["a", "b", "c"]), 2)).toBe(false);
    expect(hasWorkspacePaneCapacity(two, -1)).toBe(false);
    expect(two).toEqual(state(["a", "b"]));
  });
});

describe("mayOpenWorkspaceKeyWithReservation", () => {
  it("allows focus but blocks every new-key append for the lifetime of a reservation", () => {
    const current = state(["a"]);
    expect(mayOpenWorkspaceKeyWithReservation(current, "a", 2)).toBe(true);
    expect(mayOpenWorkspaceKeyWithReservation(current, "b", 2)).toBe(false);
    expect(mayOpenWorkspaceKeyWithReservation(current, "c", 2)).toBe(false);
    expect(mayOpenWorkspaceKeyWithReservation(current, "b", 0)).toBe(true);
  });
});

describe("switchWorkspaceKey", () => {
  it("opens and focuses the target when the workspace is empty", () => {
    expect(switchWorkspaceKey(EMPTY_WORKSPACE_STATE, "a"))
      .toEqual({ openKeys: ["a"], focusedKey: "a", maximizedKey: null });
  });

  it("focuses an already-open target without reordering or changing pane count", () => {
    expect(switchWorkspaceKey(state(["a", "b", "c"], "c"), "b"))
      .toEqual({ openKeys: ["a", "b", "c"], focusedKey: "b", maximizedKey: null });
  });

  it("replaces the focused slot in place for an unfamiliar target", () => {
    expect(switchWorkspaceKey(state(["a", "b", "c"], "b"), "d"))
      .toEqual({ openKeys: ["a", "d", "c"], focusedKey: "d", maximizedKey: null });
  });

  it("replaces the focused slot even at the four-pane ceiling", () => {
    expect(switchWorkspaceKey(state(["a", "b", "c", "d"], "c"), "e"))
      .toEqual({ openKeys: ["a", "b", "e", "d"], focusedKey: "e", maximizedKey: null });
  });

  it("transfers maximized mode to an already-open target", () => {
    expect(switchWorkspaceKey(state(["a", "b", "c"], "a", "a"), "c"))
      .toEqual({ openKeys: ["a", "b", "c"], focusedKey: "c", maximizedKey: "c" });
  });

  it("transfers maximized mode to a replacement target", () => {
    expect(switchWorkspaceKey(state(["a", "b"], "b", "b"), "c"))
      .toEqual({ openKeys: ["a", "c"], focusedKey: "c", maximizedKey: "c" });
  });

  it("ignores an invalid target without changing a valid workspace", () => {
    const original = state(["a", "b"], "b");
    expect(switchWorkspaceKey(original, "")).toEqual(original);
    expect(switchWorkspaceKey(original, null as unknown as string)).toEqual(original);
  });

  it("normalizes corrupt in-memory state before switching", () => {
    const corrupt = {
      openKeys: ["a", "a", 3, "b", "c", "d", "e"],
      focusedKey: "missing",
      maximizedKey: "also-missing",
    } as unknown as WorkspaceState;
    expect(switchWorkspaceKey(corrupt, "z"))
      .toEqual({ openKeys: ["z", "b", "c", "d"], focusedKey: "z", maximizedKey: null });
  });
});

describe("closeWorkspaceKey", () => {
  it("keeps focus when closing another tile", () => {
    expect(closeWorkspaceKey(state(["a", "b", "c"], "a"), "b"))
      .toEqual({ openKeys: ["a", "c"], focusedKey: "a", maximizedKey: null });
  });

  it("focuses the right neighbour, then the left neighbour at the end", () => {
    expect(closeWorkspaceKey(state(["a", "b", "c"], "b"), "b"))
      .toEqual({ openKeys: ["a", "c"], focusedKey: "c", maximizedKey: null });
    expect(closeWorkspaceKey(state(["a", "b", "c"], "c"), "c"))
      .toEqual({ openKeys: ["a", "b"], focusedKey: "b", maximizedKey: null });
  });

  it("returns empty after closing the only tile", () => {
    expect(closeWorkspaceKey(state(["a"]), "a")).toEqual(EMPTY_WORKSPACE_STATE);
  });

  it("clears maximized mode when its tile closes", () => {
    expect(closeWorkspaceKey(state(["a", "b"], "b", "b"), "b"))
      .toEqual({ openKeys: ["a"], focusedKey: "a", maximizedKey: null });
  });

  it("ignores a key that is not open", () => {
    expect(closeWorkspaceKey(state(["a", "b"], "b"), "missing"))
      .toEqual(state(["a", "b"], "b"));
  });
});

describe("cycleWorkspaceFocus", () => {
  it("cycles next and previous with wraparound without reordering", () => {
    const original = state(["a", "b", "c"], "c");
    expect(cycleWorkspaceFocus(original, "next"))
      .toEqual({ openKeys: ["a", "b", "c"], focusedKey: "a", maximizedKey: null });
    expect(cycleWorkspaceFocus(state(["a", "b", "c"], "a"), "previous"))
      .toEqual({ openKeys: ["a", "b", "c"], focusedKey: "c", maximizedKey: null });
  });

  it("moves maximized mode with focus", () => {
    expect(cycleWorkspaceFocus(state(["a", "b"], "a", "a"), "next"))
      .toEqual({ openKeys: ["a", "b"], focusedKey: "b", maximizedKey: "b" });
  });

  it("is stable for zero or one tile", () => {
    expect(cycleWorkspaceFocus(EMPTY_WORKSPACE_STATE, "next"))
      .toEqual(EMPTY_WORKSPACE_STATE);
    expect(cycleWorkspaceFocus(state(["a"]), "previous")).toEqual(state(["a"]));
  });
});

describe("toggleWorkspaceMaximized", () => {
  it("maximizes and focuses an open key, then restores the grid", () => {
    const maximized = toggleWorkspaceMaximized(state(["a", "b"], "a"), "b");
    expect(maximized).toEqual({ openKeys: ["a", "b"], focusedKey: "b", maximizedKey: "b" });
    expect(toggleWorkspaceMaximized(maximized, "b"))
      .toEqual({ openKeys: ["a", "b"], focusedKey: "b", maximizedKey: null });
  });

  it("ignores a key that is not open", () => {
    expect(toggleWorkspaceMaximized(state(["a"]), "missing")).toEqual(state(["a"]));
  });
});

describe("transitions that change nothing", () => {
  it("return the caller's own object, so a pane click neither renders nor persists", () => {
    // Every pointerdown and focus inside a tile re-focuses its session.
    const focused = state(["a", "b"], "b");
    expect(openWorkspaceKey(focused, "b")).toBe(focused);
    expect(switchWorkspaceKey(focused, "b")).toBe(focused);
    const maximized = state(["a", "b"], "b", "b");
    expect(openWorkspaceKey(maximized, "b")).toBe(maximized);
    expect(switchWorkspaceKey(maximized, "b")).toBe(maximized);
    expect(switchWorkspaceKey(focused, "")).toBe(focused);
    expect(openWorkspaceKey(focused, "")).toBe(focused);
    const full = state(["a", "b", "c", "d"], "a");
    expect(openWorkspaceKey(full, "e")).toBe(full);
    expect(toggleWorkspaceMaximized(focused, "missing")).toBe(focused);
  });

  it("except close and cycle, whose fresh state spends App's pending tile focus", () => {
    const focused = state(["a", "b"], "b");
    expect(closeWorkspaceKey(focused, "missing")).toEqual(focused);
    expect(closeWorkspaceKey(focused, "missing")).not.toBe(focused);
    const single = state(["a"]);
    expect(cycleWorkspaceFocus(single, "next")).toEqual(single);
    expect(cycleWorkspaceFocus(single, "next")).not.toBe(single);
  });

  it("still return a new object for every real change", () => {
    const focused = state(["a", "b"], "b");
    expect(openWorkspaceKey(focused, "a")).not.toBe(focused);
    expect(switchWorkspaceKey(focused, "c")).not.toBe(focused);
    expect(closeWorkspaceKey(focused, "a")).not.toBe(focused);
    expect(cycleWorkspaceFocus(focused, "next")).not.toBe(focused);
    expect(toggleWorkspaceMaximized(focused, "b")).not.toBe(focused);
  });

  it("replace a state normalize() had to repair, even when the key is focused", () => {
    const corrupt = { openKeys: ["a", "a", "b"], focusedKey: "a", maximizedKey: null } as
      unknown as WorkspaceState;
    expect(switchWorkspaceKey(corrupt, "a"))
      .toEqual({ openKeys: ["a", "b"], focusedKey: "a", maximizedKey: null });
    expect(switchWorkspaceKey(corrupt, "a")).not.toBe(corrupt);
    const missingField = { openKeys: ["a"], focusedKey: "a" } as unknown as WorkspaceState;
    expect(openWorkspaceKey(missingField, "a"))
      .toEqual({ openKeys: ["a"], focusedKey: "a", maximizedKey: null });
    expect(openWorkspaceKey(null as unknown as WorkspaceState, "a"))
      .toEqual({ openKeys: ["a"], focusedKey: "a", maximizedKey: null });
    expect(toggleWorkspaceMaximized(null as unknown as WorkspaceState, "a"))
      .toBe(EMPTY_WORKSPACE_STATE);
  });
});

describe("closeChordTarget", () => {
  const code = {
    composerOpen: false, settingsOpen: false, foregroundDialog: false,
    terminalsShown: true, sessionsShown: false, hasFocusedTile: false,
  };
  const threads = { ...code, terminalsShown: false, sessionsShown: true, hasFocusedTile: true };

  it("⌘W closes the selected pane, never the window", () => {
    expect(closeChordTarget(code)).toBe("terminal-pane");
    expect(closeChordTarget(threads)).toBe("tile");
    expect(closeChordTarget({ ...threads, hasFocusedTile: false })).toBe("none");
    expect(closeChordTarget({ ...code, terminalsShown: false })).toBe("none");
  });

  it("closes what covers the panes first and nothing behind a dialog", () => {
    expect(closeChordTarget({ ...code, settingsOpen: true })).toBe("settings");
    expect(closeChordTarget({ ...code, foregroundDialog: true })).toBe("none");
    expect(closeChordTarget({ ...code, settingsOpen: true, foregroundDialog: true })).toBe("none");
    expect(closeChordTarget({ ...threads, composerOpen: true })).toBe("none");
  });
});

describe("workspaceOverlayShortcut", () => {
  const shiftP = { key: "p", metaKey: true, ctrlKey: false, shiftKey: true, altKey: false };
  const plainP = { key: "p", metaKey: true, ctrlKey: false, shiftKey: false, altKey: false };
  const overlay = {
    composerOpen: false, settingsOpen: false, foregroundDialog: false,
  };

  it("keeps the command palette reachable from New Chat", () => {
    expect(workspaceOverlayShortcut(shiftP, { ...overlay, composerOpen: true }))
      .toBe("palette");
    expect(workspaceOverlayShortcut(plainP, { ...overlay, composerOpen: true }))
      .toBe("prevent-browser");
    expect(workspaceOverlayShortcut(shiftP, overlay)).toBe("palette");
  });

  it("does not open a second palette over an existing dialog", () => {
    expect(workspaceOverlayShortcut(shiftP, { ...overlay, foregroundDialog: true }))
      .toBe("prevent-browser");
    expect(workspaceOverlayShortcut(shiftP, { ...overlay, settingsOpen: true }))
      .toBe("prevent-browser");
  });
});
