/** Renderer-owned layout state for integrated terminals.
 *
 * Process identity and lifecycle remain host-owned. This module only persists
 * which known terminal ids are shown as tabs, how panes are split, and which
 * pane has keyboard focus. A Browser page is a tab too: its id carries
 * `BROWSER_TAB_PREFIX`, never names a host terminal, and it keeps its last
 * address in the tab so a reopened workspace shows the same page.
 */

import { browserPageUrl } from "./browser-pane";

export const TERMINAL_WORKSPACE_VERSION = 1 as const;
/** Panes the automatic layout opens for freshly launched terminals; later
 *  launches become tabs. The user may split past this up to the hard cap. */
export const MAX_TERMINAL_PANES = 8;
/** ponytail: sanity bound for persisted layouts and manual splits, not a
 *  product limit; raise it if anyone actually reaches it. */
export const MAX_TERMINAL_PANES_HARD = 64;
export const MAX_TERMINAL_TABS_PER_PANE = 32;
/** Deep enough to nest a split under every pane the hard cap allows. */
export const MAX_TERMINAL_SPLIT_DEPTH = MAX_TERMINAL_PANES_HARD;
export const MIN_TERMINAL_SPLIT_RATIO = 0.2;
export const MAX_TERMINAL_SPLIT_RATIO = 0.8;
export const DEFAULT_TERMINAL_SPLIT_RATIO = 0.5;

export type TerminalId = string;
export type TerminalPaneId = string;
export type TerminalSplitId = string;
export type TerminalSplitOrientation = "horizontal" | "vertical";
export type TerminalWorkspaceArrangement = "equal-grid" | "custom-split";

export interface TerminalTabState {
  readonly terminalId: TerminalId;
  /** A Browser tab's last page. Absent on terminal tabs. */
  readonly url?: string;
}

/** Browser tabs live in the same panes as terminals. The host never sees these ids. */
export const BROWSER_TAB_PREFIX = "browser:";

export function isBrowserTabId(id: string): boolean {
  return id.startsWith(BROWSER_TAB_PREFIX);
}

export function newBrowserTabId(mintId: () => string): TerminalId {
  return `${BROWSER_TAB_PREFIX}${mintId()}`;
}

function tabState(terminalId: TerminalId, url: unknown): TerminalTabState {
  const page = isBrowserTabId(terminalId) && typeof url === "string" ? browserPageUrl(url) : null;
  return page === null ? { terminalId } : { terminalId, url: page };
}

export interface TerminalPaneState {
  readonly id: TerminalPaneId;
  readonly tabs: readonly TerminalTabState[];
  readonly activeTerminalId: TerminalId;
}

export interface TerminalPaneNode {
  readonly kind: "pane";
  readonly paneId: TerminalPaneId;
}

export interface TerminalSplitNode {
  readonly kind: "split";
  readonly id: TerminalSplitId;
  /** Horizontal places children left/right; vertical places them top/bottom. */
  readonly orientation: TerminalSplitOrientation;
  /** Fraction assigned to `first`, clamped away from unusably small panes. */
  readonly ratio: number;
  readonly first: TerminalSplitTree;
  readonly second: TerminalSplitTree;
}

export type TerminalSplitTree = TerminalPaneNode | TerminalSplitNode;

export interface TerminalWorkspaceState {
  readonly version: typeof TERMINAL_WORKSPACE_VERSION;
  /** Automatic launches/tidy use equal cells; direct user splits preserve their ratios. */
  readonly arrangement: TerminalWorkspaceArrangement;
  readonly panes: readonly TerminalPaneState[];
  readonly root: TerminalSplitTree | null;
  readonly focusedPaneId: TerminalPaneId | null;
  readonly maximizedPaneId: TerminalPaneId | null;
}

export const EMPTY_TERMINAL_WORKSPACE_STATE: TerminalWorkspaceState = Object.freeze({
  version: TERMINAL_WORKSPACE_VERSION,
  arrangement: "equal-grid",
  panes: Object.freeze([]),
  root: null,
  focusedPaneId: null,
  maximizedPaneId: null,
});

type UnknownRecord = Record<string, unknown>;

function record(value: unknown): UnknownRecord | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as UnknownRecord
    : null;
}

/** IDs cross a process boundary, so reject control characters without rewriting identity. */
export function isTerminalWorkspaceId(value: unknown): value is string {
  return typeof value === "string"
    && value.length > 0
    && value.length <= 256
    && value.trim().length > 0
    && !/[\u0000-\u001f\u007f]/u.test(value);
}

function splitRatio(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return DEFAULT_TERMINAL_SPLIT_RATIO;
  }
  return Math.min(MAX_TERMINAL_SPLIT_RATIO, Math.max(MIN_TERMINAL_SPLIT_RATIO, value));
}

