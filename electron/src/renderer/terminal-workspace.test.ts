import { describe, expect, it } from "vitest";
import {
  DEFAULT_TERMINAL_SPLIT_RATIO,
  EMPTY_TERMINAL_WORKSPACE_STATE,
  MAX_TERMINAL_PANES,
  MAX_TERMINAL_PANES_HARD,
  MAX_TERMINAL_SPLIT_RATIO,
  MAX_TERMINAL_TABS_PER_PANE,
  MIN_TERMINAL_SPLIT_RATIO,
  TERMINAL_WORKSPACE_VERSION,
  attachTerminalsBalanced,
  browserTabIdsInWorkspaceState,
  browserTabPaneId,
  browserTabUrl,
  closeTerminalTab,
  cycleTerminalFocus,
  focusTerminalPane,
  focusTerminalTab,
  isBrowserTabId,
  moveTerminalTab,
  newBrowserTabId,
  normalizeTerminalWorkspaceState,
  onceTerminalIds,
  openBrowserTabBeside,
  openBrowserTabInPane,
  openTerminalTab,
  parseTerminalWorkspaceState,
  placePassedOffTerminal,
  reconcileActiveTerminalWorkspaceState,
  reconcileTerminalWorkspaceState,
  replaceTerminalTab,
  serializeTerminalWorkspaceState,
  SAVED_LAYOUT_FACTS_LIMIT,
  savedLayoutFacts,
  setBrowserTabUrl,
  setTerminalSplitRatio,
  setTerminalWorkspaceArrangement,
  splitTerminalPane,
  splitTerminalPaneInPlace,
  terminalIdsInWorkspaceState,
  terminalPaneById,
  terminalPaneForTerminal,
  terminalWorkspacePaneOrder,
  tidyTerminalWorkspace,
  toggleTerminalPaneMaximized,
  type TerminalSplitNode,
  type TerminalSplitOrientation,
  type TerminalSplitTree,
  type TerminalWorkspaceState,
} from "./terminal-workspace";

const opened = (paneId = "pane-a", terminalId = "terminal-a"): TerminalWorkspaceState =>
  openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, paneId, terminalId);

describe("terminal workspace persistence", () => {
  it.each([null, undefined, "", "not json", "null", "[]", "{}"])(
    "fails closed for malformed or wrong-shaped state (%s)", raw => {
      expect(parseTerminalWorkspaceState(raw)).toEqual(EMPTY_TERMINAL_WORKSPACE_STATE);
    });

  it("lists every tab id currently shown in the pane tree", () => {
    const state = attachTerminalsBalanced(
      EMPTY_TERMINAL_WORKSPACE_STATE,
      ["terminal-a", "terminal-b"],
      index => `pane-${index + 1}`,
      index => `split-${index + 1}`,
    );
    expect([...terminalIdsInWorkspaceState(state)]).toEqual(["terminal-a", "terminal-b"]);
    expect([...terminalIdsInWorkspaceState(EMPTY_TERMINAL_WORKSPACE_STATE)]).toEqual([]);
  });

  it("tracks opens and closes without changing when a tab moves", () => {
    let state = attachTerminalsBalanced(
      EMPTY_TERMINAL_WORKSPACE_STATE,
      ["terminal-a", "terminal-b"],
      index => `pane-${index + 1}`,
      index => `split-${index + 1}`,
    );
    expect(terminalIdsInWorkspaceState(state).size).toBe(2);
    state = moveTerminalTab(state, "terminal-a", "pane-2", 0);
    expect(terminalIdsInWorkspaceState(state).size).toBe(2);
    state = closeTerminalTab(state, "terminal-b");
    expect(terminalIdsInWorkspaceState(state).size).toBe(1);
    state = openTerminalTab(state, "pane-2", "terminal-b");
    expect(terminalIdsInWorkspaceState(state).size).toBe(2);
  });

  it("rejects unknown schema versions instead of guessing a migration", () => {
    expect(normalizeTerminalWorkspaceState({
      version: 2,
      panes: [{ id: "pane-a", tabs: [{ terminalId: "terminal-a" }],
        activeTerminalId: "terminal-a" }],
      root: { kind: "pane", paneId: "pane-a" },
    })).toEqual(EMPTY_TERMINAL_WORKSPACE_STATE);
  });

  it("deduplicates terminal identity globally, clamps ratios, and drops unreachable panes", () => {
    const normalized = normalizeTerminalWorkspaceState({
      version: TERMINAL_WORKSPACE_VERSION,
      panes: [
        { id: "pane-a", tabs: [{ terminalId: "terminal-a" }, { terminalId: "terminal-b" }],
          activeTerminalId: "missing" },
        { id: "pane-b", tabs: [{ terminalId: "terminal-b" }, { terminalId: "terminal-c" }],
          activeTerminalId: "terminal-c" },
        { id: "unreachable", tabs: [{ terminalId: "terminal-d" }],
          activeTerminalId: "terminal-d" },
      ],
      root: {
        kind: "split", id: "split-a", orientation: "invalid", ratio: 4,
        first: { kind: "pane", paneId: "pane-a" },
        second: { kind: "pane", paneId: "pane-b" },
      },
      focusedPaneId: "missing",
      maximizedPaneId: "pane-b",
    });
    expect(normalized.root).toEqual({
      kind: "split", id: "split-a", orientation: "horizontal",
      ratio: MAX_TERMINAL_SPLIT_RATIO,
      first: { kind: "pane", paneId: "pane-a" },
      second: { kind: "pane", paneId: "pane-b" },
    });
    expect(normalized.panes).toEqual([
      { id: "pane-a", tabs: [{ terminalId: "terminal-a" }, { terminalId: "terminal-b" }],
        activeTerminalId: "terminal-a" },
      { id: "pane-b", tabs: [{ terminalId: "terminal-c" }],
        activeTerminalId: "terminal-c" },
    ]);
    expect(normalized.focusedPaneId).toBe("pane-b");
    expect(normalized.maximizedPaneId).toBe("pane-b");
  });

  it("caps untrusted panes and tabs without retaining dangling tree leaves", () => {
    const panes = Array.from({ length: MAX_TERMINAL_PANES_HARD + 3 }, (_, pane) => ({
      id: `pane-${pane}`,
      tabs: Array.from({ length: MAX_TERMINAL_TABS_PER_PANE + 3 }, (_, tab) => ({
        terminalId: `terminal-${pane}-${tab}`,
      })),
      activeTerminalId: `terminal-${pane}-0`,
    }));
    // Only the first accepted pane is reachable; the rest of the untrusted
    // collection must not leak into persisted workspace state.
    const normalized = normalizeTerminalWorkspaceState({
      version: TERMINAL_WORKSPACE_VERSION,
      panes,
      root: { kind: "pane", paneId: "pane-0" },
    });
    expect(normalized.panes).toHaveLength(1);
    expect(normalized.panes[0].tabs).toHaveLength(MAX_TERMINAL_TABS_PER_PANE);
  });

  it("round-trips only the normalized versioned layout", () => {
    const state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "vertical",
    });
    expect(parseTerminalWorkspaceState(serializeTerminalWorkspaceState(state))).toEqual(state);
  });

  it("preserves old saved trees as custom splits and accepts explicit equal grids", () => {
    const legacy = normalizeTerminalWorkspaceState({
      version: TERMINAL_WORKSPACE_VERSION,
      panes: [{ id: "pane-a", tabs: [{ terminalId: "terminal-a" }],
        activeTerminalId: "terminal-a" }],
      root: { kind: "pane", paneId: "pane-a" },
      focusedPaneId: "pane-a",
    });
    expect(legacy.arrangement).toBe("custom-split");
    expect(normalizeTerminalWorkspaceState({
      ...legacy,
      arrangement: "equal-grid",
    }).arrangement).toBe("equal-grid");
    expect(normalizeTerminalWorkspaceState({
      ...legacy,
      arrangement: "unknown",
    }).arrangement).toBe("custom-split");
  });
});

