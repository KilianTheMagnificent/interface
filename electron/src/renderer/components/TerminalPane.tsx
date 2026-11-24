import { createPortal } from "react-dom";
import {
  memo, useEffect, useId, useMemo, useRef, useState, type DragEvent as ReactDragEvent,
  type ReactNode,
} from "react";
import type {
  TerminalFastModeV1, TerminalLaunchProfile, TerminalLaunchProfileId, TerminalStartupDownloadV1,
} from "../../shared/ipc";
import { TerminalFastModeToggle } from "./TerminalFastModeToggle";
import { TerminalStartupDownloadNotice } from "./TerminalStartupDownloadNotice";
import { homeRelativePath } from "../display-paths";
import type { TerminalLiveActivity } from "../terminal-activity";
import { beginTerminalPaneDrag, endTerminalPaneDrag, terminalPaneDragSource } from "../terminal-pane-drag-preview";
import type {
  TerminalId, TerminalPaneState, TerminalSplitOrientation, TerminalSplitRequest,
} from "../terminal-workspace";
import { BROWSER_PROFILE_ID, TerminalProfileIcon, useTerminalMenuPopover } from "./TerminalMenu";
import type { TerminalMenuStatus, TerminalMenuTerminal } from "./TerminalMenu";
import { TerminalLaunchProfileList, TerminalPaneNewMenu } from "./TerminalPaneNewMenu";
import { DONE_WORD, statusWord } from "./status-words";
import { continueAsThreadRefusal } from "../thread-terminal-bridge";
import { sendTerminalViewCommand, terminalViewHasSelection } from "../terminal-view-attachment";
import "../terminal.css";

export const TERMINAL_TAB_DRAG_MIME = "application/x-interface-terminal-tab";

/** Resolve whether a tab drop lands before or after the tab under the pointer. */
export function terminalTabDropIndex(tabIndex: number, clientX: number,
    tabLeft: number, tabWidth: number): number {
  const safeIndex = Number.isInteger(tabIndex) && tabIndex >= 0 ? tabIndex : 0;
  if (!Number.isFinite(clientX) || !Number.isFinite(tabLeft)
      || !Number.isFinite(tabWidth) || tabWidth <= 0) return safeIndex;
  return clientX < tabLeft + tabWidth / 2 ? safeIndex : safeIndex + 1;
}

function hasTerminalTabData(dataTransfer: DataTransfer): boolean {
  return Array.from(dataTransfer.types)
    .some(type => type.toLowerCase() === TERMINAL_TAB_DRAG_MIME);
}

function draggedTerminalId(dataTransfer: DataTransfer): TerminalId | null {
  const value = dataTransfer.getData(TERMINAL_TAB_DRAG_MIME).trim();
  return value === "" ? null : value;
}

export interface TerminalPaneTerminal extends TerminalMenuTerminal {
  readonly id: TerminalId;
  readonly status: TerminalMenuStatus;
  readonly activity?: TerminalLiveActivity;
  /** Finished a turn nobody has looked at yet: the blue dot. */
  readonly done?: boolean;
  readonly generation?: number;
  readonly fastMode?: TerminalFastModeV1;
  readonly startupDownload?: TerminalStartupDownloadV1;
  /** An ended agent's "Start again" notice text (terminalRestartOffer); null offers none. */
  readonly restartOffer?: string | null;
  /** A Browser tab's page (its profile is BROWSER_PROFILE_ID). */
  readonly url?: string;
  /** Last non-blank buffer lines, newest last. Only read while a card covers
      the pane, so it is absent whenever the terminal itself is on screen. */
  readonly preview?: readonly string[];
}

export interface TerminalPaneRenderContext {
  readonly terminalId: TerminalId;
  readonly active: boolean;
  readonly paneId: string;
}

export type WorkspaceTool = "files" | "browser" | "ios-simulator";

