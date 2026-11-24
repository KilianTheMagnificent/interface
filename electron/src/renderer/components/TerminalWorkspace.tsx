import type { TerminalPaneProps } from "./TerminalPane";
import { TerminalPaneMotion } from "./TerminalPaneMotion";
import { holdTerminalsDuring, TERMINAL_PANE_LAYOUT_EVENT, throttleTerminalFits } from "../terminal-pane-motion";
import { usePresence } from "../presence";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { TerminalLaunchProfile, TerminalLaunchProfileId } from "../../shared/ipc";
import {
  DEFAULT_TERMINAL_SPLIT_RATIO, MAX_TERMINAL_SPLIT_RATIO, MIN_TERMINAL_SPLIT_RATIO,
  terminalPaneById, terminalWorkspacePaneOrder,
  type TerminalId, type TerminalPaneId, type TerminalPaneState, type TerminalSplitNode,
  type TerminalSplitOrientation, type TerminalSplitRequest, type TerminalSplitTree,
  type TerminalWorkspaceState,
} from "../terminal-workspace";
import {
  browserTerminalSplitterDragEnvironment, TerminalSplitterDragController,
} from "../terminal-splitter-drag";
import {
  TerminalPane, type TerminalPaneLocate, type TerminalPaneRenderContext, type TerminalPaneTerminal,
} from "./TerminalPane";
import { AgentOverview, type AgentOverviewTab } from "./AgentOverview";
import type { AgentOverviewRow, AgentOverviewSince } from "../agent-overview";
import { TerminalLauncher, type TerminalLauncherNotice } from "./TerminalLauncher";
import { homeRelativePath, lastSegment } from "../display-paths";
import "../terminal.css";

export interface TerminalSplitWorkspaceProps {
  readonly state: TerminalWorkspaceState;
  readonly terminals: readonly TerminalPaneTerminal[];
  readonly onFocusPane: (paneId: TerminalPaneId) => void;
  readonly onSelectTab: (terminalId: TerminalId) => void;
  readonly onCloseTab: (terminalId: TerminalId) => void;
  readonly onMoveTerminal: (terminalId: TerminalId, targetPaneId: TerminalPaneId,
    targetIndex?: number) => void;
  readonly onRequestNewTab?: (paneId: TerminalPaneId) => void;
  readonly onRequestNewSession?: (paneId: TerminalPaneId) => void;
  readonly onOpenWorkspaceTool?: TerminalPaneProps["onOpenWorkspaceTool"];
  readonly onInspectTask?: TerminalPaneProps["onInspectTask"];
  readonly onContinueAsThread?: TerminalPaneProps["onContinueAsThread"];
  readonly onRequestSplit: (paneId: TerminalPaneId,
    orientation: TerminalSplitRequest, profileId?: TerminalLaunchProfileId) => void;
  /** Split off a new pane that picks up this one's work, transcript included.
      Absent where the surface is rendered without pass-off wiring. */
  readonly onRequestPassOff?: (paneId: TerminalPaneId,
    orientation: TerminalSplitOrientation, profileId: TerminalLaunchProfileId) => void;
  /** Empty the active terminal's screen and scrollback. Absent without wiring. */
  readonly onClearTerminal?: (terminalId: TerminalId) => void;
  /** "Send selection to thread…" (T3 section 9). Absent offers no item. */
  readonly onSendSelection?: (terminalId: TerminalId) => void;
  /** "Start again" on an ended agent pane. Absent without wiring. */
  readonly onRestartTerminal?: (terminalId: TerminalId) => void;
  readonly onToggleFastMode?: (terminalId: TerminalId) => void;
  readonly onSplitRatioChange: (splitId: string, ratio: number) => void;
  readonly onToggleMaximize: (paneId: TerminalPaneId) => void;
  /** Absent when the surface has no launcher: each pane's `+` splits a shell. */
  readonly launchProfiles?: readonly TerminalLaunchProfile[];
  readonly launchProfilesLoading?: boolean;
  readonly onLoadLaunchProfiles?: () => void;
  /** Cover every pane body with its title card; the terminals stay mounted. */
  readonly flipped?: boolean;
  /** The pane holding this terminal pulses once. */
  readonly locate?: TerminalPaneLocate | null;
  readonly renderTerminal: (context: TerminalPaneRenderContext) => ReactNode;
}

