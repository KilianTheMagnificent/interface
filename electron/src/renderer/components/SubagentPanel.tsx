import { Icon, StateGlyph } from "./glyphs";
import { useEffect, useMemo, useRef } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import type { ConvItem, SubagentStatus } from "../conversation-reducer";
import { compactSeamLabel } from "../conversation-reducer";
import { displayPath, shortenSummaryPaths } from "../display-paths";
import {
  subagentGlyph, subagentStatusSentence, subagentStatusWord, type RunState,
} from "../tool-activity";
import "../codicons.css";
import "../theme.css";

/* ============================================================================
   THE SUBAGENT DETAIL PANEL — the right side-window

   The transcript cell for a spawned subagent is one row. It can be one row
   ONLY because everything it cannot hold lives here: the untruncated task, the
   prompt the parent actually sent, the model, the status in a sentence rather
   than a `title` attribute, and a read-only list of what the agent has done.

   THIS IS NOT CLAUDE CODE PARITY, and nothing in this file should ever claim
   it is. Claude Code has no right side-window — its `/tasks` list sits below
   the prompt and a subagent transcript opens in place. A pushed third column
   is this app's own answer to a desktop window running up to four concurrent
   sessions, where an overlay would cover the transcript the cell lives in.

   ---- PRESENTATIONAL, BY CONSTRUCTION -------------------------------------
   Props in, callbacks out. No `window.iface`, no store, no data fetching, no
   document-level listeners. App owns the `{ sessionKey, agentKey }` selection,
   resolves it to an item, and passes the result here; this file decides only
   what that looks like. See SubagentPanelProps for the exact contract.

   ---- WHAT IT DELIBERATELY DOES NOT DO ------------------------------------
   1. It does NOT re-render `children` as live transcript rows. A second
      SearchableItem pass would register the same findKey from two DOM nodes
      (the later registration wins, and Enter-to-next-match then scrolls to the
      wrong node), draw a second `aria-live` status region for one agent, and
      offer a second Allow/Deny pair for one approvalId. The activity list here
      is DERIVED and read-only; an approval it shows says "Needs you" and sends
      the user back to the transcript, where the one answerable card lives.
   2. It invents no timing. No EngineEvent carries a timestamp, so `timing` is
      an OPTIONAL caller-supplied label: absent, the panel prints nothing about
      duration rather than a number it made up.
   3. It never prints an empty labelled row. Every optional field is gated on
      having content — a heading over nothing is worse than no heading.
   ========================================================================= */

/** The subagent this panel describes.
 *
 *  Structurally a superset-compatible view of the `subagent` member of
 *  ConvItem (src/renderer/conversation-reducer.ts), so App can pass the reduced item
 *  STRAIGHT THROUGH with no mapping layer:
 *
 *      <SubagentPanel agent={findSubagentItem(items, agentKey)} … />
 *
 *  `result` is declared optional here and is the one field the item may not
 *  carry yet (see SubagentPanelProps). Absent, the panel falls back to
 *  `lastActivity` under an honest "Latest activity" heading rather than
 *  labelling an 80-character clip as the full result. */
export interface SubagentPanelAgent {
  /** The agentKey. A panel key and a callback argument — never displayed. */
  id: string;
  subagentType: string;
  description: string;
  status: SubagentStatus;
  toolCount: number;
  subagentCount: number;
  /** Present when the spawn named a model, or the engine's catalogue did.
   *  Absent — the ordinary case — means it inherits the parent's. */
  model?: string;
  /** One line, clipped to 80 chars upstream. Not the full result. */
  lastActivity?: string;
  /** The untruncated closing summary, when the reducer carries one. */
  result?: string;
  /** Reduced children. Rendered as a derived read-only list, never as live rows. */
  children?: readonly ConvItem[];
}