export interface TerminalPaneProps {
  readonly pane: TerminalPaneState;
  readonly terminals: ReadonlyMap<TerminalId, TerminalPaneTerminal>;
  readonly focused: boolean;
  readonly maximized: boolean;
  readonly onFocus: (paneId: string) => void;
  readonly onSelectTab: (terminalId: TerminalId) => void;
  /** Detaches the tab from this layout. It does not imply process termination. */
  readonly onCloseTab: (terminalId: TerminalId) => void;
  /** Presentation-only move request; process ownership remains with the caller. */
  readonly onMoveTerminal: (terminalId: TerminalId, targetPaneId: string,
    targetIndex?: number) => void;
  readonly onRequestNewTab?: (paneId: string) => void;
  readonly onRequestNewSession?: (paneId: string) => void;
  /** `paneId` is the pane whose menu asked: a Browser opens as a tab in it. */
  readonly onOpenWorkspaceTool?: (tool: WorkspaceTool, cwd: string, paneId: string) => void;
  readonly onInspectTask?: (terminalId: TerminalId) => void;
  /** Continue as thread (review C1): end this pane and bring its session into Threads. */
  readonly onContinueAsThread?: (terminalId: TerminalId) => void;
  /** `"auto"` is the `+`: the new pane takes half of this one, along its longer side. */
  readonly onRequestSplit: (paneId: string, orientation: TerminalSplitRequest,
    profileId?: TerminalLaunchProfileId) => void;
  /** Absent where nothing can carry the handover: the ··· menu omits the page. */
  readonly onRequestPassOff?: (paneId: string, orientation: TerminalSplitOrientation,
    profileId: TerminalLaunchProfileId) => void;
  /** Empty the active terminal's screen and scrollback. Absent without wiring. */
  readonly onClearTerminal?: (terminalId: TerminalId) => void;
  /** "Send selection to thread…" (T3 section 9). Absent offers no item. */
  readonly onSendSelection?: (terminalId: TerminalId) => void;
  /** "Start again" on an ended agent pane. Absent without wiring: no button. */
  readonly onRestartTerminal?: (terminalId: TerminalId) => void;
  readonly onToggleFastMode?: (terminalId: TerminalId) => void;
  readonly onToggleMaximize: (paneId: string) => void;
  /** Absent when the pane is rendered without launcher wiring: `+` splits a shell. */
  readonly launchProfiles?: readonly TerminalLaunchProfile[];
  readonly launchProfilesLoading?: boolean;
  readonly onLoadLaunchProfiles?: () => void;
  /** Cover the body with this pane's title card; the terminals stay mounted. */
  readonly flipped?: boolean;
  /** The pane holding this terminal pulses once; a new nonce pulses again. */
  readonly locate?: TerminalPaneLocate | null;
  readonly renderTerminal: (context: TerminalPaneRenderContext) => ReactNode;
}

/** Pane pulse length. The CSS ring fades inside it; the class then clears. */
export const TERMINAL_PANE_LOCATE_MS = 1000;

/** A request, from the agent overview, to show where a terminal is. */
export interface TerminalPaneLocate {
  readonly terminalId: TerminalId;
  readonly nonce: number;
}

/** A new pane lands beside a wide pane and below a tall one. */
export function terminalPaneSplitOrientation(width: number,
    height: number): TerminalSplitOrientation {
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
    return "horizontal";
  }
  return width >= height ? "horizontal" : "vertical";
}

interface TerminalStatusSource {
  readonly status: TerminalMenuStatus;
  readonly activity?: TerminalLiveActivity;
  /** Finished a turn nobody has looked at yet: the blue dot. */
  readonly done?: boolean;
}

/**
 * The pane's live state, host status and renderer-derived activity folded into
 * ONE vocabulary. The glyph class, working predicate and status words are
 * all read off this, so the three of them cannot drift apart.
 */
type TerminalLiveKind =
  "starting" | "running" | "working" | "done" | "waiting" | "exited" | "failed";

function terminalLiveKind(terminal: TerminalStatusSource | undefined): TerminalLiveKind {
  const status = terminal?.status ?? "starting";
  if (status === "failed") return "failed";
  if (status === "exited") return "exited";
  if (terminalNeedsAttention(terminal)) return "waiting";
  if (status === "starting") return "starting";
  if (terminal?.activity === "working") return "working";
  if (status === "running" && terminal?.done === true) return "done";
  return status;
}

/** The status glyph in the vocabulary the rest of the window already speaks:
 *  the working hue while busy, nothing when the shell is alive but idle or
 *  has exited, blue when an agent finished a turn nobody has looked at, amber
 *  when it needs the human, red for a failure. */
export function terminalStatusClass(terminal: TerminalStatusSource | undefined,
    base: "terminal-tab-status" | "terminal-card-status"): string {
  return `${base} is-${terminalLiveKind(terminal)}`;
}

/** The process asked for the human, and nothing has failed. */
export function terminalNeedsAttention(terminal: TerminalStatusSource | undefined): boolean {
  const status = terminal?.status;
  if (status === "failed" || status === "exited") return false;
  return terminal?.activity === "waiting";
}

/** The same state as words: colour alone never reports state. */
export function terminalStatusText(terminal: TerminalStatusSource | undefined): string {
  switch (terminalLiveKind(terminal)) {
    case "failed": return "Failed";
    case "exited": return "Exited";
    case "waiting": return statusWord("waiting");
    case "starting": return "Starting";
    case "working": return statusWord("working");
    case "done": return DONE_WORD;
    case "running": return statusWord("idle");
  }
}

export type TerminalPaneMoreMenuPage = "actions" | "pass-off";

export interface TerminalPaneMoreMenuProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onSplit: (orientation: TerminalSplitOrientation) => void;
  /** Absent when the pane has no launcher wiring: the menu stays splits-only. */
  readonly profiles?: readonly TerminalLaunchProfile[];
  readonly profilesLoading?: boolean;
  readonly onLoadProfiles?: () => void;
  readonly onPassOff?: (profileId: TerminalLaunchProfileId) => void;
  /** False while this pane has no live process whose work could be handed on. */
  readonly passOffEnabled?: boolean;
  /** Empty the active terminal's screen and scrollback. Absent without wiring. */
  readonly onClear?: () => void;
  readonly onInspectTask?: () => void;
  /** Put the active terminal's selection in a thread's draft (T3 section 9). */
  readonly onSendSelection?: () => void;
  /** Copy the active terminal's selection (⌘C still works in the terminal).
      Absent offers no item; `copyEnabled` false greys it out. */
  readonly onCopy?: () => void;
  readonly copyEnabled?: boolean;
  /** Paste the clipboard into the active terminal (⌘V still works). */
  readonly onPaste?: () => void;
  /** Continue as thread (review C1); absent offers no item. */
  readonly onContinueAsThread?: () => void;
  /** Why Continue as thread does not apply now; null when it does. */
  readonly continueRefusal?: string | null;
  /** Test-only escape hatch; static rendering never gets to click through. */
  readonly initialPage?: TerminalPaneMoreMenuPage;
}