function normalizePanes(value: unknown): TerminalPaneState[] {
  if (!Array.isArray(value)) return [];
  const paneIds = new Set<string>();
  const terminalIds = new Set<string>();
  const panes: TerminalPaneState[] = [];
  for (const candidate of value) {
    if (panes.length === MAX_TERMINAL_PANES_HARD) break;
    const source = record(candidate);
    if (source === null || !isTerminalWorkspaceId(source.id) || paneIds.has(source.id)) continue;
    if (!Array.isArray(source.tabs)) continue;
    const tabs: TerminalTabState[] = [];
    for (const tabCandidate of source.tabs) {
      if (tabs.length === MAX_TERMINAL_TABS_PER_PANE) break;
      const tab = record(tabCandidate);
      if (tab === null || !isTerminalWorkspaceId(tab.terminalId)
          || terminalIds.has(tab.terminalId)) continue;
      terminalIds.add(tab.terminalId);
      tabs.push(tabState(tab.terminalId, tab.url));
    }
    if (tabs.length === 0) continue;
    const activeTerminalId = isTerminalWorkspaceId(source.activeTerminalId)
        && tabs.some(tab => tab.terminalId === source.activeTerminalId)
      ? source.activeTerminalId
      : tabs[0].terminalId;
    paneIds.add(source.id);
    panes.push({ id: source.id, tabs, activeTerminalId });
  }
  return panes;
}

interface NormalizeTreeContext {
  readonly paneIds: ReadonlySet<string>;
  readonly usedPanes: Set<string>;
  readonly usedSplits: Set<string>;
}

function normalizeTree(value: unknown, context: NormalizeTreeContext,
    depth: number): TerminalSplitTree | null {
  if (depth > MAX_TERMINAL_SPLIT_DEPTH) return null;
  const source = record(value);
  if (source === null) return null;
  if (source.kind === "pane") {
    if (!isTerminalWorkspaceId(source.paneId)
        || !context.paneIds.has(source.paneId)
        || context.usedPanes.has(source.paneId)) return null;
    context.usedPanes.add(source.paneId);
    return { kind: "pane", paneId: source.paneId };
  }
  if (source.kind !== "split" || !isTerminalWorkspaceId(source.id)
      || context.usedSplits.has(source.id)) return null;
  context.usedSplits.add(source.id);
  const first = normalizeTree(source.first, context, depth + 1);
  const second = normalizeTree(source.second, context, depth + 1);
  if (first === null) return second;
  if (second === null) return first;
  return {
    kind: "split",
    id: source.id,
    orientation: source.orientation === "vertical" ? "vertical" : "horizontal",
    ratio: splitRatio(source.ratio),
    first,
    second,
  };
}

function paneOrder(tree: TerminalSplitTree | null, into: TerminalPaneId[] = []): TerminalPaneId[] {
  if (tree === null) return into;
  if (tree.kind === "pane") {
    into.push(tree.paneId);
    return into;
  }
  paneOrder(tree.first, into);
  paneOrder(tree.second, into);
  return into;
}

/**
 * Normalize untrusted persisted or in-memory layout without inventing process
 * state. This is the boundary for parsed JSON and host-supplied records only:
 * the mutators below take an already-typed `TerminalWorkspaceState` and re-run
 * it only where they assemble a fresh literal, because re-validating a typed
 * value on every drag frame costs a deep tree walk and mints a new identity.
 */
export function normalizeTerminalWorkspaceState(value: unknown): TerminalWorkspaceState {
  const source = record(value);
  if (source === null || source.version !== TERMINAL_WORKSPACE_VERSION) {
    return EMPTY_TERMINAL_WORKSPACE_STATE;
  }
  const candidatePanes = normalizePanes(source.panes);
  if (candidatePanes.length === 0) return EMPTY_TERMINAL_WORKSPACE_STATE;
  const paneIds = new Set(candidatePanes.map(pane => pane.id));
  const context: NormalizeTreeContext = {
    paneIds,
    usedPanes: new Set(),
    usedSplits: new Set(),
  };
  const root = normalizeTree(source.root, context, 0);
  if (root === null || context.usedPanes.size === 0) return EMPTY_TERMINAL_WORKSPACE_STATE;
  const panes = candidatePanes.filter(pane => context.usedPanes.has(pane.id));
  const order = paneOrder(root);
  const requestedFocus = isTerminalWorkspaceId(source.focusedPaneId)
      && context.usedPanes.has(source.focusedPaneId)
    ? source.focusedPaneId
    : order[0];
  const maximizedPaneId = isTerminalWorkspaceId(source.maximizedPaneId)
      && context.usedPanes.has(source.maximizedPaneId)
    ? source.maximizedPaneId
    : null;
  return {
    version: TERMINAL_WORKSPACE_VERSION,
    // `arrangement` was added compatibly to the renderer-only v1 payload. An
    // older saved tree may contain deliberate user ratios, so never guess that
    // it is safe to flatten unless the saved state says so explicitly.
    arrangement: source.arrangement === "equal-grid" ? "equal-grid" : "custom-split",
    panes,
    root,
    focusedPaneId: maximizedPaneId ?? requestedFocus,
    maximizedPaneId,
  };
}

export function parseTerminalWorkspaceState(raw: string | null | undefined): TerminalWorkspaceState {
  if (typeof raw !== "string" || raw.length === 0) return EMPTY_TERMINAL_WORKSPACE_STATE;
  try {
    return normalizeTerminalWorkspaceState(JSON.parse(raw) as unknown);
  } catch {
    return EMPTY_TERMINAL_WORKSPACE_STATE;
  }
}

export function serializeTerminalWorkspaceState(state: TerminalWorkspaceState): string {
  return JSON.stringify(state);
}