function clampRatio(ratio: number): number {
  return Math.min(MAX_TERMINAL_SPLIT_RATIO, Math.max(MIN_TERMINAL_SPLIT_RATIO, ratio));
}

function separatorKeyRatio(event: ReactKeyboardEvent<HTMLDivElement>,
    split: TerminalSplitNode): number | null {
  const backward = split.orientation === "horizontal" ? "ArrowLeft" : "ArrowUp";
  const forward = split.orientation === "horizontal" ? "ArrowRight" : "ArrowDown";
  if (event.key === backward) return clampRatio(split.ratio - 0.05);
  if (event.key === forward) return clampRatio(split.ratio + 0.05);
  if (event.key === "Home") return MIN_TERMINAL_SPLIT_RATIO;
  if (event.key === "End") return MAX_TERMINAL_SPLIT_RATIO;
  return null;
}

/**
 * The terminals one pane draws. The previous map is returned while its
 * members are the same objects, so a pane whose own terminals did not change
 * is skipped when another terminal's activity, title or dot does.
 */
export function terminalPaneMembers(
    previous: ReadonlyMap<TerminalId, TerminalPaneTerminal> | undefined,
    pane: Pick<TerminalPaneState, "tabs">,
    terminals: ReadonlyMap<TerminalId, TerminalPaneTerminal>): ReadonlyMap<TerminalId, TerminalPaneTerminal> {
  const next = new Map<TerminalId, TerminalPaneTerminal>();
  for (const tab of pane.tabs) {
    const terminal = terminals.get(tab.terminalId);
    if (terminal !== undefined) next.set(tab.terminalId, terminal);
  }
  if (previous !== undefined && previous.size === next.size
      && [...next].every(([id, terminal]) => previous.get(id) === terminal)) return previous;
  return next;
}

function TerminalPaneLeaf({ paneId, props, terminals }: {
  paneId: TerminalPaneId;
  props: TerminalSplitWorkspaceProps;
  terminals: ReadonlyMap<TerminalId, TerminalPaneTerminal>;
}) {
  const members = useRef<ReadonlyMap<TerminalId, TerminalPaneTerminal> | undefined>(undefined);
  const pane = terminalPaneById(props.state, paneId);
  if (pane === undefined) return null;
  members.current = terminalPaneMembers(members.current, pane, terminals);
  return (
    <TerminalPane pane={pane} terminals={members.current}
      focused={props.state.focusedPaneId === pane.id}
      maximized={props.state.maximizedPaneId === pane.id}
      onFocus={props.onFocusPane} onSelectTab={props.onSelectTab}
      onCloseTab={props.onCloseTab} onMoveTerminal={props.onMoveTerminal}
      onRequestNewTab={props.onRequestNewTab}
      onRequestNewSession={props.onRequestNewSession}
      onOpenWorkspaceTool={props.onOpenWorkspaceTool} onInspectTask={props.onInspectTask}
      onContinueAsThread={props.onContinueAsThread}
      onRequestSplit={props.onRequestSplit}
      onRequestPassOff={props.onRequestPassOff}
      onClearTerminal={props.onClearTerminal}
      onSendSelection={props.onSendSelection}
      onRestartTerminal={props.onRestartTerminal}
      onToggleFastMode={props.onToggleFastMode}
      onToggleMaximize={props.onToggleMaximize}
      launchProfiles={props.launchProfiles}
      launchProfilesLoading={props.launchProfilesLoading}
      onLoadLaunchProfiles={props.onLoadLaunchProfiles}
      flipped={props.flipped === true}
      locate={props.locate}
      renderTerminal={props.renderTerminal} />
  );
}

/** The tree owns positions; each portal keeps its pane and xterm instances alive. */
function TerminalPaneSlot({ mount }: { mount: HTMLDivElement }) {
  const slot = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const focused = document.activeElement;
    slot.current?.appendChild(mount);
    for (const host of mount.querySelectorAll(".terminal-xterm")) {
      host.dispatchEvent(new Event(TERMINAL_PANE_LAYOUT_EVENT));
    }
    if (focused instanceof HTMLElement && mount.contains(focused)) {
      focused.focus({ preventScroll: true });
    }
  }, [mount]);
  return <div className="terminal-pane-slot" ref={slot} />;
}