describe("savedLayoutFacts", () => {
  it("parses a saved layout once, and starts over past its bound", () => {
    const state = openTerminalTab(openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "t-1"),
      "pane-a", newBrowserTabId(() => "page-1"));
    const layout = serializeTerminalWorkspaceState(state);
    const facts = savedLayoutFacts(layout);
    expect(facts.state).toEqual(parseTerminalWorkspaceState(layout));
    expect([...facts.terminalIds]).toEqual(["t-1"]);
    expect(facts.terminalIdList).toEqual(["t-1"]);
    expect(facts.browserTabIds.size).toBe(1);
    expect(savedLayoutFacts(layout)).toBe(facts);
    expect(savedLayoutFacts(undefined).state).toBe(EMPTY_TERMINAL_WORKSPACE_STATE);
    for (let index = 0; index < SAVED_LAYOUT_FACTS_LIMIT; index++) {
      savedLayoutFacts(serializeTerminalWorkspaceState(
        openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", `t-bound-${index}`)));
    }
    expect(savedLayoutFacts(layout)).not.toBe(facts);
    expect(savedLayoutFacts(layout)).toEqual(facts);
  });
});

describe("terminal workspace reconciliation", () => {
  it("drops dead terminals, chooses a surviving active tab, and collapses empty splits", () => {
    let state = openTerminalTab(opened(), "pane-a", "terminal-b");
    state = splitTerminalPane(state, {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-c", orientation: "horizontal",
    });
    state = focusTerminalTab(state, "terminal-b");
    expect(reconcileTerminalWorkspaceState(state, ["terminal-a", "terminal-c"]))
      .toMatchObject({
        panes: [
          { id: "pane-a", tabs: [{ terminalId: "terminal-a" }],
            activeTerminalId: "terminal-a" },
          { id: "pane-b", tabs: [{ terminalId: "terminal-c" }],
            activeTerminalId: "terminal-c" },
        ],
      });
    const collapsed = reconcileTerminalWorkspaceState(state, ["terminal-c"]);
    expect(collapsed.root).toEqual({ kind: "pane", paneId: "pane-b" });
    expect(collapsed.focusedPaneId).toBe("pane-b");
  });

  it("returns empty when the host reports no live terminal ids", () => {
    expect(reconcileTerminalWorkspaceState(opened(), [])).toEqual(EMPTY_TERMINAL_WORKSPACE_STATE);
  });

  it("ignores a stale roster captured for a workspace that is no longer active", () => {
    const restored = opened();
    expect(reconcileActiveTerminalWorkspaceState(
      restored, [], "workspace-empty", "workspace-restored", restored,
    )).toBe(restored);
    expect(reconcileActiveTerminalWorkspaceState(
      restored, [], "workspace-restored", "workspace-restored", restored,
    )).toEqual(EMPTY_TERMINAL_WORKSPACE_STATE);
  });

  it("does not let an older roster prune a newer layout in the same workspace", () => {
    const observed = opened();
    const attached = openTerminalTab(observed, "pane-a", "terminal-new");
    expect(reconcileActiveTerminalWorkspaceState(
      attached, ["terminal-a"], "workspace-a", "workspace-a", observed,
    )).toBe(attached);
    expect(terminalIdsInWorkspaceState(attached)).toEqual(
      new Set(["terminal-a", "terminal-new"]),
    );
  });

  it("preserves layout identity when reconciliation removes nothing", () => {
    const state = opened();
    expect(reconcileActiveTerminalWorkspaceState(
      state, ["terminal-a"], "workspace-a", "workspace-a", state,
    )).toBe(state);
  });
});

describe("terminal tab and pane actions", () => {
  it("opens tabs and focuses an existing terminal globally", () => {
    let state = openTerminalTab(opened(), "pane-a", "terminal-b");
    state = openTerminalTab(state, "pane-a", "terminal-c");
    expect(terminalPaneById(state, "pane-a")?.activeTerminalId).toBe("terminal-c");
    expect(openTerminalTab(state, "missing-pane", "terminal-b"))
      .toEqual(focusTerminalTab(state, "terminal-b"));
  });

  it("closes the active tab to its right, then collapses an empty pane", () => {
    let state = openTerminalTab(opened(), "pane-a", "terminal-b");
    state = openTerminalTab(state, "pane-a", "terminal-c");
    state = focusTerminalTab(state, "terminal-b");
    state = closeTerminalTab(state, "terminal-b");
    expect(state.panes[0].activeTerminalId).toBe("terminal-c");
    state = closeTerminalTab(closeTerminalTab(state, "terminal-a"), "terminal-c");
    expect(state).toEqual(EMPTY_TERMINAL_WORKSPACE_STATE);
  });

  it("replaces a tab in place, keeping its slot and its pane", () => {
    let state = openTerminalTab(opened(), "pane-a", "terminal-b");
    state = openTerminalTab(state, "pane-a", "terminal-c");
    state = focusTerminalTab(state, "terminal-b");
    const replaced = replaceTerminalTab(state, "terminal-b", "terminal-shell");
    expect(replaced.panes[0].tabs.map(tab => tab.terminalId))
      .toEqual(["terminal-a", "terminal-shell", "terminal-c"]);
    expect(replaced.panes[0].activeTerminalId).toBe("terminal-shell");
    // A background tab keeps whichever terminal the pane was showing.
    expect(replaceTerminalTab(state, "terminal-c", "terminal-shell").panes[0].activeTerminalId)
      .toBe("terminal-b");
  });

  it("only replaces a tab that is on screen, and never duplicates one", () => {
    const state = openTerminalTab(opened(), "pane-a", "terminal-b");
    expect(replaceTerminalTab(state, "terminal-missing", "terminal-shell")).toEqual(state);
    expect(replaceTerminalTab(state, "terminal-a", "")).toEqual(state);
    expect(replaceTerminalTab(state, "terminal-a", "terminal-b"))
      .toEqual(closeTerminalTab(state, "terminal-a"));
  });

  it("splits in either placement, validates identity, and clamps the ratio", () => {
    const state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "vertical", placement: "first", ratio: -3,
    });
    expect(state.root).toEqual({
      kind: "split", id: "split-a", orientation: "vertical", ratio: MIN_TERMINAL_SPLIT_RATIO,
      first: { kind: "pane", paneId: "pane-b" },
      second: { kind: "pane", paneId: "pane-a" },
    });
    expect(state.focusedPaneId).toBe("pane-b");
    expect(splitTerminalPane(state, {
      paneId: "pane-a", newPaneId: "pane-c", splitId: "split-a",
      terminalId: "terminal-c", orientation: "horizontal",
    })).toEqual(state);
  });

  it("updates one split ratio and ignores unknown split ids", () => {
    const state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "horizontal",
    });
    expect(setTerminalSplitRatio(state, "split-a", Number.NaN).root).toMatchObject({
      ratio: DEFAULT_TERMINAL_SPLIT_RATIO,
    });
    const equalGrid = tidyTerminalWorkspace(state, index => `tidy-ratio-${index}`);
    expect(equalGrid.arrangement).toBe("equal-grid");
    expect(setTerminalSplitRatio(equalGrid, "tidy-ratio-0", 0.6).arrangement)
      .toBe("custom-split");
    // Same object, not merely an equal one: the terminal center's reconciliation
    // effect watches layout identity, so an equivalent copy makes it feed itself.
    expect(setTerminalSplitRatio(state, "missing", 0.25)).toBe(state);
  });

  it("keeps the state when a drag against a bound asks for the ratio it already has", () => {
    const pair = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "horizontal",
    });
    const state = splitTerminalPane(pair, {
      paneId: "pane-b", newPaneId: "pane-c", splitId: "split-b",
      terminalId: "terminal-c", orientation: "vertical",
    });
    const atBound = setTerminalSplitRatio(state, "split-a", 0.95);
    expect((atBound.root as TerminalSplitNode).ratio).toBe(0.8);
    expect(setTerminalSplitRatio(atBound, "split-a", 0.99)).toBe(atBound);
    expect(setTerminalSplitRatio(atBound, "split-a", 0.8)).toBe(atBound);
    // Moving one split rebuilds only its path; the other split keeps identity.
    const inner = setTerminalSplitRatio(atBound, "split-b", 0.3);
    expect((inner.root as TerminalSplitNode).first).toBe((atBound.root as TerminalSplitNode).first);
    expect(inner.panes).toBe(atBound.panes);
    const outer = setTerminalSplitRatio(inner, "split-a", 0.6);
    expect((outer.root as TerminalSplitNode).second).toBe((inner.root as TerminalSplitNode).second);
    // An equal ratio still leaves the tidy grid for a draggable split.
    const grid = tidyTerminalWorkspace(pair, index => `tidy-${index}`);
    const gridRatio = (grid.root as TerminalSplitNode).ratio;
    const custom = setTerminalSplitRatio(grid, (grid.root as TerminalSplitNode).id, gridRatio);
    expect(custom.arrangement).toBe("custom-split");
    expect(custom.root).toBe(grid.root);
  });

  it("returns the caller's own state object when a mutator changes nothing", () => {
    const state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "horizontal",
    });
    // These take an already-typed TerminalWorkspaceState, so a no-op must not
    // re-validate it into a fresh identity. parse() remains the validating door.
    expect(focusTerminalPane(state, "pane-missing")).toBe(state);
    expect(focusTerminalTab(state, "terminal-missing")).toBe(state);
    expect(closeTerminalTab(state, "terminal-missing")).toBe(state);
    expect(moveTerminalTab(state, "terminal-missing", "pane-a")).toBe(state);
    expect(toggleTerminalPaneMaximized(state, "pane-missing")).toBe(state);
    expect(setTerminalWorkspaceArrangement(state, state.arrangement)).toBe(state);
    expect(attachTerminalsBalanced(state, ["terminal-a"],
      index => `late-pane-${index}`, index => `late-split-${index}`)).toBe(state);
    expect(reconcileTerminalWorkspaceState(state, ["terminal-a", "terminal-b"])).toBe(state);
  });

  it("leaves the equal grid for the saved split tree so its bars can be dragged", () => {
    const state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "horizontal",
    });
    const equalGrid = tidyTerminalWorkspace(state, index => `tidy-arrange-${index}`);
    expect(equalGrid.arrangement).toBe("equal-grid");
    const splits = setTerminalWorkspaceArrangement(equalGrid, "custom-split");
    expect(splits.arrangement).toBe("custom-split");
    // Presentation only: the tree the grid was drawn from is handed back intact.
    expect(splits.root).toEqual(equalGrid.root);
    expect(splits.panes).toEqual(equalGrid.panes);
    expect(setTerminalWorkspaceArrangement(splits, "custom-split")).toEqual(splits);
    expect(setTerminalWorkspaceArrangement(EMPTY_TERMINAL_WORKSPACE_STATE, "custom-split"))
      .toEqual(EMPTY_TERMINAL_WORKSPACE_STATE);
  });

  it("moves a tab between panes and removes its now-empty source leaf", () => {
    let state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "horizontal",
    });
    state = moveTerminalTab(state, "terminal-a", "pane-b", 0);
    expect(state.root).toEqual({ kind: "pane", paneId: "pane-b" });
    expect(state.panes).toEqual([{
      id: "pane-b",
      tabs: [{ terminalId: "terminal-a" }, { terminalId: "terminal-b" }],
      activeTerminalId: "terminal-a",
    }]);
    expect(terminalPaneForTerminal(state, "terminal-a")?.id).toBe("pane-b");
  });

  it("reorders a dragged tab within its existing pane", () => {
    let state = openTerminalTab(opened(), "pane-a", "terminal-b");
    state = openTerminalTab(state, "pane-a", "terminal-c");
    state = moveTerminalTab(state, "terminal-a", "pane-a", 3);
    expect(state.panes[0]).toEqual({
      id: "pane-a",
      tabs: [
        { terminalId: "terminal-b" },
        { terminalId: "terminal-c" },
        { terminalId: "terminal-a" },
      ],
      activeTerminalId: "terminal-a",
    });
    expect(moveTerminalTab(state, "terminal-a", "pane-a", 3)).toEqual(state);
  });

  it("focuses and maximizes only known panes", () => {
    const state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "horizontal",
    });
    expect(terminalWorkspacePaneOrder(state)).toEqual(["pane-a", "pane-b"]);
    expect(focusTerminalPane(state, "pane-a").focusedPaneId).toBe("pane-a");
    const maximized = toggleTerminalPaneMaximized(state, "pane-a");
    expect(maximized.maximizedPaneId).toBe("pane-a");
    expect(toggleTerminalPaneMaximized(maximized, "pane-a").maximizedPaneId).toBeNull();
    expect(toggleTerminalPaneMaximized(state, "missing")).toEqual(state);
  });

  it("retains layout and pane identities when focus or active-tab selection is unchanged", () => {
    const split = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "horizontal",
    });
    const state = focusTerminalPane(split, "pane-a");
    expect(focusTerminalPane(state, "pane-a")).toBe(state);
    expect(focusTerminalTab(state, "terminal-a")).toBe(state);
    const other = focusTerminalTab(state, "terminal-b");
    expect(other.focusedPaneId).toBe("pane-b");
    expect(other.panes).toBe(state.panes);
    expect(other.root).toBe(state.root);
    const maximized = toggleTerminalPaneMaximized(state, "pane-a");
    expect(focusTerminalPane(maximized, "pane-a")).toBe(maximized);
    expect(focusTerminalTab(maximized, "terminal-a")).toBe(maximized);
    const switched = focusTerminalTab(maximized, "terminal-b");
    expect(switched.maximizedPaneId).toBe("pane-b");
    expect(switched.panes).toBe(maximized.panes);
  });

  it("cycles terminal focus across panes and background tabs", () => {
    let state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "vertical",
    });
    // pane-a holds two tabs, so pane flipping alone could never reach terminal-x.
    state = openTerminalTab(state, "pane-a", "terminal-x");
    state = focusTerminalTab(state, "terminal-a");
    const order = ["terminal-a", "terminal-x", "terminal-b"];
    for (let index = 0; index < order.length * 2; index += 1) {
      state = cycleTerminalFocus(state, "next");
      expect(terminalPaneById(state, state.focusedPaneId ?? "")?.activeTerminalId)
        .toBe(order[(index + 1) % order.length]);
    }
    // Back round the other way: wrapping from the first lands on the last.
    state = cycleTerminalFocus(state, "previous");
    expect(terminalPaneById(state, state.focusedPaneId ?? "")?.activeTerminalId)
      .toBe("terminal-b");
    state = cycleTerminalFocus(state, "previous");
    expect(terminalPaneById(state, state.focusedPaneId ?? "")?.activeTerminalId)
      .toBe("terminal-x");
    expect(cycleTerminalFocus(opened(), "next")).toEqual(opened());
  });

  it("flips pane focus in tree order and tidies panes into a balanced grid", () => {
    let state = splitTerminalPane(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
      terminalId: "terminal-b", orientation: "vertical",
    });
    state = splitTerminalPane(state, {
      paneId: "pane-b", newPaneId: "pane-c", splitId: "split-b",
      terminalId: "terminal-c", orientation: "vertical",
    });
    state = splitTerminalPane(state, {
      paneId: "pane-c", newPaneId: "pane-d", splitId: "split-c",
      terminalId: "terminal-d", orientation: "vertical",
    });
    const tidy = tidyTerminalWorkspace(state, index => `tidy-${index}`);
    expect(tidy.arrangement).toBe("equal-grid");
    expect(tidy.root).toEqual({
      kind: "split", id: "tidy-2", orientation: "vertical", ratio: 0.5,
      first: {
        kind: "split", id: "tidy-0", orientation: "horizontal", ratio: 0.5,
        first: { kind: "pane", paneId: "pane-a" },
        second: { kind: "pane", paneId: "pane-b" },
      },
      second: {
        kind: "split", id: "tidy-1", orientation: "horizontal", ratio: 0.5,
        first: { kind: "pane", paneId: "pane-c" },
        second: { kind: "pane", paneId: "pane-d" },
      },
    });
    expect(tidyTerminalWorkspace(state, () => "duplicate")).toEqual(state);
  });

  it("keeps a split grid pane in its drawn cell: top right stays top right", () => {
    // The grid draws four panes as a-b over c-d; b is top right.
    const grid = attachTerminalsBalanced(EMPTY_TERMINAL_WORKSPACE_STATE,
      ["terminal-a", "terminal-b", "terminal-c", "terminal-d"],
      index => `pane-${"abcd"[index]}`, index => `grid-${index}`);
    const next = splitTerminalPane(grid, { paneId: "pane-b", newPaneId: "pane-e",
      splitId: "split-e", terminalId: "terminal-e", orientation: "horizontal" });
    expect(next.root).toMatchObject({ orientation: "vertical",
      first: { orientation: "horizontal",
        first: { paneId: "pane-a" },
        second: { orientation: "horizontal",
          first: { paneId: "pane-b" }, second: { paneId: "pane-e" } } },
      second: { orientation: "horizontal",
        first: { paneId: "pane-c" }, second: { paneId: "pane-d" } } });
  });

  it.each(Array.from({ length: MAX_TERMINAL_PANES }, (_, index) => index + 1))(
    "attaches a confirmed %i-terminal batch as an explicit equal grid", count => {
      const terminalIds = Array.from({ length: count }, (_, index) => `terminal-${index + 1}`);
      const state = attachTerminalsBalanced(
        EMPTY_TERMINAL_WORKSPACE_STATE,
        terminalIds,
        index => `pane-${index + 1}`,
        index => `batch-split-${index + 1}`,
      );
      expect(MAX_TERMINAL_PANES).toBe(8);
      expect(state.arrangement).toBe("equal-grid");
      expect(state.panes).toHaveLength(count);
      expect(terminalWorkspacePaneOrder(state)).toEqual(
        Array.from({ length: count }, (_, index) => `pane-${index + 1}`),
      );
      expect(state.panes.map(pane => pane.tabs[0].terminalId)).toEqual(terminalIds);
      expect(state.focusedPaneId).toBe("pane-1");
      expect(parseTerminalWorkspaceState(serializeTerminalWorkspaceState(state))).toEqual(state);
    },
  );

  it("switches an automatic five-pane grid back to the manual split renderer on split", () => {
    const automatic = attachTerminalsBalanced(
      EMPTY_TERMINAL_WORKSPACE_STATE,
      Array.from({ length: 5 }, (_, index) => `terminal-${index + 1}`),
      index => `pane-${index + 1}`,
      index => `batch-split-${index + 1}`,
    );
    const manual = splitTerminalPane(automatic, {
      paneId: "pane-1", newPaneId: "pane-6", splitId: "manual-split",
      terminalId: "terminal-6", orientation: "horizontal",
    });
    expect(automatic.arrangement).toBe("equal-grid");
    expect(manual.arrangement).toBe("custom-split");
    expect(manual.panes).toHaveLength(6);
  });

  it("lets a manual split open a ninth pane and keeps it across a reload", () => {
    const six = attachTerminalsBalanced(
      EMPTY_TERMINAL_WORKSPACE_STATE,
      Array.from({ length: MAX_TERMINAL_PANES }, (_, index) => `terminal-${index}`),
      index => `pane-${index}`,
      index => `initial-split-${index}`,
    );
    const seven = splitTerminalPane(six, {
      paneId: "pane-7", newPaneId: "pane-8", splitId: "manual-split-8",
      terminalId: "terminal-8", orientation: "vertical",
    });
    expect(seven.panes).toHaveLength(MAX_TERMINAL_PANES + 1);
    expect(seven.arrangement).toBe("custom-split");
    expect(parseTerminalWorkspaceState(serializeTerminalWorkspaceState(seven))).toEqual(seven);
    expect(MAX_TERMINAL_PANES_HARD).toBeGreaterThan(MAX_TERMINAL_PANES);
  });

  it("keeps launches above the pane ceiling as tabs without duplicating known ids", () => {
    const initial = attachTerminalsBalanced(
      EMPTY_TERMINAL_WORKSPACE_STATE,
      Array.from({ length: MAX_TERMINAL_PANES }, (_, index) => `terminal-${index}`),
      index => `pane-${index}`,
      index => `initial-split-${index}`,
    );
    const next = attachTerminalsBalanced(
      initial,
      ["terminal-0", "terminal-extra-a", "terminal-extra-b"],
      index => `unused-pane-${index}`,
      index => `next-split-${index}`,
    );
    expect(next.panes).toHaveLength(MAX_TERMINAL_PANES);
    expect(next.panes[0].tabs.map(tab => tab.terminalId)).toEqual([
      "terminal-0", "terminal-extra-a", "terminal-extra-b",
    ]);
    expect(next.panes.flatMap(pane => pane.tabs)
      .filter(tab => tab.terminalId === "terminal-0")).toHaveLength(1);
  });
});