function pruneTree(tree: TerminalSplitTree | null,
    livePanes: ReadonlySet<TerminalPaneId>): TerminalSplitTree | null {
  if (tree === null) return null;
  if (tree.kind === "pane") return livePanes.has(tree.paneId) ? tree : null;
  const first = pruneTree(tree.first, livePanes);
  const second = pruneTree(tree.second, livePanes);
  if (first === null) return second;
  if (second === null) return first;
  return { ...tree, first, second };
}

function withPanes(state: TerminalWorkspaceState, panes: readonly TerminalPaneState[]): TerminalWorkspaceState {
  const livePanes = new Set(panes.map(pane => pane.id));
  return normalizeTerminalWorkspaceState({
    ...state,
    panes,
    root: pruneTree(state.root, livePanes),
  });
}

/** Drop layout tabs whose host-owned terminal processes no longer exist.
 *  Browser tabs have no process and are the layout's own to keep. */
export function reconcileTerminalWorkspaceState(state: TerminalWorkspaceState,
    knownTerminalIds: Iterable<TerminalId>): TerminalWorkspaceState {
  const current = state;
  const known = new Set<string>();
  for (const id of knownTerminalIds as Iterable<unknown>) {
    if (isTerminalWorkspaceId(id)) known.add(id);
  }
  const kept = (tab: TerminalTabState): boolean =>
    known.has(tab.terminalId) || isBrowserTabId(tab.terminalId);
  // Preserve the state object's identity when the host roster removes
  // nothing. The renderer's reconciliation effect observes layout changes;
  // allocating an equivalent state here would make that effect feed itself.
  if (current.panes.every(pane => pane.tabs.every(kept))) {
    return state;
  }
  const panes = current.panes.flatMap<TerminalPaneState>(pane => {
    const tabs = pane.tabs.filter(kept);
    if (tabs.length === 0) return [];
    return [{
      ...pane,
      tabs,
      activeTerminalId: tabs.some(tab => tab.terminalId === pane.activeTerminalId)
        ? pane.activeTerminalId : tabs[0].terminalId,
    }];
  });
  return withPanes(current, panes);
}

/**
 * Apply a host-roster reconciliation only to the exact workspace and layout
 * whose roster was captured. React may flush an older effect after the user
 * has moved to another workspace or attached a new terminal; letting that
 * stale updater run would prune the newer layout with the older roster.
 */
export function reconcileActiveTerminalWorkspaceState(state: TerminalWorkspaceState,
    knownTerminalIds: Iterable<TerminalId>, reconciliationWorkspaceId: string,
    activeWorkspaceId: string | null,
    reconciliationLayout: TerminalWorkspaceState): TerminalWorkspaceState {
  if (reconciliationWorkspaceId !== activeWorkspaceId || state !== reconciliationLayout) {
    return state;
  }
  return reconcileTerminalWorkspaceState(state, knownTerminalIds);
}

export function terminalPaneById(state: TerminalWorkspaceState,
    paneId: TerminalPaneId): TerminalPaneState | undefined {
  return state.panes.find(pane => pane.id === paneId);
}

export function terminalPaneForTerminal(state: Pick<TerminalWorkspaceState, "panes">,
    terminalId: TerminalId): TerminalPaneState | undefined {
  return state.panes.find(pane => pane.tabs.some(tab => tab.terminalId === terminalId));
}

/** Every terminal currently shown as a tab, including background tabs.
 *  Browser tabs are not terminals and are left out. */
export function terminalIdsInWorkspaceState(
    state: Pick<TerminalWorkspaceState, "panes">): Set<TerminalId> {
  const ids = new Set<TerminalId>();
  for (const pane of state.panes) {
    for (const tab of pane.tabs) if (!isBrowserTabId(tab.terminalId)) ids.add(tab.terminalId);
  }
  return ids;
}

/** Every Browser tab in the layout, including background tabs. */
export function browserTabIdsInWorkspaceState(
    state: Pick<TerminalWorkspaceState, "panes">): Set<TerminalId> {
  const ids = new Set<TerminalId>();
  for (const pane of state.panes) {
    for (const tab of pane.tabs) if (isBrowserTabId(tab.terminalId)) ids.add(tab.terminalId);
  }
  return ids;
}

/** What a saved layout string says, parsed once: see savedLayoutFacts. */
export interface SavedLayoutFacts {
  readonly state: TerminalWorkspaceState;
  /** terminalIdsInWorkspaceState, in pane and tab order. */
  readonly terminalIds: ReadonlySet<TerminalId>;
  readonly terminalIdList: readonly TerminalId[];
  readonly browserTabIds: ReadonlySet<TerminalId>;
}

/** Distinct saved layouts held at once; past it the memo starts over. */
export const SAVED_LAYOUT_FACTS_LIMIT = 256;
const SAVED_LAYOUT_FACTS = new Map<string, SavedLayoutFacts>();

/**
 * The one parse of a saved layout string. The roster, the hidden-terminal
 * sweep, the switcher, the Browser pages and the agent overview all read
 * saved layouts on every layout change (each splitter frame among them), so
 * each distinct string is parsed and walked once. The results are shared:
 * treat them as immutable.
 */