function TerminalTree({ tree, props, terminals, splitterDrag, mounts }: {
  tree: TerminalSplitTree;
  props: TerminalSplitWorkspaceProps;
  terminals: ReadonlyMap<TerminalId, TerminalPaneTerminal>;
  splitterDrag: TerminalSplitterDragController;
  mounts: ReadonlyMap<TerminalPaneId, HTMLDivElement>;
}) {
  if (tree.kind === "pane") {
    const mount = mounts.get(tree.paneId);
    if (mount !== undefined) return <TerminalPaneSlot mount={mount} />;
    return <TerminalPaneLeaf paneId={tree.paneId} props={props} terminals={terminals} />;
  }
  const axis = tree.orientation === "horizontal" ? "columns" : "rows";
  const grid = tree.orientation === "horizontal"
    ? { gridTemplateColumns: `${tree.ratio}fr auto ${1 - tree.ratio}fr` }
    : { gridTemplateRows: `${tree.ratio}fr auto ${1 - tree.ratio}fr` };
  return (
    <div className={`terminal-split is-${axis}`} data-terminal-split-id={tree.id} style={grid}>
      <TerminalTree tree={tree.first} props={props} terminals={terminals}
        splitterDrag={splitterDrag} mounts={mounts} />
      <div className="terminal-splitter" role="separator" tabIndex={0}
        aria-label={tree.orientation === "horizontal"
          ? "Resize terminal columns" : "Resize terminal rows"}
        aria-orientation={tree.orientation === "horizontal" ? "vertical" : "horizontal"}
        aria-valuemin={Math.round(MIN_TERMINAL_SPLIT_RATIO * 100)}
        aria-valuemax={Math.round(MAX_TERMINAL_SPLIT_RATIO * 100)}
        aria-valuenow={Math.round(tree.ratio * 100)}
        onKeyDown={event => {
          const ratio = separatorKeyRatio(event, tree);
          if (ratio === null) return;
          event.preventDefault();
          props.onSplitRatioChange(tree.id, ratio);
        }}
        onDoubleClick={event => {
          event.preventDefault();
          // Double-click restores the split this bar was created at: equal halves.
          props.onSplitRatioChange(tree.id, DEFAULT_TERMINAL_SPLIT_RATIO);
        }}
        onPointerDown={event => {
          if (event.button !== 0 || !event.isPrimary) return;
          event.preventDefault();
          if (event.detail >= 2) {
            return;
          }
          const splitter = event.currentTarget;
          const parent = splitter.parentElement;
          if (parent === null) return;
          const pointerId = event.pointerId;
          splitter.focus({ preventScroll: true });
          // Both sides' emulators refit on a trailing throttle while the bar
          // moves, then exactly once at the final ratio on release.
          let settleFits: (() => void) | null = null;
          if (splitterDrag.start({
            pointerId,
            clientX: event.clientX,
            clientY: event.clientY,
            orientation: tree.orientation,
            container: parent.getBoundingClientRect(),
            splitter: splitter.getBoundingClientRect(),
            onRatioChange: ratio => { props.onSplitRatioChange(tree.id, ratio); },
            onFinish: () => {
              splitter.classList.remove("is-dragging");
              if (splitter.hasPointerCapture(pointerId)) splitter.releasePointerCapture(pointerId);
              settleFits?.();
            },
          })) {
            splitter.classList.add("is-dragging");
            settleFits = throttleTerminalFits(parent);
            // Capture is an extra browser-level guarantee; the window capture
            // listeners remain the cross-canvas and lost-capture fallback.
            splitter.setPointerCapture(pointerId);
          }
        }}
        onLostPointerCapture={event => { splitterDrag.cancelPointer(event.pointerId); }} />
      <TerminalTree tree={tree.second} props={props} terminals={terminals}
        splitterDrag={splitterDrag} mounts={mounts} />
    </div>
  );
}