describe("splitTerminalPaneInPlace", () => {
  const ids = (prefix: string) => (index: number) => `${prefix}-${index}`;
  const split = (state: TerminalWorkspaceState, paneId: string, n: string,
      orientation: TerminalSplitOrientation = "horizontal") =>
    splitTerminalPaneInPlace(state, {
      paneId, newPaneId: `pane-${n}`, splitId: `split-${n}`, terminalId: `terminal-${n}`, orientation,
    }, ids(`tidy-${n}`));
  const row = (): TerminalWorkspaceState => {
    let state = opened();
    for (const [source, next] of [["a", "b"], ["b", "c"]]) {
      state = splitTerminalPane(state, { paneId: `pane-${source}`, newPaneId: `pane-${next}`,
        terminalId: `terminal-${next}`, splitId: `row-${next}`, orientation: "horizontal",
        ratio: source === "a" ? 1 / 3 : 0.5 });
    }
    return state;
  };

  it("halves only the pane it was opened from; no other pane moves", () => {
    const before = openTerminalTab(row(), "pane-a", "extra-tab");
    const next = split(before, "pane-b", "d", "vertical");
    expect(next.arrangement).toBe("custom-split");
    expect(next.root).toEqual({
      kind: "split", id: "row-b", orientation: "horizontal", ratio: 1 / 3,
      first: { kind: "pane", paneId: "pane-a" },
      second: {
        kind: "split", id: "row-c", orientation: "horizontal", ratio: 0.5,
        first: { kind: "split", id: "split-d", orientation: "vertical", ratio: 0.5,
          first: { kind: "pane", paneId: "pane-b" }, second: { kind: "pane", paneId: "pane-d" } },
        second: { kind: "pane", paneId: "pane-c" },
      },
    });
    expect(terminalWorkspacePaneOrder(next)).toEqual(["pane-a", "pane-b", "pane-d", "pane-c"]);
    expect(next.focusedPaneId).toBe("pane-d");
    before.panes.forEach((pane, index) => expect(next.panes[index]).toEqual(pane));
    expect(parseTerminalWorkspaceState(serializeTerminalWorkspaceState(next))).toEqual(next);
  });

  it("halves an equal-grid cell where the grid drew it", () => {
    const grid = tidyTerminalWorkspace(row(), ids("grid"));
    const next = split(grid, "pane-c", "d");
    // Three panes draw two on top and one across the bottom; the bottom one is halved.
    expect(next.root).toMatchObject({ orientation: "vertical",
      first: { orientation: "horizontal",
        first: { kind: "pane", paneId: "pane-a" }, second: { kind: "pane", paneId: "pane-b" } },
      second: { id: "split-d", orientation: "horizontal", ratio: 0.5,
        first: { kind: "pane", paneId: "pane-c" }, second: { kind: "pane", paneId: "pane-d" } } });
    expect(next.arrangement).toBe("custom-split");
    expect(next.maximizedPaneId).toBeNull();
  });

  it("gives the same layout when React runs the updater again", () => {
    let minted = 0;
    const tidy = onceTerminalIds(() => `tidy-${(minted++).toString()}`);
    const grid = tidyTerminalWorkspace(row(), ids("grid"));
    const input = { paneId: "pane-a", newPaneId: "pane-d", splitId: "split-d",
      terminalId: "terminal-d", orientation: "vertical" as const };
    expect(splitTerminalPaneInPlace(grid, input, tidy))
      .toEqual(splitTerminalPaneInPlace(grid, input, tidy));
    expect(minted).toBe(2);
  });

  it("leaves the layout alone for an unknown pane or a terminal already shown", () => {
    const three = row();
    expect(split(three, "pane-missing", "d")).toBe(three);
    expect(splitTerminalPaneInPlace(three, { paneId: "pane-a", newPaneId: "pane-d",
      splitId: "split-d", terminalId: "terminal-b", orientation: "horizontal" }, ids("x"))).toBe(three);
  });
});