export function savedLayoutFacts(layout: string | null | undefined): SavedLayoutFacts {
  const key = layout ?? "";
  let facts = SAVED_LAYOUT_FACTS.get(key);
  if (facts === undefined) {
    const state = parseTerminalWorkspaceState(key);
    const terminalIds = terminalIdsInWorkspaceState(state);
    facts = { state, terminalIds, terminalIdList: [...terminalIds],
      browserTabIds: browserTabIdsInWorkspaceState(state) };
    if (SAVED_LAYOUT_FACTS.size >= SAVED_LAYOUT_FACTS_LIMIT) SAVED_LAYOUT_FACTS.clear();
    SAVED_LAYOUT_FACTS.set(key, facts);
  }
  return facts;
}

/** The saved address of one Browser tab, if the layout holds it. */
export function browserTabUrl(state: Pick<TerminalWorkspaceState, "panes">,
    tabId: TerminalId): string | undefined {
  for (const pane of state.panes) {
    const tab = pane.tabs.find(candidate => candidate.terminalId === tabId);
    if (tab !== undefined) return tab.url;
  }
  return undefined;
}

/** Remember the page a Browser tab shows. The state is unchanged when it already does. */
export function setBrowserTabUrl(state: TerminalWorkspaceState, tabId: TerminalId,
    url: string): TerminalWorkspaceState {
  const page = isBrowserTabId(tabId) ? browserPageUrl(url) : null;
  const owner = terminalPaneForTerminal(state, tabId);
  if (page === null || owner === undefined) return state;
  if (owner.tabs.some(tab => tab.terminalId === tabId && tab.url === page)) return state;
  return {
    ...state,
    panes: state.panes.map(pane => pane.id !== owner.id ? pane : {
      ...pane,
      tabs: pane.tabs.map(tab => tab.terminalId === tabId ? { terminalId: tabId, url: page } : tab),
    }),
  };
}

export function terminalWorkspacePaneOrder(state: TerminalWorkspaceState): readonly TerminalPaneId[] {
  return paneOrder(state.root);
}

/** Focus the next/previous terminal across every pane, tabs included, wrapping
 * at the ends. Pane flipping skips terminals parked in background tabs; this is
 * the keyboard route that reaches all of them.
 */
export function cycleTerminalFocus(state: TerminalWorkspaceState,
    direction: "next" | "previous"): TerminalWorkspaceState {
  const current = state;
  const order = paneOrder(current.root).flatMap(paneId =>
    terminalPaneById(current, paneId)?.tabs.map(tab => tab.terminalId) ?? []);
  if (order.length < 2) return current;
  const focused = current.focusedPaneId === null ? null
    : terminalPaneById(current, current.focusedPaneId)?.activeTerminalId ?? null;
  const at = focused === null ? -1 : order.indexOf(focused);
  const offset = direction === "previous" ? -1 : 1;
  return focusTerminalTab(current, order[(at + offset + order.length) % order.length]);
}

export function openTerminalTab(state: TerminalWorkspaceState, paneId: TerminalPaneId,
    terminalId: TerminalId): TerminalWorkspaceState {
  const current = state;
  if (!isTerminalWorkspaceId(paneId) || !isTerminalWorkspaceId(terminalId)) return current;
  const existing = terminalPaneForTerminal(current, terminalId);
  if (existing !== undefined) return focusTerminalTab(current, terminalId);
  const target = terminalPaneById(current, paneId);
  if (target === undefined) {
    if (current.root !== null) return current;
    return normalizeTerminalWorkspaceState({
      version: TERMINAL_WORKSPACE_VERSION,
      arrangement: "equal-grid",
      panes: [{ id: paneId, tabs: [{ terminalId }], activeTerminalId: terminalId }],
      root: { kind: "pane", paneId },
      focusedPaneId: paneId,
      maximizedPaneId: null,
    });
  }
  if (target.tabs.length === MAX_TERMINAL_TABS_PER_PANE) return current;
  return withPanes({ ...current, focusedPaneId: paneId }, current.panes.map(pane => pane.id === paneId
    ? { ...pane, tabs: [...pane.tabs, { terminalId }], activeTerminalId: terminalId }
    : pane));
}

/**
 * The pane a Browser opened from a pane's `+` menu or the palette joins: the
 * pane asked for, else the focused one, else the first. Null with no panes.
 */
export function browserTabPaneId(state: TerminalWorkspaceState,
    requested: TerminalPaneId | null): TerminalPaneId | null {
  if (requested !== null && terminalPaneById(state, requested) !== undefined) return requested;
  if (state.focusedPaneId !== null && terminalPaneById(state, state.focusedPaneId) !== undefined) {
    return state.focusedPaneId;
  }
  return paneOrder(state.root)[0] ?? null;
}

/**
 * A Browser opened from a pane (owner, 2026-11-15): a new tab in that pane,
 * shown and focused, and nothing else in the layout moves. With no panes at
 * all it is the first pane. A pane at the tab limit is left unchanged.
 */
export function openBrowserTabInPane(state: TerminalWorkspaceState, requested: TerminalPaneId | null,
    tabId: TerminalId, newPaneId: TerminalPaneId): TerminalWorkspaceState {
  if (!isBrowserTabId(tabId)) return state;
  const paneId = browserTabPaneId(state, requested);
  if (paneId === null) return state.root === null ? openTerminalTab(state, newPaneId, tabId) : state;
  if ((terminalPaneById(state, paneId)?.tabs.length ?? 0) >= MAX_TERMINAL_TABS_PER_PANE) return state;
  return openTerminalTab(focusTerminalPane(state, paneId), paneId, tabId);
}