export interface SubagentPanelProps {
  /** The selected subagent. Undefined draws the empty (or missing) state. */
  agent?: SubagentPanelAgent;
  /** True when a selection outlived its subject — tile closed, session
   *  switched, agentKey gone from the rebuilt items. Consulted ONLY when
   *  `agent` is undefined. */
  missing?: boolean;
  /** Last known name for the missing state, when App kept one. */
  missingLabel?: string;
  /** The full task prompt the parent sent. App reads `input.prompt` off the
   *  raw `tool.call` whose callId === agentKey; the reducer drops that call
   *  from the items (the cell IS that call) but the event log still holds it.
   *  Omit and the block is not drawn. */
  prompt?: string;
  /** The type of the subagent that spawned this one, for the tree line. */
  parentType?: string;
  /** An already-formatted duration label, e.g. "running 2m 34s". THE RENDERER
   *  HAS NO CLOCK SOURCE: pass this only if App stamps arrival per agentKey.
   *  Omitted means the panel says nothing about time. */
  timing?: string;
  /** Session root, for the same path shortening the transcript rows use. */
  pathRoot?: string;
  /** Move focus into the panel on open. Mirror the activation's `viaKeyboard`
   *  (`event.detail === 0`): a mouse user stays in the transcript they were
   *  reading, a keyboard user is taken to what they just opened. */
  focusOnOpen?: boolean;
  /** Monotonic request from the shell. Lets Enter on an already-selected row
   *  move focus into the panel again even though the agent key did not change. */
  focusRequest?: number;
  /** Close: the header button, and Escape from inside the panel. App restores
   *  focus to the `.sub-row` that opened it. */
  onClose: () => void;
  /** Scroll the owning cell back into view. The button is drawn only when
   *  this is supplied. */
  onReveal?: () => void;
  /** Re-target the panel at the parent. Draws the tree line as a breadcrumb
   *  button instead of static text. */
  onSelectParent?: () => void;
  /** Re-target the panel at a nested spawn listed in the activity list. */
  onSelectChild?: (agentKey: string) => void;
}