describe("placePassedOffTerminal", () => {
  const splitById = (tree: TerminalSplitTree | null, id: string): TerminalSplitNode | null => {
    if (tree === null || tree.kind === "pane") return null;
    if (tree.id === id) return tree;
    return splitById(tree.first, id) ?? splitById(tree.second, id);
  };
  const base = attachTerminalsBalanced(
    EMPTY_TERMINAL_WORKSPACE_STATE,
    ["terminal-a", "terminal-b"],
    index => `pane-${index + 1}`,
    index => `split-${index + 1}`,
  );

  it("splits beside the source pane and focuses the new one, like the menu's pass off", () => {
    const beside = placePassedOffTerminal(base,
      { terminalId: "terminal-c", sourceTerminalId: "terminal-b" },
      () => "pane-c", () => "split-c");
    expect(terminalPaneForTerminal(beside, "terminal-c")?.id).toBe("pane-c");
    expect(beside.focusedPaneId).toBe("pane-c");
    expect(beside.arrangement).toBe("custom-split");
    const branch = splitById(beside.root, "split-c");
    expect(branch?.orientation).toBe("horizontal");
    expect(branch?.first).toEqual({ kind: "pane", paneId: "pane-2" });
    expect(branch?.second).toEqual({ kind: "pane", paneId: "pane-c" });
    // The source still has its own terminal; nothing moved.
    expect(terminalPaneForTerminal(beside, "terminal-b")?.id).toBe("pane-2");
    // Seating is idempotent: the next upsert of the same terminal changes nothing.
    expect(placePassedOffTerminal(beside,
      { terminalId: "terminal-c", sourceTerminalId: "terminal-b" },
      () => "pane-x", () => "split-x")).toBe(beside);
    // An equal-grid layout whose tree was skewed is tidied before splitting in place.
    const skewed = setTerminalWorkspaceArrangement(beside, "equal-grid");
    const tidied = placePassedOffTerminal(skewed,
      { terminalId: "terminal-d", sourceTerminalId: "terminal-b" },
      () => "pane-d", () => "split-d", index => `tidy-${index}`);
    expect(splitById(tidied.root, "tidy-0")).not.toBeNull();
    expect(splitById(tidied.root, "split-d")?.first).toEqual({ kind: "pane", paneId: "pane-2" });
  });

  it("falls back to a balanced attach when the source is not in this layout", () => {
    const orphan = placePassedOffTerminal(base,
      { terminalId: "terminal-d", sourceTerminalId: "terminal-gone" },
      index => `pane-d${index}`, index => `split-d${index}`);
    expect(terminalPaneForTerminal(orphan, "terminal-d")).toBeDefined();
    expect(orphan.arrangement).toBe("equal-grid");
    expect(orphan.panes).toHaveLength(3);
    const empty = placePassedOffTerminal(EMPTY_TERMINAL_WORKSPACE_STATE,
      { terminalId: "terminal-e", sourceTerminalId: "terminal-a" },
      () => "pane-e", () => "split-e");
    expect(terminalPaneForTerminal(empty, "terminal-e")?.id).toBe("pane-e");
    expect(empty.root).toEqual({ kind: "pane", paneId: "pane-e" });
  });
});