/**
 * A link that wants a new tab, Chrome's way: a Browser tab right after the
 * page it came from, in the same pane. In front, it is shown and focused;
 * behind (⌘-click, middle-click, Open Link in New Tab), nothing else changes.
 * A pane at the tab limit, or an address that is not a web page, is left alone.
 */
export function openBrowserTabBeside(state: TerminalWorkspaceState, besideId: TerminalId,
    tabId: TerminalId, url: string, background: boolean): TerminalWorkspaceState {
  const page = browserPageUrl(url);
  const owner = terminalPaneForTerminal(state, besideId);
  if (page === null || owner === undefined || !isBrowserTabId(tabId) || !isTerminalWorkspaceId(tabId)
      || owner.tabs.length >= MAX_TERMINAL_TABS_PER_PANE
      || terminalPaneForTerminal(state, tabId) !== undefined) return state;
  const at = owner.tabs.findIndex(tab => tab.terminalId === besideId) + 1;
  const tabs = [...owner.tabs.slice(0, at), { terminalId: tabId, url: page }, ...owner.tabs.slice(at)];
  const next = withPanes(state, state.panes.map(pane => pane.id !== owner.id ? pane
    : { ...pane, tabs, activeTerminalId: background ? pane.activeTerminalId : tabId }));
  return background ? next : focusTerminalPane(next, owner.id);
}

export function focusTerminalPane(state: TerminalWorkspaceState,
    paneId: TerminalPaneId): TerminalWorkspaceState {
  const current = state;
  if (terminalPaneById(current, paneId) === undefined) return current;
  if (current.focusedPaneId === paneId
      && (current.maximizedPaneId === null || current.maximizedPaneId === paneId)) return current;
  return {
    ...current,
    focusedPaneId: paneId,
    maximizedPaneId: current.maximizedPaneId === null ? null : paneId,
  };
}

export function focusTerminalTab(state: TerminalWorkspaceState,
    terminalId: TerminalId): TerminalWorkspaceState {
  const current = state;
  const owner = terminalPaneForTerminal(current, terminalId);
  if (owner === undefined) return current;
  const focused = focusTerminalPane(current, owner.id);
  // Selecting the already-active tab only changes focus, if anything. Keep
  // pane identities so pointer/focus pairs do not rebuild the whole workspace.
  if (owner.activeTerminalId === terminalId) return focused;
  return withPanes(focused, current.panes.map(pane => pane.id === owner.id
    ? { ...pane, activeTerminalId: terminalId }
    : pane));
}

export function closeTerminalTab(state: TerminalWorkspaceState,
    terminalId: TerminalId): TerminalWorkspaceState {
  const current = state;
  const owner = terminalPaneForTerminal(current, terminalId);
  if (owner === undefined) return current;
  const at = owner.tabs.findIndex(tab => tab.terminalId === terminalId);
  const panes = current.panes.flatMap<TerminalPaneState>(pane => {
    if (pane.id !== owner.id) return [pane];
    const tabs = pane.tabs.filter(tab => tab.terminalId !== terminalId);
    if (tabs.length === 0) return [];
    return [{
      ...pane,
      tabs,
      activeTerminalId: pane.activeTerminalId === terminalId
        ? (tabs[at] ?? tabs[at - 1] ?? tabs[0]).terminalId
        : pane.activeTerminalId,
    }];
  });
  return withPanes(current, panes);
}

/**
 * Hand one terminal's exact tab slot to another, keeping the pane's position.
 *
 * `closeTerminalTab` followed by `openTerminalTab` is not the same thing: the
 * replacement would land at the end of whichever pane holds focus, and a pane
 * whose only tab was closed would collapse first. A pane that swaps its
 * process has to stay where the user left it.
 */
export function replaceTerminalTab(state: TerminalWorkspaceState, terminalId: TerminalId,
    replacementId: TerminalId): TerminalWorkspaceState {
  const current = state;
  if (!isTerminalWorkspaceId(replacementId)) return current;
  const owner = terminalPaneForTerminal(current, terminalId);
  if (owner === undefined) return current;
  // A terminal appears once. If the replacement is already on screen, the
  // outgoing tab still has to go.
  if (terminalPaneForTerminal(current, replacementId) !== undefined) {
    return closeTerminalTab(current, terminalId);
  }
  return withPanes(current, current.panes.map(pane => pane.id === owner.id ? {
    ...pane,
    tabs: pane.tabs.map(tab => tab.terminalId === terminalId ? { terminalId: replacementId } : tab),
    activeTerminalId: pane.activeTerminalId === terminalId
      ? replacementId : pane.activeTerminalId,
  } : pane));
}

function replacePaneLeaf(tree: TerminalSplitTree, paneId: TerminalPaneId,
    replacement: TerminalSplitTree): TerminalSplitTree {
  if (tree.kind === "pane") return tree.paneId === paneId ? replacement : tree;
  return {
    ...tree,
    first: replacePaneLeaf(tree.first, paneId, replacement),
    second: replacePaneLeaf(tree.second, paneId, replacement),
  };
}