export const SUBAGENT_PANEL_CSS = `
.subp { display:flex; flex-direction:column; min-width:0; height:100%; overflow:hidden;
  color:var(--fg); background:var(--material-chrome);
  border-left:var(--border-w) solid var(--border); font-family:var(--font-ui); font-size:var(--text-ui); }
/* Focus lands on the region itself when the keyboard opened it, so the ring
   paints for the keyboard and stays out of a mouse user's way. */
.subp:focus { outline:none; }
.subp:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }

.subp-head { display:flex; flex:none; align-items:center; gap:6px;
  height:var(--section-header-h); padding:0 2px 0 8px;
  border-bottom:var(--border-w) solid var(--border); }
.subp-head-icon { flex:none; color:var(--fg-faint); }
.subp-title { flex:1 1 auto; min-width:0; margin:0; overflow:hidden;
  color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-strong);
  text-overflow:ellipsis; white-space:nowrap; }
.subp-iconbtn { display:inline-flex; flex:none; align-items:center; justify-content:center;
  width:var(--control-h); height:var(--control-h); padding:0; color:var(--fg-muted);
  background:transparent; border:0; border-radius:var(--radius-control); cursor:pointer; }
.subp-iconbtn:hover { color:var(--fg); background:var(--row-hover); }
.subp-iconbtn:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }

.subp-body { flex:1 1 auto; min-height:0; overflow-y:auto; padding:8px 8px 16px; }
.subp-block { margin:0 0 10px; }
.subp-label { margin:0 0 3px; color:var(--fg-dim); font-size:var(--text-meta); font-weight:var(--wght-strong); }
.subp-text { margin:0; color:var(--fg); font-size:var(--text-ui); line-height:var(--lh-ui);
  white-space:pre-wrap; overflow-wrap:anywhere; }
.subp-mono { font-family:var(--font-mono); font-size:var(--text-meta); }
.subp-quiet { color:var(--fg-muted); }
.subp-scroll { max-height:180px; overflow-y:auto; padding:6px; background:var(--editor-bg);
  border:var(--border-w) solid var(--border); border-radius:var(--radius-control); }

.subp-parent { margin:0 0 8px; color:var(--fg-muted); font-size:var(--text-meta); }
.subp-link { padding:0; color:var(--accent); background:transparent; border:0;
  font:inherit; text-decoration:underline; cursor:pointer; }
.subp-link:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }

.subp-status { display:flex; align-items:center; gap:6px; min-height:var(--row-h); }
.subp-status-word { color:var(--fg); font-size:var(--text-ui); }
.subp-status-time { color:var(--fg-dim); font-size:var(--text-meta); font-variant-numeric:tabular-nums; }
.subp-note { margin:0; color:var(--fg-muted); font-size:var(--text-meta); line-height:var(--lh-ui); }

.subp-figs { display:flex; gap:16px; }
.subp-fig { display:flex; flex-direction:column; gap:1px; }
.subp-fig-n { color:var(--fg); font-size:var(--text-prose); font-variant-numeric:tabular-nums; }
.subp-fig-l { color:var(--fg-dim); font-size:var(--text-meta); }

.subp-acts { display:flex; flex-direction:column; margin:0; padding:0; list-style:none; }
.subp-act { display:flex; align-items:center; gap:6px; min-width:0; min-height:var(--row-h);
  padding:0 4px; border-radius:var(--radius-control); }
.subp-act-block { align-items:flex-start; padding:4px; }
.subp-act-nested { padding:0; }
.subp-act-open { display:flex; align-items:center; gap:6px; box-sizing:border-box; width:100%;
  min-width:0; min-height:var(--row-h); padding:0 4px; color:inherit; background:transparent;
  border:0; border-radius:var(--radius-control); font:inherit; text-align:left; cursor:pointer; }
.subp-act-open:hover { background:var(--row-hover); }
.subp-act-open:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }
.subp-act-glyph { display:inline-flex; flex:none; align-items:center; justify-content:center;
  width:var(--icon-size); color:var(--fg-dim); }
.subp-act-name { flex:none; max-width:52%; overflow:hidden; color:var(--fg); font-size:var(--text-ui);
  text-overflow:ellipsis; white-space:nowrap; }
.subp-act-sum { flex:1 1 auto; min-width:0; overflow:hidden; color:var(--fg-muted);
  font-size:var(--text-meta); text-overflow:ellipsis; white-space:nowrap; }
.subp-act-flag { flex:none; color:var(--status-waiting); font-size:var(--text-meta); font-weight:var(--wght-strong); }
.subp-act-prose { display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:3;
  min-width:0; margin:0; overflow:hidden; color:var(--fg-muted); font-size:var(--text-meta); line-height:var(--lh-ui); }

.subp-ok { color:var(--diff-added); }
.subp-fail { color:var(--status-failed); }
.subp-idle { color:var(--fg-dim); }

.subp-empty { display:flex; flex-direction:column; gap:6px; padding:16px 12px 12px; }
.subp-empty-title { margin:0; color:var(--fg); font-size:var(--text-ui); font-weight:var(--wght-strong); }
.subp-empty-copy { margin:0; color:var(--fg-muted); font-size:var(--text-meta); line-height:var(--lh-ui); }

.subp-sr { position:absolute; width:1px; height:1px; margin:-1px; padding:0; overflow:hidden;
  clip-path:inset(50%); white-space:nowrap; border:0; }
`;
/* No transition and no animation is declared above on purpose. The one moving
   thing the panel draws is the app's pulsing dot, whose reduced-motion guard
   already lives in theme.css; adding a fade here would need a second guard for
   no gain. */

/* ---------------------------------------------------------------------------
   Pure helpers. Exported so each branch is testable without a DOM.
   ------------------------------------------------------------------------ */

function countText(count: number, one: string): string {
  return `${count} ${count === 1 ? one : `${one}s`}`;
}

/**
 * "2 subagents · 17 tools", in the cell's own order — delegated work first,
 * because an agent that split its task across three others has said more with
 * that than with the one Read it did on the way. "" when it has done neither,
 * which is the signal to draw no figures at all.
 */