describe("Browser tabs", () => {
  const browser = newBrowserTabId(() => "page-1");
  const withBrowser = (): TerminalWorkspaceState => setBrowserTabUrl(splitTerminalPaneInPlace(opened(), {
    paneId: "pane-a", newPaneId: "pane-b", splitId: "split-b", terminalId: browser,
    orientation: "horizontal",
  }, index => `split-${index}`), browser, "http://localhost:5173/");

  it("is a pane of its own beside the source, and no terminal", () => {
    const state = withBrowser();
    expect(isBrowserTabId(browser)).toBe(true);
    expect(isBrowserTabId("terminal-a")).toBe(false);
    expect(terminalWorkspacePaneOrder(state)).toEqual(["pane-a", "pane-b"]);
    expect(state.focusedPaneId).toBe("pane-b");
    expect([...terminalIdsInWorkspaceState(state)]).toEqual(["terminal-a"]);
    expect([...browserTabIdsInWorkspaceState(state)]).toEqual([browser]);
  });

  it("keeps its page through a save, and survives the host roster reconciling", () => {
    const state = withBrowser();
    expect(browserTabUrl(state, browser)).toBe("http://localhost:5173/");
    const restored = parseTerminalWorkspaceState(serializeTerminalWorkspaceState(state));
    expect(restored).toEqual(state);
    expect(reconcileTerminalWorkspaceState(restored, ["terminal-a"])).toBe(restored);
    expect(terminalPaneById(reconcileTerminalWorkspaceState(restored, []), "pane-b")?.tabs)
      .toEqual([{ terminalId: browser, url: "http://localhost:5173/" }]);
  });

  it("moves with its page and remembers only a web address", () => {
    const state = openTerminalTab(withBrowser(), "pane-b", "terminal-c");
    const moved = moveTerminalTab(state, browser, "pane-b", 2);
    expect(terminalPaneById(moved, "pane-b")?.tabs.at(-1)).toEqual({
      terminalId: browser, url: "http://localhost:5173/",
    });
    expect(setBrowserTabUrl(state, browser, "http://localhost:5173/")).toBe(state);
    for (const refused of ["file:///etc/hosts", "javascript:alert(1)", "http://user:pw@example.com/"]) {
      expect(setBrowserTabUrl(state, browser, refused), refused).toBe(state);
    }
    expect(setBrowserTabUrl(state, "terminal-a", "https://example.com/")).toBe(state);
    const saved = JSON.parse(serializeTerminalWorkspaceState(state)) as {
      panes: { tabs: { terminalId: string; url?: string }[] }[];
    };
    saved.panes[0].tabs[0].url = "https://example.com/";
    saved.panes[1].tabs[0].url = "javascript:alert(1)";
    const parsed = parseTerminalWorkspaceState(JSON.stringify(saved));
    expect(parsed.panes[0].tabs[0]).toEqual({ terminalId: "terminal-a" });
    expect(parsed.panes[1].tabs[0]).toEqual({ terminalId: browser });
  });

  it("opens from a pane as a tab in that pane, shown and focused, with no pane added or moved", () => {
    const two = splitTerminalPaneInPlace(opened(), {
      paneId: "pane-a", newPaneId: "pane-b", splitId: "split-b", terminalId: "terminal-b",
      orientation: "horizontal",
    }, index => `split-${index}`);
    expect(two.focusedPaneId).toBe("pane-b");
    const state = openBrowserTabInPane(two, "pane-a", browser, "pane-new");
    expect(state.panes).toHaveLength(2);
    expect(state.root).toEqual(two.root);
    expect(terminalPaneById(state, "pane-a")).toEqual({
      id: "pane-a", tabs: [{ terminalId: "terminal-a" }, { terminalId: browser }], activeTerminalId: browser,
    });
    expect(terminalPaneById(state, "pane-b")).toEqual(terminalPaneById(two, "pane-b"));
    expect(state.focusedPaneId).toBe("pane-a");
    // The mixed pane reopens with its page, and its terminal ending leaves the page.
    const visited = setBrowserTabUrl(state, browser, "https://example.com/");
    expect(parseTerminalWorkspaceState(serializeTerminalWorkspaceState(visited))).toEqual(visited);
    expect(terminalPaneById(reconcileTerminalWorkspaceState(visited, ["terminal-b"]), "pane-a")).toEqual({
      id: "pane-a", tabs: [{ terminalId: browser, url: "https://example.com/" }], activeTerminalId: browser,
    });
    expect(terminalPaneById(closeTerminalTab(visited, browser), "pane-a")?.tabs)
      .toEqual([{ terminalId: "terminal-a" }]);
    // The palette has no pane: the focused one takes it.
    const fromPalette = openBrowserTabInPane(two, null, browser, "pane-new");
    expect(terminalPaneById(fromPalette, "pane-b")?.activeTerminalId).toBe(browser);
    expect(fromPalette.root).toEqual(two.root);
    expect(browserTabPaneId(two, "gone")).toBe("pane-b");
    // A maximized pane keeps the screen when its own menu adds the page.
    const maximized = toggleTerminalPaneMaximized(two, "pane-a");
    expect(openBrowserTabInPane(maximized, "pane-a", browser, "pane-new").maximizedPaneId).toBe("pane-a");
  });

  it("opens a link's new tab right after its page, in front or behind", () => {
    const pane = (state: TerminalWorkspaceState) => terminalPaneById(state, "pane-a");
    const start = openTerminalTab(openBrowserTabInPane(opened(), "pane-a", browser, "pane-new"), "pane-a", "terminal-z");
    expect(pane(start)?.tabs.map(tab => tab.terminalId)).toEqual(["terminal-a", browser, "terminal-z"]);
    const second = newBrowserTabId(() => "page-2");
    const front = openBrowserTabBeside(focusTerminalTab(start, browser), browser, second, "https://example.com/next", false);
    expect(pane(front)?.tabs).toEqual([{ terminalId: "terminal-a" }, { terminalId: browser },
      { terminalId: second, url: "https://example.com/next" }, { terminalId: "terminal-z" }]);
    expect(pane(front)?.activeTerminalId).toBe(second);
    expect(front.panes).toHaveLength(1);
    const behind = openBrowserTabBeside(focusTerminalTab(start, browser), browser, second, "https://example.com/next", true);
    expect(pane(behind)?.tabs[2]).toEqual({ terminalId: second, url: "https://example.com/next" });
    expect(pane(behind)?.activeTerminalId).toBe(browser);
    for (const refused of ["file:///etc/hosts", "javascript:alert(1)"]) {
      expect(openBrowserTabBeside(start, browser, second, refused, false)).toBe(start);
    }
    expect(openBrowserTabBeside(start, browser, "terminal-q", "https://example.com/", false)).toBe(start);
    expect(openBrowserTabBeside(start, "gone", second, "https://example.com/", false)).toBe(start);
    let full = start;
    while ((pane(full)?.tabs.length ?? 0) < MAX_TERMINAL_TABS_PER_PANE) {
      full = openTerminalTab(full, "pane-a", `terminal-${(pane(full)?.tabs.length ?? 0).toString()}`);
    }
    expect(openBrowserTabBeside(full, browser, second, "https://example.com/", false)).toBe(full);
  });

  it("opens the first pane when there is none, and leaves a full pane alone", () => {
    const first = openBrowserTabInPane(EMPTY_TERMINAL_WORKSPACE_STATE, null, browser, "pane-new");
    expect(first.panes).toEqual([{ id: "pane-new", tabs: [{ terminalId: browser }], activeTerminalId: browser }]);
    expect(first.focusedPaneId).toBe("pane-new");
    expect(browserTabPaneId(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a")).toBeNull();
    let full = opened();
    for (let index = 1; index < MAX_TERMINAL_TABS_PER_PANE; index++) {
      full = openTerminalTab(full, "pane-a", `terminal-${index.toString()}`);
    }
    expect(openBrowserTabInPane(full, "pane-a", browser, "pane-new")).toBe(full);
    expect(openBrowserTabInPane(opened(), "pane-a", "terminal-z", "pane-new")).toEqual(opened());
  });
});