function splitIds(tree: TerminalSplitTree | null, into = new Set<string>()): Set<string> {
  if (tree === null || tree.kind === "pane") return into;
  into.add(tree.id);
  splitIds(tree.first, into);
  splitIds(tree.second, into);
  return into;
}

export interface SplitTerminalPaneInput {
  readonly paneId: TerminalPaneId;
  readonly newPaneId: TerminalPaneId;
  readonly splitId: TerminalSplitId;
  readonly terminalId: TerminalId;
  readonly orientation: TerminalSplitOrientation;
  readonly placement?: "first" | "second";
  readonly ratio?: number;
}

/** Split an existing pane and mount one newly-created terminal in the new leaf. */
export function splitTerminalPane(state: TerminalWorkspaceState,
    input: SplitTerminalPaneInput): TerminalWorkspaceState {
  const current = state;
  if (current.root === null || current.panes.length >= MAX_TERMINAL_PANES_HARD
      || terminalPaneById(current, input.paneId) === undefined
      || terminalPaneById(current, input.newPaneId) !== undefined
      || terminalPaneForTerminal(current, input.terminalId) !== undefined
      || !isTerminalWorkspaceId(input.newPaneId)
      || !isTerminalWorkspaceId(input.splitId)
      || !isTerminalWorkspaceId(input.terminalId)
      || splitIds(current.root).has(input.splitId)) return current;
  const oldLeaf: TerminalPaneNode = { kind: "pane", paneId: input.paneId };
  const newLeaf: TerminalPaneNode = { kind: "pane", paneId: input.newPaneId };
  const newFirst = input.placement === "first";
  const branch: TerminalSplitNode = {
    kind: "split",
    id: input.splitId,
    orientation: input.orientation,
    ratio: splitRatio(input.ratio),
    first: newFirst ? newLeaf : oldLeaf,
    second: newFirst ? oldLeaf : newLeaf,
  };
  return normalizeTerminalWorkspaceState({
    ...current,
    arrangement: "custom-split",
    panes: [...current.panes, {
      id: input.newPaneId,
      tabs: [{ terminalId: input.terminalId }],
      activeTerminalId: input.terminalId,
    }],
    root: replacePaneLeaf(current.root, input.paneId, branch),
    focusedPaneId: input.newPaneId,
    maximizedPaneId: null,
  });
}

/** `"auto"` is the `+`: the new pane takes half of its source along the
 * longer side of the drawn pane; the caller resolves it before splitting. */
export type TerminalSplitRequest = TerminalSplitOrientation | "auto";

/**
 * The `+` and every directional split: the new pane takes half of its source,
 * and no other pane moves. A drawn equal grid first gets the tree that matches
 * its cells, so the source is halved where the grid showed it.
 */
export function splitTerminalPaneInPlace(state: TerminalWorkspaceState,
    input: SplitTerminalPaneInput, tidySplitId: TerminalSplitIdFactory): TerminalWorkspaceState {
  return splitTerminalPane(state.arrangement === "equal-grid"
    ? tidyTerminalWorkspace(state, tidySplitId) : state, input);
}

export interface PlacePassedOffTerminalInput {
  readonly terminalId: TerminalId;
  /** The pane the host says this terminal was passed off from. */
  readonly sourceTerminalId: TerminalId;
  readonly orientation?: TerminalSplitOrientation;
}

/**
 * Seat a pane the host created for a pass-off (an agent's `pass_off` tool,
 * not this renderer): a split beside the source's pane, or a balanced attach
 * when the source is not shown in this layout. Unchanged when the terminal
 * is already placed, so a later upsert of the same terminal is inert.
 */
export function placePassedOffTerminal(state: TerminalWorkspaceState,
    input: PlacePassedOffTerminalInput, paneId: TerminalPaneIdFactory,
    splitId: TerminalSplitIdFactory,
    tidySplitId: TerminalSplitIdFactory = index => `${splitId(0)}-tidy-${index.toString()}`):
    TerminalWorkspaceState {
  const current = state;
  if (terminalPaneForTerminal(current, input.terminalId) !== undefined) return current;
  const source = terminalPaneForTerminal(current, input.sourceTerminalId);
  if (source !== undefined) {
    const base = current.arrangement === "equal-grid"
      ? tidyTerminalWorkspace(current, tidySplitId) : current;
    const beside = splitTerminalPane(base, {
      paneId: source.id,
      newPaneId: paneId(0),
      splitId: splitId(0),
      terminalId: input.terminalId,
      orientation: input.orientation ?? "horizontal",
    });
    if (beside !== base) return beside;
  }
  return attachTerminalsBalanced(current, [input.terminalId], paneId, splitId);
}

export function setTerminalSplitRatio(state: TerminalWorkspaceState,
    splitId: TerminalSplitId, ratio: number): TerminalWorkspaceState {
  const current = state;
  const next = splitRatio(ratio);
  let found = false;
  // Only the path to the split is rebuilt; untouched branches keep identity.
  const update = (tree: TerminalSplitTree): TerminalSplitTree => {
    if (tree.kind === "pane") return tree;
    if (tree.id === splitId) {
      found = true;
      return tree.ratio === next ? tree : { ...tree, ratio: next };
    }
    const first = update(tree.first);
    const second = update(tree.second);
    return first === tree.first && second === tree.second ? tree : { ...tree, first, second };
  };
  if (current.root === null) return current;
  const root = update(current.root);
  if (!found) return current;
  // A drag held against the 0.2/0.8 bound asks for the same ratio every frame.
  if (root === current.root && current.arrangement === "custom-split") return current;
  return { ...current, arrangement: "custom-split", root };
}