/** The pane header's `···`: splits, pass-off and clear; new panes live on the `+`. */
export function TerminalPaneMoreMenu({ open, onOpenChange, onSplit, profiles, profilesLoading,
    onLoadProfiles, onPassOff, passOffEnabled = false, onClear, onInspectTask,
    onContinueAsThread, continueRefusal = null, onSendSelection, onCopy, copyEnabled = false, onPaste,
    initialPage = "actions" }: TerminalPaneMoreMenuProps) {
  const [page, setPage] = useState<TerminalPaneMoreMenuPage>(initialPage);
  // Roving tabindex: one tab stop for the whole menu, so Tab leaves it and the
  // arrow keys move inside it.
  const [activeIndex, setActiveIndex] = useState(0);
  const offersPassOff = profiles !== undefined && onPassOff !== undefined;
  const menuPlacement = useMemo(() => ({ page, profiles }), [page, profiles]);
  // Copy and Paste lead the menu (owner, 2026-11-15: they left the strip
  // that floated over the terminal's text); the splits follow them.
  const pasteIndex = Number(onCopy !== undefined);
  const splitIndex = pasteIndex + Number(onPaste !== undefined);
  const passOffIndex = splitIndex + 2;
  const threadIndex = passOffIndex + Number(offersPassOff);
  const clearIndex = threadIndex + Number(onContinueAsThread !== undefined);
  const sendIndex = clearIndex + Number(onInspectTask !== undefined) + Number(onClear !== undefined);
  const { triggerRef, menuRef, box, close, onMenuKey } = useTerminalMenuPopover({
    // Re-place on the page turn, and again when the pass-off page's own rows
    // arrive: `onLoadProfiles` resolves after the menu is already open.
    open, onOpenChange, minWidth: 200, placeOn: menuPlacement,
  });
  // A closed menu forgets the detour: reopening the ··· starts at the actions.
  useEffect(() => { if (!open) setPage(initialPage); }, [open, initialPage]);
  // Each page is its own roving order, so its first item takes the tab stop.
  useEffect(() => { setActiveIndex(0); }, [page]);

  const passOff = open && offersPassOff && page === "pass-off" ? (
    <div ref={menuRef} className="terminal-menu-panel is-compact is-floating" role="menu"
      aria-label="Pass off to" onKeyDown={onMenuKey}
      style={box === null
        ? { visibility: "hidden", top: 0, left: 0 }
        : { top: box.top, left: box.left, width: box.width, maxHeight: box.maxHeight }}>
      <button type="button" className="terminal-menu-launch" role="menuitem"
        aria-label="Back to pane actions" title="Back"
        tabIndex={activeIndex === 0 ? 0 : -1}
        onFocus={() => { setActiveIndex(0); }}
        onClick={() => { setPage("actions"); }}>
        <span className="codicon codicon-chevron-left" aria-hidden="true" />
        <span>Back</span>
      </button>
      {/* A shell cannot be handed a transcript to pick up, so it is not a
          destination here even though the + menu offers it. Neither is Muse
          Code: its TUI takes no initial prompt, so the host refuses it. Kimi
          Code takes none either, but the host types the brief into it. */}
      <TerminalLaunchProfileList loading={profilesLoading === true}
        profiles={(profiles ?? []).filter(profile => profile.id !== "shell" && profile.id !== "muse")}
        activeIndex={activeIndex - 1}
        onFocusIndex={index => { setActiveIndex(index + 1); }}
        onChoose={profileId => {
          onPassOff?.(profileId);
          close(true);
        }} />
    </div>
  ) : null;

  const panel = open && passOff === null ? (
    <div ref={menuRef} className="terminal-menu-panel is-compact is-floating" role="menu"
      aria-label="Pane actions" onKeyDown={onMenuKey}
      style={box === null
        ? { visibility: "hidden", top: 0, left: 0 }
        : { top: box.top, left: box.left, width: box.width, maxHeight: box.maxHeight }}>
      {onCopy !== undefined && (
        <button type="button" className="terminal-menu-launch" role="menuitem"
          aria-label="Copy selection" aria-keyshortcuts="Meta+C"
          disabled={!copyEnabled}
          title={copyEnabled ? "Copy (⌘C)" : "Select text in the terminal to copy it."}
          tabIndex={activeIndex === 0 ? 0 : -1}
          onFocus={() => { setActiveIndex(0); }}
          onClick={() => {
            onCopy();
            close(true);
          }}>
          <span className="codicon codicon-copy" aria-hidden="true" />
          <span>Copy</span>
        </button>
      )}
      {onPaste !== undefined && (
        <button type="button" className="terminal-menu-launch" role="menuitem"
          aria-label="Paste into terminal" aria-keyshortcuts="Meta+V" title="Paste (⌘V)"
          tabIndex={activeIndex === pasteIndex ? 0 : -1}
          onFocus={() => { setActiveIndex(pasteIndex); }}
          onClick={() => {
            onPaste();
            close(true);
          }}>
          <span className="codicon codicon-file" aria-hidden="true" />
          <span>Paste</span>
        </button>
      )}
      <button type="button" className="terminal-menu-launch" role="menuitem"
        aria-label="Split terminal right" title="Split right (⌘D)"
        aria-keyshortcuts="Meta+D"
        tabIndex={activeIndex === splitIndex ? 0 : -1}
        onFocus={() => { setActiveIndex(splitIndex); }}
        onClick={() => {
          onSplit("horizontal");
          close(true);
        }}>
        <span className="codicon codicon-split-horizontal" aria-hidden="true" />
        <span>Split right</span>
      </button>
      <button type="button" className="terminal-menu-launch" role="menuitem"
        aria-label="Split terminal down" title="Split down (⌘⇧D)"
        aria-keyshortcuts="Meta+Shift+D"
        tabIndex={activeIndex === splitIndex + 1 ? 0 : -1}
        onFocus={() => { setActiveIndex(splitIndex + 1); }}
        onClick={() => {
          onSplit("vertical");
          close(true);
        }}>
        <span className="codicon codicon-split-vertical" aria-hidden="true" />
        <span>Split down</span>
      </button>
      {offersPassOff && (
        <button type="button" className="terminal-menu-launch" role="menuitem"
          aria-label="Pass off to another agent" aria-haspopup="menu"
          disabled={!passOffEnabled}
          tabIndex={activeIndex === passOffIndex ? 0 : -1}
          onFocus={() => { setActiveIndex(passOffIndex); }}
          title={passOffEnabled
            ? "Hand this pane's work to a new agent"
            : "Pass off needs a running source terminal."}
          onClick={() => {
            onLoadProfiles?.();
            setPage("pass-off");
          }}>
          <span className="codicon codicon-send" aria-hidden="true" />
          <span>Pass off to…</span>
        </button>
      )}
      {onContinueAsThread !== undefined && (
        <button type="button" className="terminal-menu-launch" role="menuitem"
          disabled={continueRefusal !== null}
          tabIndex={activeIndex === threadIndex ? 0 : -1}
          onFocus={() => { setActiveIndex(threadIndex); }}
          title={continueRefusal
            ?? "End this pane and continue its session in Threads. Only one side drives a session at a time."}
          onClick={() => { close(false); onContinueAsThread(); }}>
          <span className="codicon codicon-comment-discussion" aria-hidden="true" />
          <span>Continue as thread</span>
        </button>
      )}
      {onInspectTask && <button type="button" className="terminal-menu-launch" role="menuitem"
        tabIndex={activeIndex === clearIndex ? 0 : -1}
        onFocus={() => setActiveIndex(clearIndex)}
        onClick={() => { close(false); onInspectTask(); }}>
        <span className="codicon codicon-search" aria-hidden="true" /><span>Inspect task result</span>
      </button>}
      {/* Clearing a terminal cannot be undone and also drops the replay tail,
          so it is not a trash can in a strip that appears under the pointer. */}
      {onClear !== undefined && (
        <button type="button" className="terminal-menu-launch" role="menuitem"
          aria-label="Clear terminal scrollback"
          tabIndex={activeIndex === clearIndex + Number(onInspectTask !== undefined) ? 0 : -1}
          onFocus={() => { setActiveIndex(clearIndex + Number(onInspectTask !== undefined)); }}
          onClick={() => {
            onClear();
            close(true);
          }}>
          <span className="codicon codicon-clear-all" aria-hidden="true" />
          <span>Clear scrollback</span>
        </button>
      )}
      {onSendSelection !== undefined && (
        <button type="button" className="terminal-menu-launch" role="menuitem"
          tabIndex={activeIndex === sendIndex ? 0 : -1}
          onFocus={() => { setActiveIndex(sendIndex); }}
          title="Put the selected text in a thread's draft"
          onClick={() => { close(false); onSendSelection(); }}>
          <span className="codicon codicon-comment" aria-hidden="true" />
          <span>Send selection to thread…</span>
        </button>
      )}
    </div>
  ) : null;

  const shown = passOff ?? panel;
  return (
    <>
      <button ref={triggerRef} type="button" className="terminal-pane-action"
        aria-label="More pane actions" title="More…"
        aria-haspopup="menu" aria-expanded={open}
        onClick={() => { if (!open) onLoadProfiles?.(); onOpenChange(!open); }}>
        <span className="codicon codicon-sm codicon-ellipsis" aria-hidden="true" />
      </button>
      {shown !== null && (typeof document === "undefined" ? shown : createPortal(shown, document.body))}
    </>
  );
}

