import { providerLabel } from "../provider-labels";
import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent, ReactNode } from "react";
import type { EngineId, SessionKey } from "../../shared/engine";
import "../codicons.css";
import "../theme.css";
import { ConfirmDialog } from "./ConfirmDialog";
import {
  logicalListFocusIndex,
  rovingTabKeyAction,
  useModalFocus,
} from "./ModalBehavior";

export type SessionLibraryTab = "search" | "archives" | "worktrees";

export interface SessionSearchHitView {
  key: SessionKey;
  engine: EngineId;
  title: string;
  cwd: string;
  excerpts: readonly string[];
}

export interface ArchivedSessionView {
  key: SessionKey;
  engine: EngineId;
  title: string;
  cwd: string;
  /** ISO 8601. Rendered as an age; the exact instant stays on hover. */
  lastActivity: string;
}

export interface ManagedWorktreeView {
  key: SessionKey;
  path: string;
  sourceCwd: string;
  branch?: string;
  state: "active" | "retained" | "missing";
  /** Present only after the backend has inspected current checkout status. */
  dirty?: boolean;
}

export interface SessionLibraryProps {
  open: boolean;
  tab: SessionLibraryTab;
  onTabChange: (tab: SessionLibraryTab) => void;
  query: string;
  onQueryChange: (query: string) => void;
  onSearch: () => void;
  searchHits: readonly SessionSearchHitView[];
  searchLoading?: boolean;
  searchError?: string | null;
  searchIncomplete?: readonly string[];
  archived: readonly ArchivedSessionView[];
  archivedLoading?: boolean;
  worktrees: readonly ManagedWorktreeView[];
  worktreesLoading?: boolean;
  notice?: { message: string; error?: boolean } | null;
  /**
   * Clock the archived ages are measured against. Defaults to the render's own
   * `Date.now()`, which is right for a dialog that is opened, read and closed;
   * pass the shell's ticking clock instead and the ages stay live while it sits
   * open. Tests pass a fixed instant.
   */
  now?: number;
  onRefresh: () => void;
  onOpenSession: (key: SessionKey) => void;
  onRestoreSession: (key: SessionKey) => void;
  /** Authorize and open the exact checkout Interface recorded for this session. */
  onOpenWorktree: (key: SessionKey) => void;
  /** Verify/authorize the recorded checkout, then open its root in a terminal. */
  onOpenTerminal: (key: SessionKey) => void;
  onCleanWorktree: (key: SessionKey) => void;
  onClose: () => void;
}