/** Pure split-tree renderer used by the full terminal center surface. */
export function TerminalSplitWorkspace(props: TerminalSplitWorkspaceProps) {
  const splitterDrag = useMemo(() => new TerminalSplitterDragController(
    browserTerminalSplitterDragEnvironment(),
  ), []);
  useEffect(() => () => { splitterDrag.dispose(); }, [splitterDrag]);
  const terminals = useMemo(() => new Map(props.terminals.map(terminal =>
    [terminal.id, terminal])), [props.terminals]);
  const previousMounts = useRef(new Map<TerminalPaneId, HTMLDivElement>());
  const mounts = useMemo(() => {
    const next = new Map<TerminalPaneId, HTMLDivElement>();
    if (typeof document === "undefined") return next;
    for (const pane of props.state.panes) {
      const mount = previousMounts.current.get(pane.id) ?? document.createElement("div");
      mount.className = "terminal-pane-mount";
      next.set(pane.id, mount);
    }
    return next;
  }, [props.state.panes]);
  useLayoutEffect(() => {
    // React removes a retired portal's contents, but its manually attached
    // container still fills the slot. Remove those containers after the slots
    // have moved surviving mounts, or workspace switches push new panes offscreen.
    for (const [paneId, mount] of previousMounts.current) {
      if (mounts.get(paneId) !== mount) mount.remove();
    }
    previousMounts.current = mounts;
  }, [mounts]);
  // The centre draws this surface only once a pane exists; an empty
  // workspace shows the launcher instead.
  if (props.state.root === null || props.state.panes.length === 0) return null;
  // Maximizing only adds a class: the layout keeps every pane mounted so the
  // other terminals keep the xterm buffers that unmounting would dispose.
  const maximized = props.state.maximizedPaneId !== null
    && terminalPaneById(props.state, props.state.maximizedPaneId) !== undefined;
  const equalGrid = props.state.arrangement === "equal-grid";
  const paneIds = equalGrid ? terminalWorkspacePaneOrder(props.state) : [];
  const workspaceClass = `terminal-workspace${
    equalGrid ? ` is-equal-grid terminal-count-${paneIds.length}` : ""}${
    maximized ? " has-maximized" : ""}`;
  return (
    <TerminalPaneMotion layout={props.state} className={workspaceClass}>
      {equalGrid
        ? paneIds.map(paneId => {
            const mount = mounts.get(paneId);
            return mount ? <TerminalPaneSlot key={paneId} mount={mount} />
              : <TerminalPaneLeaf key={paneId} paneId={paneId} props={props} terminals={terminals} />;
          })
        : <TerminalTree tree={props.state.root} props={props} terminals={terminals}
            splitterDrag={splitterDrag} mounts={mounts} />}
      {[...mounts].map(([paneId, mount]) => createPortal(
        <TerminalPaneLeaf paneId={paneId} props={props} terminals={terminals} />,
        mount, paneId,
      ))}
    </TerminalPaneMotion>
  );
}

/**
 * The launcher is covering the surface: asked for, or forced by a workspace
 * with no panes. The centre reads this too — a review dialog cancelled over a
 * still-open launcher must not clear the folder that launcher is showing.
 */
export function terminalLauncherShowing(open: boolean,
    state: Pick<TerminalWorkspaceState, "root" | "panes">): boolean {
  return open || state.root === null || state.panes.length === 0;
}

export type TerminalWorkspaceView = "split" | "cards";

export interface TerminalWorkspaceCallbacks {
  readonly onFocusTerminal: (terminalId: TerminalId) => void;
  /** Remove a terminal from the layout while leaving its process alive. */
  readonly onDetachTerminal: (terminalId: TerminalId) => void;
  /** Move layout presentation only; this must not spawn, stop, or reparent a process. */
  readonly onMoveTerminal: (terminalId: TerminalId, targetPaneId: TerminalPaneId,
    targetIndex?: number) => void;
  readonly onFocusPane: (paneId: TerminalPaneId) => void;
  readonly onRequestNewTab?: (paneId: TerminalPaneId) => void;
  readonly onRequestNewSession?: (paneId: TerminalPaneId) => void;
  readonly onOpenWorkspaceTool?: TerminalPaneProps["onOpenWorkspaceTool"];
  readonly onInspectTask?: TerminalPaneProps["onInspectTask"];
  /** Continue as thread (review C1): end this pane and bring its session into Threads. */
  readonly onContinueAsThread?: TerminalPaneProps["onContinueAsThread"];
  readonly onRequestSplit: (paneId: TerminalPaneId,
    orientation: TerminalSplitRequest, profileId?: TerminalLaunchProfileId) => void;
  /** Split off a new pane that picks up this one's work, transcript included. */
  readonly onRequestPassOff: (paneId: TerminalPaneId,
    orientation: TerminalSplitOrientation, profileId: TerminalLaunchProfileId) => void;
  /** Empty one terminal's screen and scrollback. Presentation only. */
  readonly onClearTerminal: (terminalId: TerminalId) => void;
  /** "Send selection to thread…" (T3 section 9). Absent offers no item. */
  readonly onSendSelection?: (terminalId: TerminalId) => void;
  /** "Start again" on an ended agent pane: the host restarts that terminal. */
  readonly onRestartTerminal?: (terminalId: TerminalId) => void;
  readonly onToggleFastMode?: (terminalId: TerminalId) => void;
  readonly onSplitRatioChange: (splitId: string, ratio: number) => void;
  readonly onToggleMaximize: (paneId: TerminalPaneId) => void;
  readonly onTidy: () => void;
}

