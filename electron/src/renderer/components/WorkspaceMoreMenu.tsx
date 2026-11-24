import { useCallback, useEffect, useId, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { attentionTrayLabel, type AttentionRow } from "../attention-tray";
import { APP_SELECT_DISMISS_EVENT } from "./AppSelect";
import { AttentionBadge } from "./AttentionTray";
import { useTerminalMenuPopover } from "./TerminalMenu";
import { TerminalLayoutItems, type TerminalMenuModel } from "./TerminalWorkspace";

/**
 * The ··· menu's window destinations the command palette alone used to reach:
 * the Coordination canvas, Worktrees and the Context inventory (review C3,
 * 2026-10-14). Each glyph keeps its app-wide meaning: cross-model work,
 * a worktree, and what Agent customization lists. One not wired is not drawn.
 */
export function WorkspaceDestinationItems({ choose, onCanvas, onWorktrees, onContextInventory }: {
  readonly choose: (action: () => void) => void;
  readonly onCanvas?: () => void;
  readonly onWorktrees?: () => void;
  readonly onContextInventory?: () => void;
}) {
  const item = (label: string, icon: string, action?: () => void) => action && (
    <button type="button" className="terminal-menu-launch" role="menuitem"
      onClick={() => { choose(action); }}>
      <span className={`codicon codicon-${icon}`} aria-hidden="true" /><span>{label}</span>
    </button>
  );
  return <>
    {item("Coordination canvas", "git-compare", onCanvas)}
    {item("Worktrees", "type-hierarchy-sub", onWorktrees)}
    {item("Context inventory…", "robot", onContextInventory)}
  </>;
}

/**
 * Secondary window commands share one quiet, keyboard-accessible menu. It also
 * holds Attention, Usage and, on the terminal view, the layout controls — the
 * owner moved their toolbar buttons here to compact the top bar (2026-09-26).
 * The button carries the attention badge the bell used to, and its accessible
 * description says what the badge shows.
 */
export function WorkspaceMoreMenu({ buttonRef, inert, onCommands, attention, onUsage,
    layout, onOpenTasks, onOpenSavedSessions, onCanvas, onWorktrees, onContextInventory }: {
  buttonRef: RefObject<HTMLButtonElement | null>;
  inert: boolean;
  onCommands: () => void;
  /** The attention tray's rows; choosing the item opens the tray under this button. */
  attention?: {
    readonly rows: readonly AttentionRow[];
    readonly unread: number;
    readonly onOpen: () => void;
  };
  /** Opens the usage panel under this button. */
  onUsage?: () => void;
  /** The terminal layout section; absent off the terminal view. */
  layout?: TerminalMenuModel | null;
  onOpenTasks?: () => void;
  onOpenSavedSessions?: () => void;
  /** Destinations the palette alone used to reach (review C3, 2026-10-14). */
  onCanvas?: () => void;
  onWorktrees?: () => void;
  onContextInventory?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const descriptionId = useId();
  const expanded = open && !inert;
  const { triggerRef, menuRef, box, onMenuKey } = useTerminalMenuPopover({
    // Tall enough for every row at once; a short window still caps it.
    open: expanded, onOpenChange: setOpen, minWidth: 240, align: "end", maxHeight: 640,
  });
  useEffect(() => { if (inert) setOpen(false); }, [inert]);
  useEffect(() => {
    const dismiss = (): void => { setOpen(false); };
    window.addEventListener(APP_SELECT_DISMISS_EVENT, dismiss);
    return () => window.removeEventListener(APP_SELECT_DISMISS_EVENT, dismiss);
  }, []);
  // Stable, so the button never detaches mid-commit: the attention tray and
  // the usage panel measure it as their anchor from their own layout effects.
  const setButton = useCallback((node: HTMLButtonElement | null): void => {
    triggerRef.current = node;
    buttonRef.current = node;
  }, [buttonRef, triggerRef]);
  const choose = (action: () => void): void => {
    setOpen(false);
    // Hand modal focus ownership back to its stable opener before opening it.
    triggerRef.current?.focus();
    action();
  };
  const panel = expanded ? <div ref={menuRef} className="terminal-menu-panel is-floating"
    role="menu" aria-label="Workspace menu" onKeyDown={onMenuKey}
    style={box === null ? { visibility: "hidden", top: 0, left: 0 }
      : { top: box.top, left: box.left, width: box.width, maxHeight: box.maxHeight }}>
    {attention && <button type="button" className="terminal-menu-launch" role="menuitem"
      aria-label={attentionTrayLabel(attention.rows.length, attention.unread)}
      onClick={() => { choose(attention.onOpen); }}>
      <span className="codicon codicon-bell" aria-hidden="true" /><span>Attention</span>
      <span className="workspace-menu-end"><AttentionBadge rows={attention.rows} unread={attention.unread} /></span>
    </button>}
    {onUsage && <button type="button" className="terminal-menu-launch" role="menuitem"
      onClick={() => { choose(onUsage); }}>
      <span className="codicon codicon-dashboard" aria-hidden="true" /><span>Usage</span>
    </button>}
    {layout && <>
      <div className="terminal-pane-menu-divider" role="separator" />
      <TerminalLayoutItems menu={layout} choose={choose}
        onOpenTasks={onOpenTasks} onOpenSavedSessions={onOpenSavedSessions} />
    </>}
    {(attention || onUsage || layout) && (onCanvas || onWorktrees || onContextInventory)
      && <div className="terminal-pane-menu-divider" role="separator" />}
    <WorkspaceDestinationItems choose={choose} onCanvas={onCanvas} onWorktrees={onWorktrees}
      onContextInventory={onContextInventory} />
    {(attention || onUsage || layout || onCanvas || onWorktrees || onContextInventory)
      && <div className="terminal-pane-menu-divider" role="separator" />}
    <button type="button" className="terminal-menu-launch" role="menuitem"
      aria-label="Open command palette" tabIndex={0}
      onClick={() => { choose(onCommands); }}>
      <span className="codicon codicon-terminal" aria-hidden="true" />
      <span>Command palette</span><kbd className="workspace-menu-shortcut">⌘⇧P</kbd>
    </button>
  </div> : null;
  return <>
    <button ref={setButton}
      type="button" className="workspace-icon-btn workspace-more-button"
      aria-label="More workspace options"
      aria-describedby={attention === undefined ? undefined : descriptionId}
      title="Workspace commands" aria-haspopup="menu" aria-expanded={expanded}
      onClick={() => { setOpen(!open); }}>
      <span className="codicon codicon-ellipsis" aria-hidden="true" />
      {attention && <AttentionBadge rows={attention.rows} unread={attention.unread} />}
      {attention && <span id={descriptionId} className="terminal-sr-only">
        {attentionTrayLabel(attention.rows.length, attention.unread)}
      </span>}
    </button>
    {panel !== null && (typeof document === "undefined" ? panel : createPortal(panel, document.body))}
  </>;
}