export const SESSION_LIBRARY_CSS = `
/* ONE overlay recipe. This dialog, the command palette and the file picker are
   the same kind of surface, and they shipped three: 48% dim here against 60%
   there, 68px of offset against max(64px, 10vh), one that animated in and two
   that did not, and two row geometries. The palette's is the recipe. */
.session-library-backdrop {
  position:fixed; inset:0; z-index:1100; display:grid; place-items:start center;
  padding:max(64px, 10vh) 24px 24px;
  background:color-mix(in srgb, var(--chrome-bg) 60%, transparent);
  animation:session-library-backdrop-in var(--motion-base) var(--ease-standard) both;
}
.session-library {
  display:grid; grid-template-rows:auto minmax(0, 1fr); width:min(760px, 100%);
  transform-origin:50% 0;
  animation:session-library-dialog-in var(--motion-slow) var(--ease-out) both;
  height:min(590px, calc(100vh - 92px)); overflow:hidden; color:var(--fg);
  background:var(--editor-bg); border:var(--border-w) solid var(--border);
  border-radius:var(--radius-overlay); box-shadow:var(--overlay-shadow); font-family:var(--font-ui);
  container-type:inline-size;
}
.session-library, .session-library *, .session-library *::before, .session-library *::after { box-sizing:border-box; }
.session-library-head {
  display:flex; align-items:stretch; gap:8px; min-height:38px; padding:0 6px 0 12px;
  background:var(--editor-bg); border-bottom:var(--border-w) solid var(--border);
}
.session-library-title {
  display:flex; align-items:center; margin:0 4px 0 0; color:var(--fg-strong);
  font-size:var(--text-ui); font-weight:var(--wght-strong);
}
.session-library-tabs { display:flex; align-items:stretch; gap:0; }
.session-library-tab, .session-library-icon, .session-library-action {
  min-height:28px; padding:0 8px; color:var(--fg-muted); background:transparent;
  border:var(--border-w) solid transparent; border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-meta); cursor:pointer;
}
.session-library-tab { border-radius:0; }
.session-library-tab:not(:disabled):hover, .session-library-icon:not(:disabled):hover,
.session-library-action:not(:disabled):hover {
  color:var(--fg-strong); background:var(--row-hover);
}
/* The horizontal-tab selected look, in the app's ONE selection hue. This drew
   the underline in --accent (bronze) while the toolbar's surface switch drew
   the same underline in --selection-accent (blue), two feet apart. */
.session-library-tab[aria-selected="true"] {
  color:var(--fg-strong); background:transparent;
  box-shadow:inset 0 -2px 0 var(--selection-accent);
}
.session-library-spacer { flex:1 1 auto; }
.session-library-icon {
  display:inline-grid; place-items:center; align-self:center; width:28px; padding:0;
}
.session-library-body { min-width:0; min-height:0; overflow:auto; }
.session-library-panel { min-width:0; min-height:100%; outline:none; }
.session-library-search {
  display:flex; gap:6px; padding:10px 12px; border-bottom:var(--border-w) solid var(--border);
}
.session-library-input {
  flex:1 1 auto; min-width:0; height:30px; padding:0 9px; color:var(--input-fg);
  background:transparent; border:var(--border-w) solid var(--input-border);
  border-radius:var(--radius-control); font:inherit;
}
.session-library-input:focus { background:var(--input-bg); }
.session-library-action { min-height:28px; border-color:transparent; }
.session-library-search .session-library-action {
  color:var(--fg); background:var(--surface-raised); border-color:var(--border);
}
.session-library-action:disabled { color:var(--fg-faint); cursor:default; background:transparent; }
.session-library-list { margin:0; padding:0 12px 12px; list-style:none; }
.session-library-row {
  display:grid; grid-template-columns:minmax(0, 1fr) auto; gap:12px; min-width:0;
  min-height:38px; padding:5px 8px; border-radius:0;
}
.session-library-row + .session-library-row { border-top:var(--border-w) solid var(--border); }
.session-library-row:hover { background:color-mix(in srgb, var(--row-hover) 54%, transparent); }
.session-library-copy { min-width:0; }
.session-library-name {
  display:block; overflow:hidden; color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-strong);
  text-overflow:ellipsis; white-space:nowrap;
}
.session-library-meta {
  display:block; margin-top:2px; overflow:hidden; color:var(--fg-dim); font-family:var(--font-mono); font-size:var(--text-meta);
  line-height:var(--lh-ui); text-overflow:ellipsis; white-space:nowrap;
}
.session-library-excerpt {
  display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:2; margin:6px 0 0;
  overflow:hidden; color:var(--fg-muted); font-size:var(--text-meta); line-height:var(--lh-ui); overflow-wrap:anywhere;
}
.session-library-search-hit { padding-block:12px; }
.session-library-result-count { padding:10px 20px 6px; color:var(--fg-muted); font-size:var(--text-meta); }
.session-library-more { margin-top:8px; color:var(--fg-muted); font-size:var(--text-meta); }
.session-library-more > summary { cursor:pointer; width:fit-content; padding-block:2px; }
.session-library-more[open] .session-library-excerpt { display:block; }
.session-library-more ul { margin:8px 0; padding-left:18px; line-height:var(--lh-ui); overflow-wrap:anywhere; }
.session-library-warning > p { margin:0; }
.session-library-copy mark { background:var(--selection); color:var(--fg-strong); }
.session-library :where(summary,button,input):focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.session-library-actions {
  display:flex; flex-wrap:wrap; align-items:center; align-self:start; gap:1px;
}
.session-library-state { padding:34px 16px; color:var(--fg-dim); text-align:center; font-size:var(--text-meta); }
/* A notice about the search or the library, not a summons: a 2px amber left rail
   is the app's "an agent is blocked on you" shape, and this banner says things
   like "Partial results". The rail stays currentColor, so the .is-error variant
   below still paints its own rail in the danger tier. */
.session-library-warning {
  margin:8px 12px 0; padding:5px 8px; color:var(--fg-muted);
  border-left:2px solid currentColor; font-size:var(--text-meta);
}
.session-library-error { color:var(--danger-fg); }
/* Uncommitted work in a managed worktree is a version-control mark, the same
   fact the file tree spends --vcs-modified on — never the attention hue. */
.session-library-dirty { color:var(--vcs-modified); }
.session-library-sr-only { position:absolute; width:1px; height:1px; padding:0; margin:-1px;
  overflow:hidden; clip:rect(0,0,0,0); white-space:nowrap; border:0; }
@container (max-width:750px) {
  .session-library-head { display:grid; grid-template-columns:minmax(0,1fr) 28px 28px; }
  .session-library-title { grid-column:1; grid-row:1; min-width:0; }
  .session-library-tabs { grid-column:1 / -1; grid-row:2; min-width:0; overflow-x:auto; }
  .session-library-tab { flex:1 0 auto; }
  .session-library-spacer { display:none; }
  .session-library-icon { grid-row:1; }
  .session-library-row { grid-template-columns:minmax(0,1fr); gap:8px; }
  .session-library-actions { justify-content:flex-start; }
}
@container (max-width:460px) {
  .session-library-search { flex-wrap:wrap; }
  .session-library-input, .session-library-search .session-library-action {
    flex:1 0 100%; width:100%;
  }
}
@media (max-width:760px) {
  .session-library-backdrop { padding:40px 8px 8px; }
  .session-library { height:calc(100vh - 48px); }
}
@keyframes session-library-backdrop-in { from { opacity:0; } to { opacity:1; } }
@keyframes session-library-dialog-in {
  from { opacity:0; transform:translateY(-8px) scale(.985); }
  to { opacity:1; transform:translateY(0) scale(1); }
}
@media (prefers-reduced-motion: reduce) {
  .session-library-backdrop, .session-library { animation:none; }
}
`;