/** The saved-workspace switcher. Absent while the roster is still loading. */
export interface TerminalWorkspaceSwitcher {
  readonly records: readonly {
    readonly id: string;
    readonly name: string;
    /** Title of this workspace's first saved terminal, including while it is hidden. */
    readonly firstTerminalTitle?: string;
    /** Tabs currently attached to this workspace, excluding parked processes. */
    readonly openTerminalCount: number;
    /** Those tabs' terminals, in pane order: the sidebar draws one pip each. */
    readonly openTerminalIds: readonly string[];
    readonly terminalIds: readonly string[];
    readonly cwd?: string;
  }[];
  readonly activeId: string | null;
  readonly onSelect: (workspaceId: string) => void;
  /** A project-menu launch seeds this workspace's working folder. */
  readonly onCreate: (projectPath?: string) => void;
  readonly onRename: (workspaceId: string) => void;
  /** Opens the confirmed delete for this exact workspace, even if it is not active. */
  readonly onDelete: (workspaceId: string) => void;
  /** Prevents workspace ownership changing while a terminal batch is in flight. */
  readonly busy: boolean;
  readonly canCreate: boolean;
}

export interface TerminalWorkspaceProps {
  /** `present` stays true while the panel slides out after a close. */
  readonly outputPreview?: { open: boolean; present?: boolean; canOpen: boolean; onToggle: () => void; panel: ReactNode };
  /**
   * Receives the element Browser pages live in. It sits after the panes, so
   * each page can be anchored over its tab's slot, and is never moved.
   */
  readonly browserLayer?: (element: HTMLDivElement | null) => void;
  readonly state: TerminalWorkspaceState;
  readonly terminals: readonly TerminalPaneTerminal[];
  readonly activeCwd: string | null;
  readonly callbacks: TerminalWorkspaceCallbacks;
  readonly renderTerminal: (context: TerminalPaneRenderContext) => ReactNode;
  readonly view?: TerminalWorkspaceView;
  readonly defaultView?: TerminalWorkspaceView;
  readonly onViewChange?: (view: TerminalWorkspaceView) => void;
  /**
   * Where this surface's controls are drawn: an element in the window toolbar,
   * owned by App. The controls and their state stay here and are portaled in,
   * so the tiles start directly under the title row with no bar of their own.
   * Absent (or null, before the toolbar mounts) draws no controls at all — the
   * server renderer has no portals, which is also why tests render
   * TerminalToolbarActions directly.
   */
  readonly toolbarSlot?: HTMLElement | null;
  /** Publishes the layout section of the window's ··· menu; null on unmount. */
  readonly onMenuChange?: (menu: TerminalMenuModel | null) => void;
  /** The agent overview column beside the panes, toggled from the ··· menu. */
  readonly overview?: {
    readonly open: boolean;
    readonly rows: readonly AgentOverviewRow[];
    readonly profileLabel: (profileId: string) => string;
    /** Each agent's last screen lines, for the live cards. */
    readonly tails?: ReadonlyMap<string, readonly string[]>;
    /** When each agent entered its status. */
    readonly since?: ReadonlyMap<string, AgentOverviewSince>;
    readonly onCloseIdle?: (terminalIds: readonly string[]) => void;
    readonly onOpenTasks?: () => void;
    /** The column's Browser tab body; absent, no Browser tab. */
    readonly browser?: ReactNode;
    readonly tab?: AgentOverviewTab;
    readonly onTabChange?: (tab: AgentOverviewTab) => void;
    readonly onOpenChange: (open: boolean) => void;
    readonly onLocate: (row: AgentOverviewRow) => void;
  };
  /** The pane holding this terminal pulses once. */
  readonly locate?: TerminalPaneLocate | null;
  readonly launcher?: {
    readonly open: boolean;
    /** The folder the launcher shows. The centre owns it, so the review dialog
        beside it cannot name a different one. */
    readonly cwd: string | null;
    /** False while the surface is hidden: the launcher waits before taking focus. */
    readonly visible?: boolean;
    readonly profiles: readonly TerminalLaunchProfile[];
    readonly profilesLoading: boolean;
    readonly launching: boolean;
    readonly notice: TerminalLauncherNotice | null;
    readonly onOpen: () => void;
    readonly onClose: () => void;
    readonly onLaunch: (
      profileId: TerminalLaunchProfileId,
      count: number,
      cwd: string,
    ) => void;
    /** Profile and count the last launch used, remembered for the session. */
    readonly defaultProfileId?: TerminalLaunchProfileId | null;
    readonly defaultCount?: number;
    /** Open the adversarial-review dialog. Absent until review is connected. */
    readonly onOpenReview?: () => void;
    readonly onBrowse: (preferredPath?: string) => Promise<string | null>;
    readonly onCwdChange: (cwd: string) => void;
    readonly onRetryProfiles: () => void;
    /** A pane menu's re-read: no loading state while a list is already drawn.
        Absent, the menus use onRetryProfiles. */
    readonly onRefreshProfiles?: () => void;
    /** Each provider's plan headroom on its tile (review E). */
    readonly headroom?: Readonly<Record<string, { readonly text: string; readonly title: string }>>;
  };
}