export function moveTerminalTab(state: TerminalWorkspaceState, terminalId: TerminalId,
    targetPaneId: TerminalPaneId, targetIndex?: number): TerminalWorkspaceState {
  const current = state;
  const owner = terminalPaneForTerminal(current, terminalId);
  const target = terminalPaneById(current, targetPaneId);
  if (owner === undefined || target === undefined
      || owner.id !== target.id && target.tabs.length === MAX_TERMINAL_TABS_PER_PANE) return current;
  if (owner.id === target.id) {
    const sourceIndex = owner.tabs.findIndex(tab => tab.terminalId === terminalId);
    const requestedIndex = targetIndex === undefined || !Number.isInteger(targetIndex)
      ? owner.tabs.length
      : Math.min(owner.tabs.length, Math.max(0, targetIndex));
    const tabs = owner.tabs.filter(tab => tab.terminalId !== terminalId);
    const insertionIndex = Math.min(
      tabs.length,
      sourceIndex < requestedIndex ? requestedIndex - 1 : requestedIndex,
    );
    tabs.splice(insertionIndex, 0, owner.tabs[sourceIndex]);
    if (tabs.every((tab, index) => tab.terminalId === owner.tabs[index]?.terminalId)) return current;
    return withPanes({ ...current, focusedPaneId: owner.id }, current.panes.map(pane =>
      pane.id === owner.id ? { ...pane, tabs, activeTerminalId: terminalId } : pane));
  }
  const without = current.panes.flatMap<TerminalPaneState>(pane => {
    if (pane.id !== owner.id) return [pane];
    const tabs = pane.tabs.filter(tab => tab.terminalId !== terminalId);
    if (tabs.length === 0) return [];
    return [{
      ...pane,
      tabs,
      activeTerminalId: pane.activeTerminalId === terminalId
        ? tabs[0].terminalId : pane.activeTerminalId,
    }];
  });
  const moved = without.map(pane => {
    if (pane.id !== targetPaneId) return pane;
    const tabs = [...pane.tabs];
    const at = targetIndex === undefined || !Number.isInteger(targetIndex)
      ? tabs.length : Math.min(tabs.length, Math.max(0, targetIndex));
    tabs.splice(at, 0, owner.tabs.find(tab => tab.terminalId === terminalId) ?? { terminalId });
    return { ...pane, tabs, activeTerminalId: terminalId };
  });
  return withPanes({ ...current, focusedPaneId: targetPaneId, maximizedPaneId: null }, moved);
}

/** Exchange occupied slots, preserving pane contents and the split geometry. */
export function swapTerminalPanes(state: TerminalWorkspaceState,
    sourcePaneId: TerminalPaneId, targetPaneId: TerminalPaneId): TerminalWorkspaceState {
  if (state.root === null || sourcePaneId === targetPaneId
      || terminalPaneById(state, sourcePaneId) === undefined
      || terminalPaneById(state, targetPaneId) === undefined) return state;
  const order = terminalWorkspacePaneOrder(state);
  if (!order.includes(sourcePaneId) || !order.includes(targetPaneId)) return state;
  const swap = (tree: TerminalSplitTree): TerminalSplitTree => {
    if (tree.kind === "pane") {
      if (tree.paneId === sourcePaneId) return { ...tree, paneId: targetPaneId };
      if (tree.paneId === targetPaneId) return { ...tree, paneId: sourcePaneId };
      return tree;
    }
    const first = swap(tree.first);
    const second = swap(tree.second);
    return first === tree.first && second === tree.second
      ? tree : { ...tree, first, second };
  };
  return { ...state, root: swap(state.root) };
}

export function toggleTerminalPaneMaximized(state: TerminalWorkspaceState,
    paneId: TerminalPaneId): TerminalWorkspaceState {
  const current = state;
  if (terminalPaneById(current, paneId) === undefined) return current;
  return {
    ...current,
    focusedPaneId: paneId,
    maximizedPaneId: current.maximizedPaneId === paneId ? null : paneId,
  };
}

export type TerminalSplitIdFactory = (index: number) => TerminalSplitId;
export type TerminalPaneIdFactory = (index: number) => TerminalPaneId;

/**
 * Ids minted once per index. React re-runs a queued layout updater when it
 * rebases the queue, and an updater minting fresh ids would hand the new pane
 * a different id in the next commit, so it fades out and in again.
 */
export function onceTerminalIds(mint: () => string): (index: number) => string {
  const ids: string[] = [];
  return index => ids[index] ??= mint();
}