/** Literal, case-insensitive highlights; provider text is always escaped by React. */
export function highlightSearchText(text: string, query: string): ReactNode {
  const needle = query.trim();
  if (!needle) return text;
  const escaped = needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const parts: ReactNode[] = [];
  let start = 0;
  for (const match of text.matchAll(new RegExp(escaped, "gi"))) {
    const at = match.index;
    parts.push(text.slice(start, at), <mark key={at}>{match[0]}</mark>);
    start = at + match[0].length;
  }
  parts.push(text.slice(start));
  return parts;
}

export function libraryProjectLabel(path: string): string {
  return path.replace(/\/+$/, "").split("/").at(-1) || path || "No project";
}

/** Preserve the original on hover when a provider supplied a path or protocol preamble as its title. */
export function librarySessionTitle(title: string, engine: string): string {
  if (!title.trim() || /^["']?\//.test(title) || /^<local-command-/.test(title)) {
    return `${providerLabel(engine)} session`;
  }
  return title;
}

function SearchHit({ hit, query, onOpen }: {
  hit: SessionSearchHitView; query: string; onOpen: () => void;
}) {
  const excerpts = [...new Set(hit.excerpts)];
  return <li className="session-library-row session-library-search-hit">
    <div className="session-library-copy">
      <span className="session-library-name" title={hit.title}>
        {highlightSearchText(librarySessionTitle(hit.title, hit.engine), query)}
      </span>
      <span className="session-library-meta" title={hit.cwd}>
        {providerLabel(hit.engine)} · {libraryProjectLabel(hit.cwd)}
      </span>
      {excerpts.slice(0, 2).map((excerpt, index) =>
        <p key={index} className="session-library-excerpt">{highlightSearchText(excerpt, query)}</p>)}
      {excerpts.length > 2 && <details className="session-library-more">
        <summary>Show {excerpts.length - 2} more {excerpts.length - 2 === 1 ? "match" : "matches"}</summary>
        {excerpts.slice(2).map((excerpt, index) =>
          <p key={index} className="session-library-excerpt">{highlightSearchText(excerpt, query)}</p>)}
      </details>}
    </div>
    <div className="session-library-actions">
      <button type="button" className="session-library-action" onClick={onOpen}>Open</button>
    </div>
  </li>;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * "now" / "45s" / "12m" / "3h" / "2d" — one field, always short enough to sit in
 * a meta row beside the project name.
 *
 * Returns "" for anything unparseable rather than "NaN", and clamps a future
 * timestamp to "now": clock skew between the engine's store and this machine is
 * real, and "in -3s" is worse than a small lie.
 */
export function relativeAge(iso: string | undefined, now: number): string {
  if (iso === undefined || iso === "") return "";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "";
  const ms = now - at;
  if (ms < 10_000) return "now";
  if (ms < MINUTE) return `${Math.floor(ms / 1000)}s`;
  if (ms < HOUR) return `${Math.floor(ms / MINUTE)}m`;
  if (ms < DAY) return `${Math.floor(ms / HOUR)}h`;
  return `${Math.floor(ms / DAY)}d`;
}

/**
 * The archived list used to print the stored ISO string — millisecond
 * precision, UTC, unreadable at a glance. `relativeAge` prints "8m" instead,
 * with the machine timestamp kept on hover and in `dateTime` so nothing is lost.
 *
 * An unparseable value falls back to printing itself: `relativeAge` returns ""
 * for one, and a silently blank age would be a worse lie than an ugly string.
 *
 * A stamp in the FUTURE gets the same treatment, and that is the one place this
 * deliberately departs from `relativeAge`, which clamps ahead-of-now to "now".
 * That is the right call for a live row — clock skew between an engine's store
 * and this machine is real, and a running agent really was active a moment ago.
 * Here it would be a plain falsehood: every row in this list is a session that
 * has finished, so "now" states that a finished agent is active this second.
 * There is no elapsed time to print, so the row prints the instant itself,
 * which is true and is what it printed before ages arrived.
 */
function ArchivedAge({ iso, now }: { iso: string; now: number }) {
  const at = Date.parse(iso);
  // Same guard `relativeAge` applies, restated here because the two branches
  // below both need `at`; an empty or foreign string parses to NaN.
  if (Number.isNaN(at)) return <>{iso}</>;
  if (at > now) return <time dateTime={iso} title={iso}>{iso}</time>;
  return <time dateTime={iso} title={iso}>{relativeAge(iso, now)}</time>;
}

const labelForState = (item: ManagedWorktreeView): string => {
  if (item.state === "missing") return "Missing";
  if (item.state === "active") return item.dirty ? "Active · changed" : "Active";
  if (item.dirty === true) return "Retained · changed";
  if (item.dirty === false) return "Retained · clean";
  return "Retained · cleanup rechecks changes";
};

const LIBRARY_TABS = ["search", "archives", "worktrees"] as const;
type RowAction = "restore" | "open" | "terminal" | "cleanup";
type PendingRowFocus = {
  tab: "archives" | "worktrees";
  key: SessionKey;
  index: number;
  action: RowAction;
};

const actionRefKey = (tab: "archives" | "worktrees", key: SessionKey, action: RowAction): string =>
  `${tab}:${key}:${action}`;

export function SessionLibrary(props: SessionLibraryProps) {
  const { open, tab, onTabChange, query, onQueryChange, onSearch, searchHits,
    searchLoading = false, searchError = null, searchIncomplete = [], archived,
    archivedLoading = false, worktrees, worktreesLoading = false, notice = null, now, onRefresh,
    onOpenSession, onRestoreSession, onOpenWorktree, onOpenTerminal, onCleanWorktree,
    onClose } = props;
  const clock = now ?? Date.now();
  const input = useRef<HTMLInputElement>(null);
  const dialogRef = useRef<HTMLElement>(null);
  const tabRefs = useRef<Partial<Record<SessionLibraryTab, HTMLButtonElement | null>>>({});
  const rowActionRefs = useRef<Map<string, HTMLButtonElement>>(new Map());
  const pendingRowFocus = useRef<PendingRowFocus | null>(null);
  const lastRowFocus = useRef<PendingRowFocus | null>(null);
  const baseId = useId();
  // Clean up removes a checkout from disk, so it confirms like Delete
  // workspace does; the host still keeps a worktree with uncommitted work.
  const [cleanupTarget, setCleanupTarget] = useState<ManagedWorktreeView | null>(null);
  // Closing the library (Escape, backdrop, the shell) drops the pending
  // confirm, so reopening does not present it again.
  useEffect(() => {
    if (!open) setCleanupTarget(null);
  }, [open]);

  const modalKeyDown = useModalFocus({
    open,
    containerRef: dialogRef,
    initialFocus: () => tab === "search" ? input.current : tabRefs.current[tab] ?? null,
    onEscape: onClose,
  });

  useEffect(() => {
    if (!open) {
      pendingRowFocus.current = null;
      lastRowFocus.current = null;
      rowActionRefs.current.clear();
      return;
    }

    const pending = pendingRowFocus.current;
    const last = lastRowFocus.current;
    const request = pending ?? last;
    if (request === null || request.tab !== tab) return;

    const items = request.tab === "archives" ? archived : worktrees;
    const exactIndex = items.findIndex(item => item.key === request.key);
    const actionSettled = pending !== null && (exactIndex < 0 || notice !== null);
    const focusWasRemoved = pending === null
      && dialogRef.current?.contains(document.activeElement) !== true;
    if (!actionSettled && !focusWasRemoved) return;

    const targetIndex = exactIndex >= 0
      ? exactIndex : logicalListFocusIndex(request.index, items.length);
    const targetKey = targetIndex >= 0 ? items[targetIndex]?.key : undefined;
    const preferred = targetKey === undefined ? undefined
      : rowActionRefs.current.get(actionRefKey(request.tab, targetKey, request.action));
    const primary = targetKey === undefined ? undefined
      : rowActionRefs.current.get(actionRefKey(
          request.tab,
          targetKey,
          request.tab === "archives" ? "restore" : "open",
        ));
    const target = preferred !== undefined && !preferred.disabled ? preferred
      : primary !== undefined && !primary.disabled ? primary : null;
    if (target?.isConnected) target.focus();
    else tabRefs.current[request.tab]?.focus();
    pendingRowFocus.current = null;
  }, [archived, notice, open, tab, worktrees]);

  if (!open) return null;

  const rememberAction = (
    request: PendingRowFocus,
    node: HTMLButtonElement | null,
  ): void => {
    const refKey = actionRefKey(request.tab, request.key, request.action);
    if (node === null) rowActionRefs.current.delete(refKey);
    else rowActionRefs.current.set(refKey, node);
  };

  const focusAfterAction = (request: PendingRowFocus): void => {
    pendingRowFocus.current = request;
    lastRowFocus.current = request;
  };

  const handleTabKey = (
    event: KeyboardEvent<HTMLButtonElement>,
    value: SessionLibraryTab,
  ): void => {
    const currentIndex = LIBRARY_TABS.indexOf(value);
    const action = rovingTabKeyAction(event.key, currentIndex, LIBRARY_TABS.length);
    if (action === null) return;
    event.preventDefault();
    event.stopPropagation();
    const next = LIBRARY_TABS[action.index]!;
    onTabChange(next);
    tabRefs.current[next]?.focus();
  };

  const stopBackdrop = (event: MouseEvent<HTMLDivElement>): void => {
    if (event.target === event.currentTarget) onClose();
  };

  const anyLoading = searchLoading || archivedLoading || worktreesLoading;

  return <>
    <style>{SESSION_LIBRARY_CSS}</style>
    <div className="session-library-backdrop" onMouseDown={stopBackdrop}>
      <section ref={dialogRef} className="session-library is-floating" role="dialog" aria-modal="true"
        tabIndex={-1} aria-labelledby={`${baseId}-title`} onKeyDown={modalKeyDown}>
        <header className="session-library-head">
          <h2 id={`${baseId}-title`} className="session-library-title">Library</h2>
          <div className="session-library-tabs" role="tablist" aria-label="Library sections"
            aria-orientation="horizontal">
            {LIBRARY_TABS.map(value =>
              <button key={value} id={`${baseId}-${value}-tab`} type="button" role="tab"
                ref={node => { tabRefs.current[value] = node; }}
                className="session-library-tab" aria-selected={tab === value}
                aria-label={value === "search" ? "Search transcripts" : undefined}
                aria-controls={`${baseId}-${value}-panel`}
                tabIndex={tab === value ? 0 : -1}
                onKeyDown={event => { handleTabKey(event, value); }}
                onClick={() => { onTabChange(value); }}>
                {value === "search" ? "Search"
                  : value === "archives" ? "Archived" : "Worktrees"}
              </button>)}
          </div>
          <span className="session-library-spacer" />
          <button type="button" className="session-library-icon" title="Refresh"
            aria-label="Refresh library" disabled={anyLoading} onClick={onRefresh}>
            <span className="codicon codicon-refresh" aria-hidden="true" />
          </button>
          <button type="button" className="session-library-icon" title="Close"
            aria-label="Close session library" onClick={onClose}>
            <span className="codicon codicon-close" aria-hidden="true" />
          </button>
        </header>

        <div className="session-library-body">
          {notice !== null && <div
            className={notice.error
              ? "session-library-warning session-library-error"
              : "session-library-warning"}
            role={notice.error ? "alert" : "status"} aria-live="polite">
            {notice.message}
          </div>}

          <div id={`${baseId}-search-panel`} className="session-library-panel"
            role="tabpanel" aria-labelledby={`${baseId}-search-tab`} tabIndex={0}
            aria-busy={searchLoading} hidden={tab !== "search"}>
            <div className="session-library-search">
              <input ref={input} className="session-library-input" value={query}
                placeholder="Search across session transcripts" aria-label="Search session transcripts"
                maxLength={256} onChange={event => { onQueryChange(event.currentTarget.value); }}
                onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); onSearch(); } }} />
              <button type="button" className="session-library-action"
                disabled={searchLoading || query.trim() === ""} onClick={onSearch}>
                {searchLoading ? "Searching…" : "Search"}
              </button>
            </div>
            {searchIncomplete.length > 0 && <div className="session-library-warning">
              <p role="status">Partial results — some provider history could not be searched.</p>
              <details className="session-library-more"><summary>Search details</summary>
                <ul>{searchIncomplete.map((reason, index) => <li key={index}>{reason}</li>)}</ul>
              </details>
            </div>}
            {searchError !== null
              ? <div className="session-library-state session-library-error" role="alert">{searchError}</div>
              : searchLoading
                ? <div className="session-library-state" role="status">Searching bounded provider history…</div>
                : searchHits.length === 0
                  ? <div className="session-library-state">Enter a phrase to search past sessions.</div>
                  : <><div className="session-library-result-count" role="status" aria-live="polite">
                      {searchHits.length} {searchHits.length === 1 ? "session" : "sessions"} found.
                    </div><ul className="session-library-list">{searchHits.map(hit =>
                      <SearchHit key={hit.key} hit={hit} query={query}
                        onOpen={() => { onOpenSession(hit.key); }} />)}</ul></>}
          </div>

          <div id={`${baseId}-archives-panel`} className="session-library-panel"
            role="tabpanel" aria-labelledby={`${baseId}-archives-tab`} tabIndex={0}
            aria-busy={archivedLoading} hidden={tab !== "archives"}>
            {archivedLoading
            ? <div className="session-library-state" role="status">Loading archived sessions…</div>
            : archived.length === 0
              ? <div className="session-library-state">No locally archived sessions.</div>
              : <><div className="session-library-sr-only" role="status" aria-live="polite">
                  {archived.length} archived {archived.length === 1 ? "session" : "sessions"}.
                </div><ul className="session-library-list">{archived.map((item, index) =>
                  <li key={item.key} className="session-library-row">
                    <div className="session-library-copy">
                      <span className="session-library-name" title={item.title}>{librarySessionTitle(item.title, item.engine)}</span>
                      <span className="session-library-meta">
                        {providerLabel(item.engine)} · <span title={item.cwd}>{libraryProjectLabel(item.cwd)}</span> ·{" "}
                        <ArchivedAge iso={item.lastActivity} now={clock} />
                      </span>
                    </div>
                    <div className="session-library-actions">
                      <button type="button" className="session-library-action"
                        ref={node => { rememberAction(
                          { tab: "archives", key: item.key, index, action: "restore" }, node,
                        ); }}
                        onFocus={() => { lastRowFocus.current = {
                          tab: "archives", key: item.key, index, action: "restore",
                        }; }}
                        onClick={() => {
                          focusAfterAction({
                            tab: "archives", key: item.key, index, action: "restore",
                          });
                          onRestoreSession(item.key);
                        }}>Restore</button>
                    </div>
                  </li>)}</ul></>}
          </div>

          <div id={`${baseId}-worktrees-panel`} className="session-library-panel"
            role="tabpanel" aria-labelledby={`${baseId}-worktrees-tab`} tabIndex={0}
            aria-busy={worktreesLoading} hidden={tab !== "worktrees"}>
            {worktreesLoading
            ? <div className="session-library-state" role="status">Inspecting managed worktrees…</div>
            : worktrees.length === 0
              ? <div className="session-library-state">
                  No managed worktrees. Start a new chat in an isolated worktree to create one.
                </div>
              : <><div className="session-library-sr-only" role="status" aria-live="polite">
                  {worktrees.length} managed {worktrees.length === 1 ? "worktree" : "worktrees"}.
                </div><ul className="session-library-list">{worktrees.map((item, index) =>
                  <li key={item.key} className="session-library-row">
                    <div className="session-library-copy">
                      <span className="session-library-name">{item.branch ?? item.key}</span>
                      <span className="session-library-meta">{item.path}</span>
                      <span className={item.dirty ? "session-library-meta session-library-dirty"
                        : "session-library-meta"}>{labelForState(item)} · from {item.sourceCwd}</span>
                    </div>
                    <div className="session-library-actions">
                      <button type="button" className="session-library-action"
                        ref={node => { rememberAction(
                          { tab: "worktrees", key: item.key, index, action: "open" }, node,
                        ); }}
                        disabled={item.state === "missing"}
                        onFocus={() => { lastRowFocus.current = {
                          tab: "worktrees", key: item.key, index, action: "open",
                        }; }}
                        onClick={() => { onOpenWorktree(item.key); }}>Open workspace</button>
                      <button type="button" className="session-library-action"
                        ref={node => { rememberAction(
                          { tab: "worktrees", key: item.key, index, action: "terminal" }, node,
                        ); }}
                        disabled={item.state === "missing"}
                        onFocus={() => { lastRowFocus.current = {
                          tab: "worktrees", key: item.key, index, action: "terminal",
                        }; }}
                        onClick={() => { onOpenTerminal(item.key); }}>Terminal</button>
                      {item.state === "retained" && <button type="button"
                        className="session-library-action"
                        ref={node => { rememberAction(
                          { tab: "worktrees", key: item.key, index, action: "cleanup" }, node,
                        ); }}
                        onFocus={() => { lastRowFocus.current = {
                          tab: "worktrees", key: item.key, index, action: "cleanup",
                        }; }}
                        onClick={() => { setCleanupTarget(item); }}>Clean up</button>}
                    </div>
                  </li>)}</ul></>}
          </div>
        </div>
      </section>
    </div>
    <ConfirmDialog open={cleanupTarget !== null} title="Clean up this worktree?"
      description={cleanupTarget === null ? ""
        : `Removes the worktree checkout at ${cleanupTarget.path} from disk. It is kept if it still holds uncommitted work. This cannot be undone.`}
      confirmLabel="Clean up" tone="danger"
      onConfirm={() => {
        if (cleanupTarget === null) return;
        const index = worktrees.findIndex(item => item.key === cleanupTarget.key);
        focusAfterAction({ tab: "worktrees", key: cleanupTarget.key, index, action: "cleanup" });
        setCleanupTarget(null);
        onCleanWorktree(cleanupTarget.key);
      }}
      onCancel={() => { setCleanupTarget(null); }} />
  </>;
}