/**
 * What the window's ··· menu needs to draw the terminal layout section
 * (owner, 2026-09-26: the four-square Layout button and the Agent overview
 * toggle moved into that menu). TerminalWorkspace publishes it; the actions
 * are stable and always act on the surface's current state.
 */
export interface TerminalMenuModel {
  /** The folder the section heading names; null draws no heading. */
  readonly cwd: string | null;
  readonly view: TerminalWorkspaceView;
  readonly equalGrid: boolean;
  /** A launch in flight locks the layout, the way it locks the panes' controls. */
  readonly disabled: boolean;
  /** Tidy needs two panes to arrange. */
  readonly canTidy: boolean;
  readonly onFlip: () => void;
  readonly onTidy: () => void;
  /** Absent when this surface has no agent overview column. */
  readonly overview?: { readonly open: boolean; readonly onToggle: () => void };
}

/**
 * The terminal layout section of the ··· menu — the folder, Agent overview,
 * the two layout toggles, then the window destinations. `choose` closes the
 * menu before the action runs. A destination App has not wired is not drawn.
 */
export function TerminalLayoutItems({ menu, choose, onOpenTasks, onOpenSavedSessions }: {
  readonly menu: TerminalMenuModel;
  readonly choose: (action: () => void) => void;
  readonly onOpenTasks?: () => void;
  readonly onOpenSavedSessions?: () => void;
}) {
  const destination = (label: string, icon: string, action?: () => void) => action && (
    <button type="button" className="terminal-menu-launch" role="menuitem"
      onClick={() => { choose(action); }}>
      <span className={`codicon codicon-${icon}`} aria-hidden="true" /><span>{label}</span>
    </button>
  );
  const check = (on: boolean) => on
    && <span className="codicon codicon-check terminal-menu-check" aria-hidden="true" />;
  return <>
    {menu.cwd !== null && menu.cwd !== "" && <div className="terminal-pane-menu-heading" role="presentation">
      <strong>{lastSegment(menu.cwd) || menu.cwd}</strong><span title={menu.cwd}>{homeRelativePath(menu.cwd)}</span>
    </div>}
    {menu.overview !== undefined && <button type="button" className="terminal-menu-launch"
      role="menuitemcheckbox" aria-checked={menu.overview.open}
      onClick={() => { choose(menu.overview!.onToggle); }}>
      <span className="codicon codicon-layout-sidebar-right" aria-hidden="true" /><span>Live panes</span>
      {check(menu.overview.open)}
    </button>}
    {/* Tidy is two-way: the equal grid has no splitter bars, so leaving it
        is the only way back to a draggable layout. */}
    <button type="button" className="terminal-menu-launch" role="menuitemcheckbox"
      aria-checked={menu.equalGrid} disabled={!menu.canTidy || menu.disabled}
      onClick={() => { choose(menu.onTidy); }}>
      <span className="codicon codicon-layout-grid" aria-hidden="true" /><span>Tidy panes</span>
      {check(menu.equalGrid)}
    </button>
    <button type="button" className="terminal-menu-launch" role="menuitemcheckbox"
      aria-checked={menu.view === "cards"} disabled={menu.disabled}
      onClick={() => { choose(menu.onFlip); }}>
      <span className="codicon codicon-arrow-swap" aria-hidden="true" /><span>Flip terminals</span>
      {check(menu.view === "cards")}
    </button>
    <div className="terminal-pane-menu-divider" role="separator" />
    {destination("Tasks", "checklist", onOpenTasks)}
    {/* Placeholder until Codex account switching exists (owner, 2026-09-14):
        the row is drawn and does nothing. */}
    <button type="button" className="terminal-menu-launch" role="menuitem"
      aria-disabled="true" title="Account switching is not yet available.">
      <span className="codicon codicon-account" aria-hidden="true" /><span>Codex accounts…</span>
    </button>
    {destination("Saved sessions…", "history", onOpenSavedSessions)}
  </>;
}

