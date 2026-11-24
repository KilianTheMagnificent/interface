import type { CSSProperties, RefObject } from "react";
import type { DiffStat } from "../diff-stats";
import { clamp } from "../helpers";

/**
 * A READ-ONLY preview of one session, beside the sidebar row the pointer is
 * resting on.
 *
 * It answers "what is this agent doing, and does it need me" without taking the
 * user out of whatever they are reading — the question that otherwise costs a
 * session switch, a scroll and a switch back. Nothing in it is actionable: an
 * approval is ANSWERED in the session, never from a card that
 * appears under the pointer and leaves when the pointer does. A hover surface
 * with an Allow button on it is a mis-click waiting to happen.
 *
 * Every field is honestly omitted when unknown. A card that has only a title
 * and a status word is the correct card for a session this window has seen
 * nothing from.
 */

/** The facts the card shows that the sidebar row itself does not carry. */
export interface SessionPeekFacts {
  /** What the agent last did, in one line — `sessionActivity`'s own wording. */
  activity?: string;
  /** Distinct files this window watched it change. */
  files?: number;
  /**
   * The request holding it up, VERBATIM — the same string the transcript
   * shows, from the same `pendingApproval`. Quoting it is the whole point: a
   * paraphrase of a permission prompt is how someone approves the wrong thing.
   */
  approval?: string;
}

export interface SessionPeekProps {
  /** Matches the row's `aria-describedby`. */
  id: string;
  title: string;
  badge: "waiting" | "working" | "idle" | "failed";
  /** The state's one word, from the shared status table. */
  status: string;
  /** Compact recency; "" when the timestamp is unreadable. */
  age: string;
  /** null draws no Changes row at all. */
  diff: DiffStat | null;
  /** The managed checkout's name; "" draws no Worktree row at all. */
  worktree: string;
  /** A thread's pull request, "#42 · open"; absent draws no PR row. */
  pullRequest?: string;
  facts: SessionPeekFacts;
  /** Placement from `positionSessionPeek`, or null while it is being measured. */
  box: SessionPeekPosition | null;
  /** The caller measures the rendered card to place it. */
  cardRef?: RefObject<HTMLDivElement | null>;
}

export interface SessionPeekRect {
  top: number; bottom: number; left: number; right: number;
}
export interface SessionPeekSize { width: number; height: number }
export interface SessionPeekViewport { width: number; height: number }
export interface SessionPeekPosition { top: number; left: number }

/** Clear of the pane, and clear of the window edge. */
const PEEK_GAP = 8;
const PEEK_GUTTER = 8;

/**
 * Beside the row, never over it: the row stays readable while its own preview
 * is open, which is what lets the pointer keep walking down the list.
 *
 * Right of the sidebar by preference. It flips to the left only when the card
 * would not fit — a narrow window with the pane on the right — and the top is
 * clamped so a row near the bottom of a long list still gets a whole card.
 */
export function positionSessionPeek(
  anchor: SessionPeekRect,
  size: SessionPeekSize,
  viewport: SessionPeekViewport,
): SessionPeekPosition {
  const rightOf = anchor.right + PEEK_GAP;
  const left = rightOf + size.width + PEEK_GUTTER <= viewport.width
    ? rightOf
    : Math.max(PEEK_GUTTER, anchor.left - PEEK_GAP - size.width);
  return {
    top: clamp(anchor.top, PEEK_GUTTER, viewport.height - PEEK_GUTTER - size.height),
    left,
  };
}

