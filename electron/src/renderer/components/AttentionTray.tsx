import { useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import {
  attentionRowMeta, attentionTrayLabel, type AttentionRow,
} from "../attention-tray";
import { useTerminalMenuPopover } from "./TerminalMenu";

/**
 * The bell: one toolbar control that says how many things need the user, and
 * opens the list of them — what, where, since when — with each row a jump to
 * the agent or pane it names.
 *
 * The rows are the caller's; this draws them. Live rows (blocked agents and
 * panes) come and go with the fact; event rows (finished, failed) stay until
 * explicitly cleared. Clicking a row acknowledges viewing and navigates to it.
 */
export interface AttentionTrayProps {
  readonly rows: readonly AttentionRow[];
  /** Rows newer than the last open: the dot on the bell. */
  readonly unread: number;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onActivate: (row: AttentionRow) => void;
  readonly onInspect?: (row: AttentionRow) => void;
  readonly onClearAll: () => void;
  /** The clock ages are drawn against, from the app's shared tick. */
  readonly now: number;
  /** Open under this button instead of drawing a bell of its own: the tray
      lives in the window's ··· menu (owner, 2026-09-26), whose button carries
      the badge. */
  readonly anchorRef?: RefObject<HTMLElement | null>;
}

/** The bell's signal, wherever the tray's opener is: the live count of agents
    blocked on you, or else a quiet dot for unseen finished and failed rows. */
export function AttentionBadge({ rows, unread }: {
  readonly rows: readonly AttentionRow[];
  readonly unread: number;
}) {
  const liveCount = rows.filter(row => row.live).length;
  if (liveCount > 0) {
    return (
      <span className="attention-tray-count is-unread is-live" aria-hidden="true">
        {liveCount > 99 ? "99+" : liveCount}
      </span>
    );
  }
  return unread > 0 ? <span className="attention-tray-dot" aria-hidden="true" /> : null;
}

/** The quiet unread dot. Exported so the agent roster's unread mark is this
 *  same dot, drawn even where the tray itself is not mounted. */
export const ATTENTION_TRAY_DOT_CSS = `
.attention-tray-dot {
  flex: none;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--fg-dim);
  pointer-events: none;
}
`;

const TRAY_CSS = `
.workspace-icon-btn.attention-tray-button {
  width: auto;
  min-width: var(--control-h);
  gap: 4px;
  padding-inline: 4px;
}
/* Keep the count beside the bell, inside its hit target. Even a large unread
   count must not obscure the icon or crowd the neighboring control. */
.attention-tray-count {
  display: grid;
  flex: none;
  place-items: center;
  min-width: 16px;
  height: 16px;
  padding: 0 3px;
  color: var(--fg-muted);
  background: color-mix(in srgb, var(--fg) 12%, var(--workbench-canvas));
  border-radius: var(--radius-pill);
  font-size: var(--text-meta);
  font-weight: var(--wght-strong);
  font-variant-numeric: tabular-nums;
  line-height: 1;
  pointer-events: none;
}
.attention-tray-count.is-unread { color: var(--fg-strong); }
/* The number means "blocked on you" (owner, 2026-09-12): live needs only, in
   the app's one attention hue. Finished turns wait inside the tray behind a dot. */
.attention-tray-count.is-live { color: var(--status-waiting); }
${ATTENTION_TRAY_DOT_CSS}
.attention-tray-panel { width: 340px; max-width: min(340px, calc(100vw - 16px)); }
.attention-tray-head {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 28px;
  padding: 2px 6px 4px 10px;
  color: var(--fg-muted);
  font-size: var(--text-meta);
  font-weight: var(--wght-strong);
}
.attention-tray-head-spacer { flex: 1 1 auto; }
/* The Inspect result item draws exactly like the clear button beside the title. */
.attention-tray-clear,
.attention-tray-inspect {
  height: 22px;
  padding: 0 7px;
  color: var(--fg-muted);
  background: transparent;
  border: 0;
  border-radius: var(--radius-control);
  font: inherit;
  font-size: var(--text-meta);
  cursor: pointer;
}
.attention-tray-clear:hover,
.attention-tray-inspect:hover { color: var(--fg-strong); background: var(--row-hover); }
.attention-tray-empty {
  margin: 0;
  padding: 6px 10px 10px;
  color: var(--fg-dim);
  font-size: var(--text-ui);
}
/* Two lines per row: the title, then the state word, the place and the age.
   The row is a menuitem, so it borrows .terminal-menu-launch's hover, focus
   ring and roving tabindex; only the second line is its own. */
.attention-tray-row { align-items: flex-start; min-height: 44px; padding-top: 6px; padding-bottom: 6px; }
.attention-tray-row .codicon { flex: none; margin-top: 1px; }
.attention-tray-row-glyph.is-needs-input { color: var(--status-waiting); }
.attention-tray-row-glyph.is-failed { color: var(--status-failed); }
/* Finished is the blue done light every pane, tab and row shows. */
.attention-tray-row-glyph.is-finished { color: var(--status-done); }
.attention-tray-row-text { display: flex; flex: 1 1 auto; flex-direction: column; gap: 2px; min-width: 0; text-align: left; }
.attention-tray-row-title { overflow: hidden; color: var(--fg); text-overflow: ellipsis; white-space: nowrap; }
.attention-tray-row-meta {
  overflow: hidden;
  color: var(--fg-dim);
  font-size: var(--text-meta);
  text-overflow: ellipsis;
  white-space: nowrap;
}
`;

const GLYPH: Record<AttentionRow["kind"], string> = {
  "needs-input": "codicon-warning",
  failed: "codicon-error",
  finished: "codicon-pass",
};

export function AttentionTray({ rows, unread, open, onOpenChange, onActivate, onClearAll,
    now, onInspect, anchorRef }: AttentionTrayProps) {
  // Roving tabindex: one tab stop for the whole list, arrows move inside it.
  const [activeIndex, setActiveIndex] = useState(0);
  const { triggerRef, menuRef, box, close, onMenuKey } = useTerminalMenuPopover({
    open, onOpenChange, minWidth: 340, placeOn: rows.length, anchorRef,
    ...(anchorRef === undefined ? {} : { align: "end" as const }),
  });
  const events = rows.filter(row => !row.live).length;
  const panel = open ? (
    <div ref={menuRef} className="terminal-menu-panel is-floating attention-tray-panel" role="menu"
      aria-label="Attention" onKeyDown={onMenuKey}
      style={box === null
        ? { visibility: "hidden", top: 0, left: 0 }
        : { top: box.top, left: box.left, width: box.width, maxHeight: box.maxHeight }}>
      <div className="attention-tray-head">
        <span>Attention</span>
        <span className="attention-tray-head-spacer" />
        {events > 0 && (
          <button type="button" className="attention-tray-clear"
            aria-label="Clear finished and failed" title="Clear finished and failed"
            onClick={onClearAll}>
            Clear all
          </button>
        )}
      </div>
      {rows.length === 0
        ? <p className="attention-tray-empty">Nothing needs you right now.</p>
        : rows.map((row, index) => {
          const meta = attentionRowMeta(row, now);
          // One tab stop for the whole menu: Inspect is reached with the arrows.
          return <div key={row.id} role="none"><button type="button" role="menuitem"
            className="terminal-menu-launch attention-tray-row"
            aria-label={`${row.title}, ${meta}`}
            title={row.where === "" ? row.title : `${row.title} — ${row.where}`}
            tabIndex={activeIndex === index ? 0 : -1}
            onFocus={() => { setActiveIndex(index); }}
            onClick={() => {
              onActivate(row);
              close(true);
            }}>
            <span className={`codicon ${GLYPH[row.kind]} attention-tray-row-glyph is-${row.kind}`}
              aria-hidden="true" />
            <span className="attention-tray-row-text">
              <span className="attention-tray-row-title">{row.title}</span>
              <span className="attention-tray-row-meta">{meta}</span>
            </span>
          </button>{onInspect && !row.live && <button type="button" role="menuitem" tabIndex={-1}
            className="attention-tray-inspect"
            onClick={() => { onInspect(row); close(false); }}>Inspect result for {row.title}</button>}</div>;
        })}
    </div>
  ) : null;
  return (
    <>
      <style>{TRAY_CSS}</style>
      {anchorRef === undefined && <button ref={triggerRef} type="button"
        className={open ? "workspace-icon-btn attention-tray-button is-active" : "workspace-icon-btn attention-tray-button"}
        aria-label={attentionTrayLabel(rows.length, unread)}
        title="Attention"
        aria-haspopup="menu" aria-expanded={open}
        onClick={() => { onOpenChange(!open); }}>
        <span className="codicon codicon-bell" aria-hidden="true" />
        <AttentionBadge rows={rows} unread={unread} />
      </button>}
      {panel !== null && (typeof document === "undefined" ? panel : createPortal(panel, document.body))}
    </>
  );
}