export function subagentCountsText(toolCount: number, subagentCount: number): string {
  return [
    subagentCount > 0 ? countText(subagentCount, "subagent") : null,
    toolCount > 0 ? countText(toolCount, "tool") : null,
  ].filter((part): part is string => part !== null).join(" · ");
}

/** Escape is the panel's only key. Anything else belongs to whatever has focus
 *  — the find bar and the composer both own Escape elsewhere in this window,
 *  which is why the handler is bound to the panel subtree and never to the
 *  document. */
export function subagentPanelKeyAction(key: string): "close" | null {
  return key === "Escape" ? "close" : null;
}

/** One derived, read-only line of a subagent's activity. */
export interface SubagentActivityRow {
  key: string;
  /** A codicon name, drawn when `state` is null. Both null: an empty column,
   *  so names stay aligned. */
  icon: string | null;
  /** The app's state vocabulary, which outranks a kind icon when known. */
  state: RunState | null;
  /** Spoken state, for the glyph that carries no text. "" draws nothing. */
  stateWord: string;
  name: string;
  summary: string;
  /** A prose child, line-clamped instead of ellipsised. "" for line rows. */
  prose: string;
  /** Non-empty only on a nested spawn: the agentKey to re-target the panel at. */
  agentKey: string;
  /** An approval nobody has answered. The one row worth a visible flag. */
  attention: boolean;
}

function emptyRow(key: string): SubagentActivityRow {
  return { key, icon: null, state: null, stateWord: "", name: "", summary: "",
    prose: "", agentKey: "", attention: false };
}

/**
 * The subagent's children, flattened into one line each. DERIVED: this never
 * mounts a transcript renderer, so it cannot double-register a findKey, draw a
 * second live region, or offer a second Allow/Deny for one approvalId.
 *
 * A nested spawn is one row that re-targets the panel — never an inline tree,
 * which is what the transcript's own fold is for.
 */
export function subagentActivityRows(
  children: readonly ConvItem[], root?: string, parentStatus?: SubagentStatus,
): SubagentActivityRow[] {
  return children.map(child => {
    // ConvItems are never mutated once reduced, so a child's row holds for as
    // long as the path root and the parent's status do. A streamed token into
    // the watched agent re-derives the one child it changed.
    const cached = ACTIVITY_ROWS.get(child);
    if (cached !== undefined && cached.root === root && cached.parentStatus === parentStatus) {
      return cached.row;
    }
    const row = activityRow(child, root, parentStatus);
    ACTIVITY_ROWS.set(child, { root, parentStatus, row });
    return row;
  });
}

const ACTIVITY_ROWS = new WeakMap<ConvItem, {
  root: string | undefined; parentStatus: SubagentStatus | undefined; row: SubagentActivityRow;
}>();