/** Memoized: the workspace hands each pane only its own terminals (see
 *  `terminalPaneMembers`) and stable callbacks, so it redraws for itself. */
export const TerminalPane = memo(function TerminalPane({ pane, terminals, focused, maximized, onFocus, onSelectTab,
    onCloseTab, onMoveTerminal, onRequestSplit, onRequestPassOff, onClearTerminal, onSendSelection,
    onRestartTerminal,
    onToggleMaximize, onToggleFastMode, onRequestNewTab, onRequestNewSession, onOpenWorkspaceTool, onInspectTask,
    onContinueAsThread,
    launchProfiles, launchProfilesLoading, onLoadLaunchProfiles, flipped, locate,
    renderTerminal }: TerminalPaneProps) {
  const baseId = `${useId()}-terminal-pane`;
  const activeTerminal = terminals.get(pane.activeTerminalId);
  const activeTitle = activeTerminal?.title || "Terminal";
  // A Browser tab has no process: nothing to pass off, clear, speed up or
  // end when its tab closes.
  const browsing = activeTerminal?.profileId === BROWSER_PROFILE_ID;
  const pages = pane.tabs.filter(tab => terminals.get(tab.terminalId)?.profileId === BROWSER_PROFILE_ID).length;
  // A pane may hold terminals and pages together: each is counted as itself.
  const shells = pane.tabs.length - pages;
  const paneLabel = pages > 0 && shells === 0
    ? `Browser pane${pages === 1 ? "" : `, ${pages.toString()} pages`}`
    : `Terminal pane, ${shells} ${shells === 1 ? "terminal" : "terminals"}${
      pages === 0 ? "" : `, ${pages.toString()} ${pages === 1 ? "page" : "pages"}`}`;
  // A flipped card says where a Browser is, where a terminal says its folder.
  const cardStatus = browsing ? "Browser" : terminalStatusText(activeTerminal);
  const cardPlace = browsing ? activeTerminal?.url ?? "No page yet"
    : homeRelativePath(activeTerminal?.cwd ?? "");
  const paneRef = useRef<HTMLElement>(null);
  const [newMenuOpen, setNewMenuOpen] = useState(false);
  const [moreMenuOpen, setMoreMenuOpen] = useState(false);
  const [dropTarget, setDropTarget] = useState(false);
  const [locating, setLocating] = useState(false);
  useEffect(() => () => { endTerminalPaneDrag(pane.id); }, [pane.id]);
  // One pulse per request. The class comes off for a frame first, so choosing
  // the same pane twice restarts the ring instead of leaving it on.
  const locateNonce = locate != null
    && pane.tabs.some(tab => tab.terminalId === locate.terminalId) ? locate.nonce : null;
  useEffect(() => {
    if (locateNonce === null) return undefined;
    let timer: number | undefined;
    setLocating(false);
    const frame = window.requestAnimationFrame(() => {
      setLocating(true);
      timer = window.setTimeout(() => { setLocating(false); }, TERMINAL_PANE_LOCATE_MS);
    });
    return () => {
      window.cancelAnimationFrame(frame);
      if (timer !== undefined) window.clearTimeout(timer);
      setLocating(false);
    };
  }, [locateNonce]);
  const startDrag = (event: ReactDragEvent<HTMLElement>, terminalId: TerminalId): void => {
    const tab = [...(paneRef.current?.querySelectorAll<HTMLElement>(".terminal-tab") ?? [])]
      .find(node => node.dataset.terminalId === terminalId);
    if (!tab) { event.preventDefault(); return; }
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData(TERMINAL_TAB_DRAG_MIME, terminalId);
    beginTerminalPaneDrag(event.dataTransfer, pane.id, tab, event.clientX, event.clientY);
  };
  const splitOrientation = (): TerminalSplitOrientation => {
    const bounds = paneRef.current?.getBoundingClientRect();
    return terminalPaneSplitOrientation(bounds?.width ?? 0, bounds?.height ?? 0);
  };
  useEffect(() => {
    if (!dropTarget) return undefined;
    const clear = (): void => { setDropTarget(false); };
    const cancel = (event: KeyboardEvent): void => { if (event.key === "Escape") clear(); };
    window.addEventListener("dragend", clear, true);
    window.addEventListener("drop", clear, true);
    window.addEventListener("blur", clear);
    window.addEventListener("keydown", cancel, true);
    return () => {
      window.removeEventListener("dragend", clear, true);
      window.removeEventListener("drop", clear, true);
      window.removeEventListener("blur", clear);
      window.removeEventListener("keydown", cancel, true);
    };
  }, [dropTarget]);
  const acceptDrag = (event: Pick<ReactDragEvent<HTMLElement>, "dataTransfer" | "preventDefault">): boolean => {
    if (!hasTerminalTabData(event.dataTransfer)) return false;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    const sourcePaneId = terminalPaneDragSource();
    setDropTarget(sourcePaneId !== undefined && sourcePaneId !== pane.id);
    return true;
  };
  // A terminal that starts waiting for input or fails changes a 6px dot's hue
  // and bolds a tab title; a screen-reader user was told nothing until they
  // navigated back to that tab. WAITING AND FAILED ONLY: idle/working flips on
  // every burst of output, and announcing those is chatter that would make the
  // region worth switching off.
  const [announcement, setAnnouncement] = useState("");
  const announced = useRef(new Map<TerminalId, TerminalLiveKind>());
  useEffect(() => {
    const spoken: string[] = [];
    const next = new Map<TerminalId, TerminalLiveKind>();
    for (const tab of pane.tabs) {
      const terminal = terminals.get(tab.terminalId);
      const kind = terminalLiveKind(terminal);
      next.set(tab.terminalId, kind);
      // First sight is not a change: a pane that arrives holding a failed
      // terminal must not report a failure that happened before it existed.
      const previous = announced.current.get(tab.terminalId);
      if (previous === undefined || previous === kind) continue;
      if (kind !== "waiting" && kind !== "failed") continue;
      spoken.push(`${terminal?.title.trim() || "Terminal"}: ${terminalStatusText(terminal)}`);
    }
    announced.current = next;
    if (spoken.length > 0) setAnnouncement(spoken.join(". "));
  }, [pane.tabs, terminals]);
  // Emptied a moment after it is set, so a sentence about a state the terminal
  // has already left is not left sitting in the accessibility tree. It is not a
  // repeat mechanism: setting the same string again inside that window changes
  // nothing and says nothing, and once is the right number of times to be told.
  useEffect(() => {
    if (announcement === "") return undefined;
    const timer = window.setTimeout(() => { setAnnouncement(""); }, 1000);
    return () => { window.clearTimeout(timer); };
  }, [announcement]);
  const dropAt = (event: Pick<ReactDragEvent<HTMLElement>, "dataTransfer" | "preventDefault" | "stopPropagation">, targetIndex: number): void => {
    if (!hasTerminalTabData(event.dataTransfer)) return;
    const terminalId = draggedTerminalId(event.dataTransfer);
    if (terminalId === null) return;
    event.preventDefault();
    event.stopPropagation();
    setDropTarget(false);
    onMoveTerminal(terminalId, pane.id, targetIndex);
  };
  useEffect(() => {
    const element = paneRef.current;
    if (element === null) return;
    // Pooled terminal portals belong to the center in React, but to this pane
    // in the DOM. Route body focus and tab drops through their actual slot.
    // Header/tab events keep their React handlers and precise insertion index.
    const fromView = (event: Event): boolean => event.target instanceof Element
      && event.target.closest(".terminal-view") !== null;
    const focus = (event: Event): void => { if (fromView(event)) onFocus(pane.id); };
    const over = (event: DragEvent): void => {
      if (fromView(event) && event.dataTransfer !== null) acceptDrag({
        dataTransfer: event.dataTransfer, preventDefault: () => event.preventDefault(),
      });
    };
    const leave = (event: DragEvent): void => {
      if (fromView(event) && !(event.relatedTarget instanceof Node && element.contains(event.relatedTarget))) {
        setDropTarget(false);
      }
    };
    const drop = (event: DragEvent): void => {
      if (fromView(event) && event.dataTransfer !== null) dropAt({
        dataTransfer: event.dataTransfer, preventDefault: () => event.preventDefault(),
        stopPropagation: () => event.stopPropagation(),
      }, pane.tabs.length);
    };
    element.addEventListener("pointerdown", focus);
    element.addEventListener("focusin", focus);
    element.addEventListener("dragover", over);
    element.addEventListener("dragleave", leave);
    element.addEventListener("drop", drop);
    return () => {
      element.removeEventListener("pointerdown", focus);
      element.removeEventListener("focusin", focus);
      element.removeEventListener("dragover", over);
      element.removeEventListener("dragleave", leave);
      element.removeEventListener("drop", drop);
    };
  }, [pane.id, pane.tabs.length, onFocus, onMoveTerminal]);
  return (
    // The tile the grid, maximize and pane motion place around each pane.
    <div className="terminal-pane-frame">
    <section ref={paneRef} className={`terminal-pane${focused ? " is-focused" : ""}${
      maximized ? " is-maximized" : ""}${dropTarget ? " is-drop-target" : ""}${
      locating ? " is-locating" : ""}`}
      data-terminal-pane-id={pane.id}
      data-terminal-drop-zone="pane"
      aria-label={paneLabel}
      onPointerDown={() => { onFocus(pane.id); }}
      onFocusCapture={() => { onFocus(pane.id); }}
      onDragOver={event => { acceptDrag(event); }}
      onDragLeave={event => {
        const related = event.relatedTarget;
        if (related instanceof Node && event.currentTarget.contains(related)) return;
        setDropTarget(false);
      }}
      onDrop={event => { dropAt(event, pane.tabs.length); }}>
      <header className="terminal-pane-head" draggable
        aria-description="Drag onto another pane to swap positions"
        onDragStart={event => {
          const target = event.target;
          if (target instanceof Element && target.closest("button")) {
            event.preventDefault();
            return;
          }
          startDrag(event, pane.activeTerminalId);
        }}>
        <div className="terminal-tabs" role="tablist" aria-label="Terminals"
          aria-orientation="horizontal"
          onKeyDown={event => {
            const tabs = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
            const at = tabs.indexOf(document.activeElement as HTMLButtonElement);
            let next = -1;
            if (event.key === "ArrowRight") next = (Math.max(0, at) + 1) % tabs.length;
            else if (event.key === "ArrowLeft") {
              next = (at < 0 ? 0 : at - 1 + tabs.length) % tabs.length;
            } else if (event.key === "Home") next = 0;
            else if (event.key === "End") next = tabs.length - 1;
            if (next < 0 || tabs[next] === undefined) return;
            event.preventDefault();
            const terminalId = tabs[next].dataset.terminalId;
            if (terminalId !== undefined) onSelectTab(terminalId);
            tabs[next].focus();
          }}>
          {pane.tabs.map((tab, index) => {
            const terminal = terminals.get(tab.terminalId);
            const active = tab.terminalId === pane.activeTerminalId;
            const title = terminal?.title || "Terminal";
            const tabId = `${baseId}-tab-${encodeURIComponent(tab.terminalId)}`;
            const panelId = `${baseId}-panel-${encodeURIComponent(tab.terminalId)}`;
            const page = terminal?.profileId === BROWSER_PROFILE_ID;
            const closeLabel = page ? `Close ${title}` : `Close ${title} (ends it)`;
            return (
              <span key={tab.terminalId}
                className={`terminal-tab-wrap${active ? " is-active" : ""}${
                  terminalNeedsAttention(terminal) ? " has-unread" : ""}`}
                data-terminal-drop-index={index}
                onDragOver={event => { acceptDrag(event); event.stopPropagation(); }}
                onDrop={event => {
                  const bounds = event.currentTarget.getBoundingClientRect();
                  dropAt(event, terminalTabDropIndex(index, event.clientX,
                    bounds.left, bounds.width));
                }}
                role="presentation">
                <button type="button" id={tabId} className="terminal-tab" role="tab"
                  data-terminal-id={tab.terminalId}
                  draggable
                  aria-description="Drag onto another pane to swap positions, or within this pane to reorder tabs"
                  aria-selected={active} aria-controls={panelId} tabIndex={active ? 0 : -1}
                  title={`${terminal?.title ?? title} — ${
                    terminal?.url ?? (page ? "No page yet" : terminal?.cwd ?? "Working directory unavailable")}`}
                  onClick={() => { onSelectTab(tab.terminalId); }}
                  onDragStart={event => {
                    event.stopPropagation();
                    startDrag(event, tab.terminalId);
                  }}>
                  <span className={terminalStatusClass(terminal, "terminal-tab-status")}
                    aria-hidden="true" />
                  <TerminalProfileIcon profileId={terminal?.profileId}
                    sessionKey={terminal?.sessionKey} />
                  <span className="terminal-tab-title">{title}</span>
                  <span className="terminal-sr-only">, {terminalStatusText(terminal)}</span>
                </button>
                <button type="button" className="terminal-tab-close"
                  aria-label={closeLabel}
                  title={closeLabel}
                  onClick={event => {
                    event.stopPropagation();
                    onCloseTab(tab.terminalId);
                  }}>
                  <span className="codicon codicon-close" aria-hidden="true" />
                </button>
              </span>
            );
          })}
        </div>
        {/* The head is the tabs and the actions, nothing else (owner,
            2026-11-15): each tab's dot is its state, with no folder and no
            state in words. The one word left is the amber one beside the
            strip under the screen (owner, 2026-09-28: a 6px dot alone was
            easy to miss among eight panes). The tab already says it to
            assistive tech. */}
        {pane.tabs.some(tab => terminalNeedsAttention(terminals.get(tab.terminalId)))
          && <span className="terminal-pane-needs" aria-hidden="true">{statusWord("waiting")}</span>}
        <div className="terminal-pane-actions">
          {activeTerminal?.status === "running" && activeTerminal.fastMode?.version === 1
            && onToggleFastMode !== undefined && <TerminalFastModeToggle
              key={`${activeTerminal.id}:${activeTerminal.generation}`}
              mode={activeTerminal.fastMode}
              onToggle={() => { onToggleFastMode(activeTerminal.id); }} />}
          <TerminalPaneMoreMenu onInspectTask={onInspectTask && activeTerminal && !browsing ? () => onInspectTask(activeTerminal.id) : undefined} open={moreMenuOpen} onOpenChange={setMoreMenuOpen}
            onSplit={orientation => { onRequestSplit(pane.id, orientation); }}
            profiles={launchProfiles} profilesLoading={launchProfilesLoading}
            onLoadProfiles={onLoadLaunchProfiles}
            passOffEnabled={activeTerminal?.status === "running" && !browsing}
            onPassOff={onRequestPassOff === undefined ? undefined : profileId => {
              onRequestPassOff(pane.id, splitOrientation(), profileId);
            }}
            onClear={onClearTerminal === undefined || browsing ? undefined : () => {
              onClearTerminal(pane.activeTerminalId);
            }}
            onSendSelection={onSendSelection === undefined || browsing ? undefined : () => {
              onSendSelection(pane.activeTerminalId);
            }}
            {...(browsing || activeTerminal === undefined ? {} : {
              onCopy: () => { sendTerminalViewCommand(paneRef.current, "copy"); },
              copyEnabled: moreMenuOpen && terminalViewHasSelection(paneRef.current),
              onPaste: () => { sendTerminalViewCommand(paneRef.current, "paste"); },
            })}
            {...(onContinueAsThread === undefined || browsing || activeTerminal === undefined ? {} : {
              onContinueAsThread: () => { onContinueAsThread(activeTerminal.id); },
              continueRefusal: continueAsThreadRefusal(activeTerminal),
            })} />
          <button type="button" className="terminal-pane-action terminal-pane-expand"
            aria-label={maximized ? "Restore terminal layout" : "Maximize terminal pane"}
            title={maximized ? "Restore all panes" : "Expand pane for reading"}
            onClick={() => { onToggleMaximize(pane.id); }}>
            <span className={`codicon codicon-sm codicon-${maximized ? "collapse-diagonal" : "expand-diagonal"}`}
              aria-hidden="true" />
          </button>
          {launchProfiles === undefined
            ? <button type="button" className="terminal-pane-action"
                aria-label="New terminal in this pane"
                title="New terminal in this pane"
                onClick={() => { onRequestSplit(pane.id, "auto"); }}>
                <span className="codicon codicon-sm codicon-add" aria-hidden="true" />
              </button>
            : <TerminalPaneNewMenu open={newMenuOpen} profiles={launchProfiles}
                loading={launchProfilesLoading === true} onOpenChange={setNewMenuOpen}
                onLoad={onLoadLaunchProfiles}
                onChoose={profileId => {
                  onRequestSplit(pane.id, "auto", profileId);
                }}
                cwd={activeTerminal?.cwd}
                onOpenTool={onOpenWorkspaceTool && activeTerminal?.cwd
                  ? tool => { onOpenWorkspaceTool(tool, activeTerminal.cwd, pane.id); } : undefined}
                onSplitDown={() => { onRequestSplit(pane.id, "vertical"); }}
                onNewTab={onRequestNewTab && (() => { onRequestNewTab(pane.id); })}
                onNewSession={onRequestNewSession && (() => { onRequestNewSession(pane.id); })} />}
          {/* Ends the active terminal, exactly like the per-tab ×, which hides
              at one tab. A Browser tab has no process: closing it closes the page. */}
          {browsing
            ? <button type="button" className="terminal-pane-action"
                aria-label="Close browser" title="Close browser"
                onClick={() => { onCloseTab(pane.activeTerminalId); }}>
                <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
              </button>
            : <button type="button" className="terminal-pane-action"
                aria-label="Close active terminal (ends it)"
                title="Close active terminal (ends it)"
                onClick={() => { onCloseTab(pane.activeTerminalId); }}>
                <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
              </button>}
        </div>
      </header>
      <div className="terminal-pane-body">
        {pane.tabs.map(tab => {
          const active = tab.terminalId === pane.activeTerminalId;
          const tabId = `${baseId}-tab-${encodeURIComponent(tab.terminalId)}`;
          const panelId = `${baseId}-panel-${encodeURIComponent(tab.terminalId)}`;
          return (
            <div key={tab.terminalId} id={panelId}
              className={active ? "terminal-tabpanel is-active" : "terminal-tabpanel"}
              role="tabpanel" aria-labelledby={tabId}
              aria-hidden={!active || flipped || undefined}
              inert={!active || flipped || undefined}>
              {renderTerminal({ terminalId: tab.terminalId, active, paneId: pane.id })}
            </div>
          );
        })}
        {!flipped && activeTerminal?.status === "running"
          && activeTerminal.startupDownload?.version === 1
          && <TerminalStartupDownloadNotice download={activeTerminal.startupDownload} />}
        {!flipped && onRestartTerminal !== undefined && activeTerminal !== undefined
          && typeof activeTerminal.restartOffer === "string" && (
          <div className="terminal-restart-offer" role="status">
            <span className="terminal-restart-offer-text">{activeTerminal.restartOffer}</span>
            <button type="button" className="terminal-restart-offer-button"
              onClick={() => { onRestartTerminal(activeTerminal.id); }}>Start again</button>
          </div>
        )}
        {/* Flipped: the terminal is covered, never unmounted — unmounting
            disposes xterm and loses the buffer. */}
        {flipped && (
          <div className="terminal-pane-card"
            /* Built FROM the card's own strings, not instead of them: the
               override used to stop at title and state, so the folder the card
               prints and the preview below it — the two things that tell three
               shells in one folder apart — reached nobody reading by name. */
            aria-label={[activeTitle, cardStatus, cardPlace,
              ...activeTerminal?.preview ?? []]
              .filter(part => part !== "").join(", ")}>
            <span className="terminal-pane-card-head">
              <span className={terminalStatusClass(activeTerminal, "terminal-card-status")}
                aria-hidden="true" />
              <TerminalProfileIcon profileId={activeTerminal?.profileId}
                sessionKey={activeTerminal?.sessionKey} size={18} />
              <span className="terminal-pane-card-title" title={activeTitle}>{activeTitle}</span>
            </span>
            <span className="terminal-card-meta">
              <span>{cardStatus}</span>
              <span aria-hidden="true"> · </span>
              <span className="terminal-card-cwd">{cardPlace}</span>
            </span>
            {/* What the pane was last saying, so a flipped wall of cards still
                distinguishes three shells in the same folder. */}
            {(activeTerminal?.preview ?? []).length > 0 && (
              <span className="terminal-card-preview">
                {(activeTerminal?.preview ?? []).join("\n")}
              </span>
            )}
          </div>
        )}
      </div>
      <span className="terminal-sr-only" role="status">{announcement}</span>
    </section>
    </div>
  );
});