function balancedTerminalTree(paneIds: readonly TerminalPaneId[], depth: number,
    splitId: TerminalSplitIdFactory, index: { value: number },
    used: Set<string>): TerminalSplitTree | null {
  if (paneIds.length === 0) return null;
  if (paneIds.length === 1) return { kind: "pane", paneId: paneIds[0] };
  // The equal grid draws one row for two panes and two rows beyond that, the
  // top row taking the odd pane, each row left to right. The tree matches it,
  // so leaving the grid (a split, a drag) keeps every pane where it was drawn.
  const rows = depth === 0 && paneIds.length > 2;
  const at = Math.ceil(paneIds.length / 2);
  const first = balancedTerminalTree(paneIds.slice(0, at), depth + 1, splitId, index, used);
  const second = balancedTerminalTree(paneIds.slice(at), depth + 1, splitId, index, used);
  const id = splitId(index.value);
  index.value += 1;
  if (first === null || second === null || !isTerminalWorkspaceId(id) || used.has(id)) return null;
  used.add(id);
  return {
    kind: "split",
    id,
    // ponytail: matches the wide grid; narrow windows wrap 5+ panes into more
    // rows, which this two-row tree cannot follow.
    orientation: rows ? "vertical" : "horizontal",
    ratio: rows ? DEFAULT_TERMINAL_SPLIT_RATIO : splitRatio(at / paneIds.length),
    first,
    second,
  };
}

/** Rebuild stable pane order and select the automatic equal-cell presentation. */
export function tidyTerminalWorkspace(state: TerminalWorkspaceState,
    splitId: TerminalSplitIdFactory): TerminalWorkspaceState {
  const current = state;
  if (current.panes.length < 2) return current;
  const root = balancedTerminalTree(paneOrder(current.root), 0, splitId, { value: 0 }, new Set());
  if (root === null) return current;
  return { ...current, arrangement: "equal-grid", root, maximizedPaneId: null };
}

/**
 * Switch presentation without touching the tree. The equal grid draws its cells
 * directly and so has no splitter bars; only this returns the saved split tree,
 * whose bars can be dragged. Without it the grid is a one-way door: every
 * automatic launch and every Tidy selects it, and the only other way back out
 * is dragging a bar that the grid never rendered.
 */
export function setTerminalWorkspaceArrangement(state: TerminalWorkspaceState,
    arrangement: TerminalWorkspaceArrangement): TerminalWorkspaceState {
  const current = state;
  if (current.arrangement === arrangement || current.root === null) return current;
  return { ...current, arrangement };
}

/**
 * Attach one confirmed host batch as individual panes, then select the equal
 * grid presentation. The balanced tree remains the durable order and the
 * fallback if the user creates or resizes a manual split. Process creation is
 * deliberately outside this helper: callers
 * pass only terminal ids that the host's post-launch registry read confirmed.
 *
 * Once eight panes are visible, additional terminals remain reachable as tabs in
 * the focused pane. This preserves the user's requested process count without
 * letting a repeated launcher visit create an unreadably deep split tree.
 */
export function attachTerminalsBalanced(state: TerminalWorkspaceState,
    terminalIds: readonly TerminalId[], paneId: TerminalPaneIdFactory,
    splitId: TerminalSplitIdFactory): TerminalWorkspaceState {
  const current = state;
  const knownTerminalIds = new Set(current.panes.flatMap(pane =>
    pane.tabs.map(tab => tab.terminalId)));
  const requested: TerminalId[] = [];
  for (const id of terminalIds) {
    if (!isTerminalWorkspaceId(id) || knownTerminalIds.has(id)) continue;
    knownTerminalIds.add(id);
    requested.push(id);
  }
  if (requested.length === 0) return current;

  const panes = current.panes.map(existing => ({
    ...existing,
    tabs: [...existing.tabs],
  }));
  const usedPaneIds = new Set(panes.map(existing => existing.id));
  const newPaneIds: TerminalPaneId[] = [];
  const overflow: TerminalId[] = [];

  for (const [index, terminalId] of requested.entries()) {
    if (panes.length >= MAX_TERMINAL_PANES) {
      overflow.push(terminalId);
      continue;
    }
    const id = paneId(index);
    if (!isTerminalWorkspaceId(id) || usedPaneIds.has(id)) {
      overflow.push(terminalId);
      continue;
    }
    usedPaneIds.add(id);
    newPaneIds.push(id);
    panes.push({ id, tabs: [{ terminalId }], activeTerminalId: terminalId });
  }

  if (overflow.length > 0 && panes.length > 0) {
    const targetId = current.focusedPaneId !== null && usedPaneIds.has(current.focusedPaneId)
      ? current.focusedPaneId : panes[0].id;
    const targetIndex = panes.findIndex(candidate => candidate.id === targetId);
    const target = panes[targetIndex];
    const room = Math.max(0, MAX_TERMINAL_TABS_PER_PANE - target.tabs.length);
    const tabs = [...target.tabs, ...overflow.slice(0, room).map(terminalId => ({ terminalId }))];
    panes[targetIndex] = {
      ...target,
      tabs,
      activeTerminalId: tabs.at(-1)?.terminalId ?? target.activeTerminalId,
    };
  }

  const order = [...paneOrder(current.root), ...newPaneIds];
  if (order.length === 0) return current;
  const root = balancedTerminalTree(order, 0, splitId, { value: 0 }, new Set());
  if (root === null) return current;
  return normalizeTerminalWorkspaceState({
    version: TERMINAL_WORKSPACE_VERSION,
    arrangement: "equal-grid",
    panes,
    root,
    focusedPaneId: newPaneIds[0] ?? current.focusedPaneId ?? panes[0].id,
    maximizedPaneId: null,
  });
}