// The overlay idiom, as every other floating surface in the app draws it:
// --widget-bg over a 1px --border-strong, --radius-overlay, --overlay-shadow.
// Colours are tokens; no literal may appear in this file.
export const SESSION_PEEK_CSS = `
.session-peek {
  position: fixed;
  z-index: 100;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: 296px;
  max-width: calc(100vw - 16px);
  padding: 11px 12px;
  color: var(--fg);
  background: var(--widget-bg);
  border: var(--border-w) solid var(--border-strong);
  border-radius: var(--radius-overlay);
  box-shadow: var(--overlay-shadow);
  font-family: var(--font-ui);
  /* Read-only, and out of the pointer's way: the card must never intercept a
     click meant for the row underneath it, and never keep itself open by
     swallowing the pointerleave that should dismiss it. */
  pointer-events: none;
  animation: session-peek-in var(--motion-fast) var(--ease-out) both;
}
@keyframes session-peek-in {
  from { opacity: 0; }
  to { opacity: 1; }
}
@media (prefers-reduced-motion: reduce) {
  .session-peek { animation: none; }
}

.session-peek-head { display: flex; align-items: center; gap: 8px; min-width: 0; }
.session-peek-title {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  color: var(--fg-strong);
  font-size: var(--text-body);
  font-weight: var(--wght-strong);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.session-peek-age {
  flex: none;
  color: var(--fg-dim);
  font-size: var(--text-meta);
  font-variant-numeric: tabular-nums;
}
.session-peek-age.is-waiting { color: var(--status-waiting); }
.session-peek-age.is-failed { color: var(--status-failed); }

/* A definition list, because that is what these rows are: a label and the one
   fact it names. The 52px label column is what makes three unlike facts scan
   as one block. */
.session-peek-facts {
  display: grid;
  grid-template-columns: 52px minmax(0, 1fr);
  gap: 5px 8px;
  margin: 0;
  font-size: var(--text-ui);
  line-height: var(--lh-ui);
}
.session-peek-label { color: var(--fg-dim); }
.session-peek-value { min-width: 0; margin: 0; color: var(--fg-muted); overflow-wrap: anywhere; }
.session-peek-added { color: var(--diff-added); font-variant-numeric: tabular-nums; }
.session-peek-removed {
  margin-left: 4px;
  color: var(--diff-removed);
  font-variant-numeric: tabular-nums;
}
.session-peek-worktree { font-family: var(--font-mono); font-size: var(--text-meta); }

/* The amber border IS the meaning: this block is the one thing on the card
   that is about the user rather than about the agent. */
.session-peek-ask {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px 10px;
  border: var(--border-w) solid var(--status-waiting);
  border-radius: var(--radius-container);
}
.session-peek-ask-title {
  color: var(--status-waiting);
  font-size: var(--text-meta);
  font-weight: var(--wght-strong);
}
.session-peek-ask-body {
  color: var(--fg);
  font-size: var(--text-ui);
  line-height: var(--lh-ui);
  overflow-wrap: anywhere;
}

.session-peek-hint {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--fg-dim);
  font-size: var(--text-meta);
}
.session-peek-hint-spacer { flex: 1 1 auto; }
.session-peek-key { font-family: var(--font-mono); font-size: var(--text-meta); }
`;

export function SessionPeek(props: SessionPeekProps) {
  const { diff, facts, worktree } = props;
  const pullRequest = props.pullRequest ?? "";
  const waiting = props.badge === "waiting";
  const tone = waiting ? " is-waiting" : props.badge === "failed" ? " is-failed" : "";
  // "waiting 3m" says the one thing a blocked agent's age means. Every other
  // state needs its word beside the number for the age to mean anything.
  const when = props.age === ""
    ? props.status
    : waiting ? `waiting ${props.age}` : `${props.status} · ${props.age}`;
  const files = facts.files === undefined || facts.files === 0
    ? "" : `${facts.files} file${facts.files === 1 ? "" : "s"}`;
  return (
    <div ref={props.cardRef} id={props.id} className="session-peek is-floating" role="tooltip"
      style={props.box === null
        ? ({ visibility: "hidden", top: 0, left: 0 } satisfies CSSProperties)
        : ({ top: props.box.top, left: props.box.left } satisfies CSSProperties)}>
      <div className="session-peek-head">
        <span className={props.badge === "working"
          ? "dot dot-working dot-pulse" : `dot dot-${props.badge}`} aria-hidden="true" />
        <span className="session-peek-title">{props.title}</span>
        <span className={`session-peek-age${tone}`}>{when}</span>
      </div>

      {(facts.activity !== undefined || diff !== null || worktree !== "" || pullRequest !== "") && (
        <dl className="session-peek-facts">
          {facts.activity !== undefined && facts.activity !== "" && (
            <>
              <dt className="session-peek-label">Last</dt>
              <dd className="session-peek-value">{facts.activity}</dd>
            </>
          )}
          {diff !== null && (
            <>
              <dt className="session-peek-label">Changes</dt>
              <dd className="session-peek-value">
                <span className="session-peek-added">+{diff.added}</span>
                <span className="session-peek-removed">−{diff.removed}</span>
                {files !== "" && ` · ${files}`}
              </dd>
            </>
          )}
          {worktree !== "" && (
            // "Worktree", not "Branch". What the renderer can name truthfully
            // is the managed checkout — its directory is the branch's stem, but
            // it is not the ref, and a card that says "Branch" over a stem is
            // making a claim the app cannot back. See managedWorktreeName.
            <>
              <dt className="session-peek-label">Worktree</dt>
              <dd className="session-peek-value session-peek-worktree">{worktree}</dd>
            </>
          )}
          {pullRequest !== "" && (
            <>
              <dt className="session-peek-label">PR</dt>
              <dd className="session-peek-value">{pullRequest}</dd>
            </>
          )}
        </dl>
      )}

      {waiting && facts.approval !== undefined && facts.approval !== "" && (
        <div className="session-peek-ask">
          <span className="session-peek-ask-title">Waiting on you</span>
          <span className="session-peek-ask-body">{facts.approval}</span>
        </div>
      )}

      <div className="session-peek-hint">
        <span>Click opens the session</span>
        <span className="session-peek-hint-spacer" />
        <span className="session-peek-key">Space</span>
      </div>
    </div>
  );
}