/** Dedicated center surface: the split workspace, with its layout menu in the toolbar. */
export function TerminalWorkspace({ state, terminals, activeCwd, callbacks, renderTerminal,
    view: controlledView, defaultView = "split", onViewChange, toolbarSlot,
    onMenuChange, overview, locate, outputPreview, browserLayer,
    launcher }: TerminalWorkspaceProps) {
  const [localView, setLocalView] = useState<TerminalWorkspaceView>(defaultView);
  const view = controlledView ?? localView;
  const equalGrid = state.arrangement === "equal-grid";
  const setView = (next: TerminalWorkspaceView): void => {
    if (controlledView === undefined) setLocalView(next);
    onViewChange?.(next);
  };
  const showLauncher = launcher !== undefined && terminalLauncherShowing(launcher.open, state);
  const showWorkspaceView = (next: TerminalWorkspaceView): void => {
    if (launcher?.launching === true) return;
    launcher?.onClose();
    setView(next);
  };
  // The ··· menu's layout section. Its actions go through a ref so the model
  // App holds stays valid across renders; only the drawn state republishes it.
  const menuActions = useRef({ flip: () => {}, tidy: () => {}, overview: () => {} });
  menuActions.current = {
    flip: () => { showWorkspaceView(showLauncher || view !== "cards" ? "cards" : "split"); },
    tidy: callbacks.onTidy,
    overview: () => { overview?.onOpenChange(!overview.open); },
  };
  const [stableMenuActions] = useState(() => ({
    onFlip: () => { menuActions.current.flip(); },
    onTidy: () => { menuActions.current.tidy(); },
    onToggleOverview: () => { menuActions.current.overview(); },
  }));
  const hasOverview = overview !== undefined;
  const overviewOpen = overview?.open === true;
  const launching = launcher?.launching === true;
  const canTidy = state.panes.length >= 2;
  useEffect(() => {
    onMenuChange?.({
      cwd: activeCwd, view, equalGrid, disabled: launching, canTidy,
      onFlip: stableMenuActions.onFlip, onTidy: stableMenuActions.onTidy,
      ...(hasOverview
        ? { overview: { open: overviewOpen, onToggle: stableMenuActions.onToggleOverview } } : {}),
    });
  }, [activeCwd, canTidy, equalGrid, hasOverview, launching, onMenuChange, overviewOpen,
    stableMenuActions, view]);
  useEffect(() => () => { onMenuChange?.(null); }, [onMenuChange]);
  // The overview and the output preview slide in and out beside the panes.
  // The panes hold their terminal grids while either moves and refit once it
  // settles, as they do for the left sidebar: a fit per frame blanks canvases
  // and resizes every PTY. Opening one closes the other, so the longer
  // arrival is the one to wait for.
  const overviewPresence = usePresence(overview?.open === true);
  const centerRef = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    const center = centerRef.current;
    const slides = [...center?.querySelectorAll(
      ":scope > .agent-overview, :scope > .output-preview") ?? []]
      .flatMap(panel => panel.getAnimations().filter(animation =>
        /^(agent-overview|output-preview)-/u.test((animation as CSSAnimation).animationName ?? "")));
    const slide = slides.find(animation => /-in$/u.test((animation as CSSAnimation).animationName)) ?? slides[0];
    return center === null || slide === undefined ? undefined
      : holdTerminalsDuring(center, slide) ?? undefined;
  }, [overviewPresence.phase, outputPreview?.open]);
  return (
    <section ref={centerRef}
      className={`${overview !== undefined && overviewPresence.present ? "terminal-center has-agent-overview" : "terminal-center"}${outputPreview?.present ?? outputPreview?.open ? " has-output-preview" : ""}`}
      aria-label="Integrated terminals">
      {/* No workspace bar here by design: saved workspaces are a destination in
          the sidebar, and the switcher this surface builds is published to it
          through onSwitcherChange rather than drawn twice. */}
      {/* No bar of its own: the controls ride the window toolbar, so the tiles
          start directly under the title row. State stays here; only the
          drawing moves. */}
      {toolbarSlot !== undefined && toolbarSlot !== null && outputPreview !== undefined
        && createPortal(<button type="button" className="terminal-center-action"
          aria-label="Output preview" title="Output preview" aria-pressed={outputPreview.open}
          disabled={!outputPreview.canOpen} onClick={outputPreview.onToggle}>
          <span className="codicon codicon-open-preview" aria-hidden="true" />
        </button>, toolbarSlot)}
      <div className="terminal-center-body">
        {launcher !== undefined && showLauncher
          && <TerminalLauncher profiles={launcher.profiles}
              profilesLoading={launcher.profilesLoading} cwd={launcher.cwd}
              launching={launcher.launching} notice={launcher.notice}
              onLaunch={launcher.onLaunch}
              onOpenReview={launcher.onOpenReview}
              defaultProfileId={launcher.defaultProfileId}
              defaultCount={launcher.defaultCount}
              visible={launcher.visible}
              onBrowse={launcher.onBrowse}
              onCwdChange={launcher.onCwdChange}
              onRetryProfiles={launcher.onRetryProfiles}
              headroom={launcher.headroom}
              /* No exit on an EMPTY workspace, and none is possible: onClose
                 only lowers `launcher.open`, and showLauncher is forced true
                 by `panes.length === 0`, so a button here would be inert. The
                 code view bar directly above it is that case's visible exit. */
              onCancel={state.panes.length > 0 ? launcher.onClose : undefined} />}
        {/* Covered rather than unmounted: unmounting disposes every xterm, which
            loses each terminal's buffer and re-fits its pty to a stale size. The
            panes stay laid out so a window resize behind the overlay still fits. */}
        <div key="terminal-split" className="terminal-center-split"
          inert={showLauncher || undefined}>
          {state.panes.length > 0 && <TerminalSplitWorkspace state={state} terminals={terminals}
            onFocusPane={callbacks.onFocusPane}
            onSelectTab={callbacks.onFocusTerminal}
            onCloseTab={callbacks.onDetachTerminal}
            onMoveTerminal={callbacks.onMoveTerminal}
            onRequestNewTab={callbacks.onRequestNewTab}
            onRequestNewSession={callbacks.onRequestNewSession}
            onOpenWorkspaceTool={callbacks.onOpenWorkspaceTool} onInspectTask={callbacks.onInspectTask}
            onContinueAsThread={callbacks.onContinueAsThread}
            onRequestSplit={callbacks.onRequestSplit}
            onRequestPassOff={callbacks.onRequestPassOff}
            onClearTerminal={callbacks.onClearTerminal}
            onSendSelection={callbacks.onSendSelection}
            onRestartTerminal={callbacks.onRestartTerminal}
            onToggleFastMode={callbacks.onToggleFastMode}
            onSplitRatioChange={callbacks.onSplitRatioChange}
            onToggleMaximize={callbacks.onToggleMaximize}
            launchProfiles={launcher?.profiles}
            launchProfilesLoading={launcher?.profilesLoading}
            onLoadLaunchProfiles={launcher?.onRefreshProfiles ?? launcher?.onRetryProfiles}
            flipped={view === "cards"}
            locate={locate}
            renderTerminal={renderTerminal} />}
          {browserLayer !== undefined && <div ref={browserLayer} className="browser-page-layer" />}
        </div>
      </div>
      {outputPreview?.panel}
      {overview !== undefined && overviewPresence.present && <AgentOverview rows={overview.rows}
        profileLabel={overview.profileLabel} onLocate={overview.onLocate}
        tails={overview.tails} since={overview.since} onCloseIdle={overview.onCloseIdle}
        onOpenTasks={overview.onOpenTasks}
        browser={overview.browser} tab={overview.tab} onTabChange={overview.onTabChange}
        phase={overviewPresence.phase} onAnimationEnd={overviewPresence.onAnimationEnd}
        onClose={() => { overview.onOpenChange(false); }} />}
    </section>
  );
}