function activityRow(
  child: ConvItem, root: string | undefined, parentStatus: SubagentStatus | undefined,
): SubagentActivityRow {
  const row = emptyRow(child.id);
  switch (child.kind) {
    case "text":
      return { ...row, icon: "comment", prose: shortenSummaryPaths(child.text, root) };
    case "user":
      return { ...row, icon: "comment-discussion",
        prose: shortenSummaryPaths(child.text, root) };
    case "tool": {
      const result = child.result;
      const unresolvedIsRunning = result === undefined
        && (parentStatus === undefined || parentStatus === "running");
      const state: RunState = result === undefined
        ? unresolvedIsRunning ? "running" : "ended"
        : result.ok ? "ok" : "failed";
      const word = result === undefined
        ? unresolvedIsRunning ? "Running" : "No result"
        : result.ok ? "Done" : "Failed";
      const said = result !== undefined && result.summary !== ""
        ? result.summary : child.summary;
      return { ...row, state, stateWord: word, name: child.tool,
        summary: shortenSummaryPaths(said, root) };
    }
    case "approval": {
      const resolved = child.resolved;
      return { ...row, icon: "warning", name: child.tool,
        summary: shortenSummaryPaths(child.summary, root),
        stateWord: resolved === "allow" ? "Allowed"
          : resolved === "deny" ? "Denied" : "Needs you",
        attention: resolved === undefined };
    }
    // Unreachable today — a peer.result carries no agentKey, so the reducer
    // always buckets it to the main thread — but a row that ever does appear
    // here must still name the source and keep the untrusted mark: this panel
    // is a second place the reader meets the text, and a line that drops the
    // mark is the same relabelling in a smaller window.
    case "peer":
      return { ...row, icon: "comment-discussion",
        name: `${child.sourceEngine} · untrusted`,
        prose: shortenSummaryPaths(child.text, root) };
    case "diff":
      return { ...row, icon: "git-compare", name: displayPath(child.path, root) };
    case "notice":
      return { ...row,
        icon: child.tone === "error" ? "error" : child.tone === "warning" ? "warning" : "info",
        summary: shortenSummaryPaths(child.text, root) };
    case "compact":
      return { ...row, icon: "info",
        summary: compactSeamLabel(child) };
    // Unreachable today (a visual carries no agentKey): named, never framed.
    case "visual":
      return { ...row, icon: "open-preview", name: "Visual", summary: child.title };
    case "subagent": {
      return { ...row, icon: "robot", state: subagentGlyph(child.status),
        stateWord: subagentStatusWord(child.status),
        name: child.subagentType, agentKey: child.id,
        summary: subagentCountsText(child.toolCount, child.subagentCount) };
    }
    default: {
      // Unreachable while every ConvItem member has a case above, and a
      // compile error the moment one does not.
      const unhandled: never = child;
      return unhandled;
    }
  }
}

/* ---------------------------------------------------------------------------
   The component.
   ------------------------------------------------------------------------ */

/** A labelled block, drawn only when it has something to say. */
function Block({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section className="subp-block">
      <h3 className="subp-label">{label}</h3>
      {children}
    </section>
  );
}

function ActivityLine({ row }: { row: SubagentActivityRow }) {
  return <>
    <span className="subp-act-glyph">
      {row.state !== null ? <StateGlyph state={row.state} tier="panel" />
        : row.icon !== null ? <Icon name={row.icon} sm /> : null}
    </span>
    {/* The hidden word is the glyph's voice. An unanswered approval already
        prints that word visibly, so it is not said twice. */}
    {row.stateWord !== "" && !row.attention
      && <span className="subp-sr">{row.stateWord}</span>}
    {row.name !== "" && <span className="subp-act-name">{row.name}</span>}
    {row.summary !== "" && <span className="subp-act-sum">{row.summary}</span>}
    {row.attention && <span className="subp-act-flag">Needs you</span>}
  </>;
}

/**
 * One subagent, in detail, beside the transcript that spawned it.
 *
 * Three states: an agent (everything below), a dead selection (`missing`), and
 * nothing selected. The empty state ORIENTS rather than apologises, and exists
 * for an integrator that keeps the column mounted; the shipped behaviour is
 * for App to render no column at all when nothing is selected, because 340px
 * reading "nothing selected" is 340px of nothing on most screens.
 */
export function SubagentPanel({
  agent, missing, missingLabel, prompt, parentType, timing, pathRoot, focusOnOpen,
  focusRequest, onClose, onReveal, onSelectParent, onSelectChild,
}: SubagentPanelProps) {
  const rootRef = useRef<HTMLElement>(null);
  const agentKey = agent?.id;

  // Keyboard activation only (see focusOnOpen). Re-runs when the panel is
  // re-targeted at another agent, so keyboard navigation between nested spawns
  // keeps focus where the user is looking.
  useEffect(() => {
    if (focusOnOpen !== true) return;
    rootRef.current?.focus();
  }, [focusOnOpen, focusRequest, agentKey]);

  // Bound to the panel subtree, never to the document: Escape typed in the
  // find bar or the composer must keep belonging to them.
  function handleKeyDown(event: KeyboardEvent<HTMLElement>): void {
    if (subagentPanelKeyAction(event.key) === null) return;
    event.stopPropagation();
    onClose();
  }

  // `??` let "" through: App initialises the remembered name to the empty string,
  // so a selection that never resolved rendered a blank <h2> over a MissingDetail
  // that handles the same value carefully. Both paths agree now.
  const title = agent?.subagentType
    ?? (missing === true && missingLabel !== undefined && missingLabel.trim() !== ""
      ? missingLabel : "Subagent");
  const label = agent === undefined ? "Subagent details"
    : `${agent.subagentType} subagent details`;

  return (
    <aside ref={rootRef} className="subp" role="complementary" aria-label={label}
      tabIndex={-1} onKeyDown={handleKeyDown}>
      <style>{SUBAGENT_PANEL_CSS}</style>

      <header className="subp-head">
        <Icon name="robot" sm className="subp-head-icon" />
        <h2 className="subp-title" title={title}>{title}</h2>
        {agent !== undefined && onReveal !== undefined && (
          <button type="button" className="subp-iconbtn" onClick={onReveal}
            title="Reveal in transcript" aria-label="Reveal in transcript">
            <Icon name="list-tree" />
          </button>
        )}
        <button type="button" className="subp-iconbtn" onClick={onClose}
          title="Close (Esc)" aria-label="Close subagent details">
          <Icon name="close" />
        </button>
      </header>

      <div className="subp-body">
        {agent !== undefined
          ? <AgentDetail agent={agent} prompt={prompt} parentType={parentType}
              timing={timing} pathRoot={pathRoot} onSelectParent={onSelectParent}
              onSelectChild={onSelectChild} />
          : missing === true ? <MissingDetail name={missingLabel} />
          : <EmptyDetail />}
      </div>
    </aside>
  );
}

const NO_CHILDREN: readonly ConvItem[] = [];

function AgentDetail({ agent, prompt, parentType, timing, pathRoot,
    onSelectParent, onSelectChild }: {
  agent: SubagentPanelAgent;
  prompt?: string;
  parentType?: string;
  timing?: string;
  pathRoot?: string;
  onSelectParent?: () => void;
  onSelectChild?: (agentKey: string) => void;
}) {
  const description = agent.description.trim();
  const promptText = (prompt ?? "").trim();
  const counts = subagentCountsText(agent.toolCount, agent.subagentCount);
  // The result, or the honest lesser thing. Never both: once a subagent has
  // finished, `lastActivity` IS its closing summary at 80 characters, so
  // printing the two would be one string and its own clip under two headings.
  const result = (agent.result ?? "").trim();
  const activity = (agent.lastActivity ?? "").trim();
  // App hands over the same agent object until a frame touches it, so a stream
  // elsewhere in the session re-derives nothing here.
  const rows = useMemo(() => subagentActivityRows(agent.children ?? NO_CHILDREN, pathRoot,
    agent.status), [agent.children, pathRoot, agent.status]);
  const shownResult = useMemo(() => result === "" ? ""
    : shortenSummaryPaths(result, pathRoot), [result, pathRoot]);
  const parent = (parentType ?? "").trim();
  const clock = (timing ?? "").trim();

  return <>
    {parent !== "" && (
      <p className="subp-parent">
        Spawned by{" "}
        {onSelectParent === undefined
          ? <span>{parent}</span>
          : <button type="button" className="subp-link" onClick={onSelectParent}>
              {parent}
            </button>}
      </p>
    )}

    {description !== "" && (
      <Block label="Task">
        {/* In full and wrapped — this is the copy the row suppresses when it
            only repeats the agent's name, and the copy the row ellipsises
            when it does not. */}
        <p className="subp-text">{description}</p>
      </Block>
    )}

    {promptText !== "" && (
      <Block label="Prompt sent">
        <div className="subp-scroll">
          <p className="subp-text subp-mono">{promptText}</p>
        </div>
      </Block>
    )}

    <Block label="Model">
      {agent.model === undefined || agent.model.trim() === ""
        // Not a blank row and not a guess: the engines omit the field for a
        // spawn that runs on whatever the parent is running.
        ? <p className="subp-text subp-quiet">Inherits the parent&rsquo;s model</p>
        : <p className="subp-text subp-mono">{agent.model}</p>}
    </Block>

    <Block label="Status">
      {/* Deliberately not a live region: the transcript cell already owns the
          `role="status"` announcement for this agent, and a second one makes a
          screen reader say every change twice. */}
      <p className="subp-status">
        <StateGlyph state={subagentGlyph(agent.status)} tier="panel" />
        <span className="subp-status-word">{subagentStatusWord(agent.status)}</span>
        {clock !== "" && <span className="subp-status-time">{clock}</span>}
      </p>
      <p className="subp-note">{subagentStatusSentence(agent.status)}</p>
    </Block>

    {counts !== "" && (
      <Block label="Work">
        <div className="subp-figs">
          {agent.subagentCount > 0 && (
            <div className="subp-fig">
              <span className="subp-fig-n">{agent.subagentCount}</span>
              <span className="subp-fig-l">
                {agent.subagentCount === 1 ? "subagent" : "subagents"}
              </span>
            </div>
          )}
          {agent.toolCount > 0 && (
            <div className="subp-fig">
              <span className="subp-fig-n">{agent.toolCount}</span>
              <span className="subp-fig-l">{agent.toolCount === 1 ? "tool" : "tools"}</span>
            </div>
          )}
        </div>
      </Block>
    )}

    {result !== "" ? (
      <Block label="Result">
        <p className="subp-text">{shownResult}</p>
      </Block>
    ) : activity !== "" ? (
      <Block label="Latest activity">
        <p className="subp-text">{shortenSummaryPaths(activity, pathRoot)}</p>
      </Block>
    ) : null}

    {rows.length > 0 && (
      <Block label="Activity">
        <ul className="subp-acts">
          {rows.map(row => {
            if (row.prose !== "") {
              return (
                <li key={row.key} className="subp-act subp-act-block">
                  <span className="subp-act-glyph">
                    {row.icon !== null && <Icon name={row.icon} sm />}
                  </span>
                  <p className="subp-act-prose">{row.prose}</p>
                </li>
              );
            }
            if (row.agentKey !== "" && onSelectChild !== undefined) {
              const open = onSelectChild;
              const key = row.agentKey;
              return (
                <li key={row.key} className="subp-act subp-act-nested">
                  <button type="button" className="subp-act-open"
                    onClick={() => open(key)}
                    title={`Inspect ${row.name}`}
                    aria-label={`Inspect ${row.name} subagent`}>
                    <ActivityLine row={row} />
                  </button>
                </li>
              );
            }
            return (
              <li key={row.key} className="subp-act">
                <ActivityLine row={row} />
              </li>
            );
          })}
        </ul>
      </Block>
    )}
  </>;
}

/** The selection outlived its subject. Says so plainly, once. */
function MissingDetail({ name }: { name?: string }) {
  const known = (name ?? "").trim();
  return (
    <div className="subp-empty" role="status">
      <p className="subp-empty-title">
        {known === "" ? "That subagent is no longer in this transcript."
          : `${known} is no longer in this transcript.`}
      </p>
      <p className="subp-empty-copy">
        Its pane was closed, or the session it ran in was switched or reloaded.
        Pick another subagent to inspect it here.
      </p>
    </div>
  );
}

/** Nothing selected. Orients — what this column is for and how to fill it —
 *  rather than apologising for being blank. */
function EmptyDetail() {
  return (
    <div className="subp-empty">
      <p className="subp-empty-title">Subagent details</p>
      <p className="subp-empty-copy">
        Select a subagent in a transcript to open it here: its full task, the
        prompt it was given, its model, status and everything it has done.
      </p>
      <p className="subp-empty-copy">
        That is what keeps each subagent one line in the transcript instead of
        a card.
      </p>
    </div>
  );
}
