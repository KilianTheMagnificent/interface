import type { ThreadSide } from "../../shared/threads";
import { Markdown } from "../markdown";
import { sideStatusText, type SideAnswer } from "../side-question";

/* ============================================================================
   THE SIDE QUESTION PANEL (protocol 67, T3 section 12) — the right column

   A side question asks a hidden fork of the thread something without adding
   to the thread. This column (the slot the diff and subagent panels use)
   shows the question, the answer as it streams (Markdown, as a reply is
   drawn), and two buttons: Bring back
   (the answer goes into the thread's draft as a quote) and Close. Both end
   the side question; Rust removes its side thread.

   PRESENTATIONAL. Props in, callbacks out: no `window.iface`, no store.
   ========================================================================= */

export const SIDE_QUESTION_CSS = `
.side-panel { display:flex; flex-direction:column; gap:12px; box-sizing:border-box; height:100%; min-width:0;
  padding:16px 20px; overflow:hidden; color:var(--fg); background:var(--surface);
  border-left:var(--border-w) solid var(--border); font-family:var(--font-ui); font-size:var(--text-body);
  line-height:var(--lh-ui); }
.side-panel-head { display:flex; align-items:center; justify-content:space-between; gap:8px; }
.side-panel-title { margin:0; color:var(--fg-strong); font-size:var(--text-body); font-weight:var(--wght-medium); }
.side-panel-question { margin:0; padding:8px 10px; color:var(--fg); background:var(--row-hover);
  border-radius:var(--radius-container); white-space:pre-wrap; overflow-wrap:anywhere; }
.side-panel-answer { flex:1 1 auto; min-height:0; margin:0; overflow:auto; overflow-wrap:anywhere; }
.side-panel-status { margin:0; color:var(--fg-dim); font-size:var(--text-ui); }
.side-panel-status.is-failed { color:var(--danger-fg); }
.side-panel-actions { display:flex; justify-content:flex-end; gap:8px; }
.side-panel-btn { height:30px; padding:0 12px; color:var(--fg); background:transparent; border:0;
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong); border-radius:var(--radius-container);
  font:inherit; font-weight:var(--wght-medium); cursor:pointer; }
.side-panel-btn.is-primary { color:var(--surface); background:var(--fg-strong); box-shadow:none; }
.side-panel-btn:disabled { color:var(--fg-faint); background:transparent;
  box-shadow:inset 0 0 0 var(--border-w) var(--border); cursor:default; }
.side-panel-btn:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
`;

export function SideQuestionPanel({ side, answer, closing, onBringBack, onClose }: {
  side: ThreadSide;
  answer: SideAnswer;
  /** Close (or Bring back) is on its way. */
  closing: boolean;
  onBringBack: () => void;
  onClose: () => void;
}) {
  const running = answer.state === "asking" || answer.state === "answering";
  return (
    <aside className="side-panel" aria-label="Side question" data-side-question={side.key}>
      <style>{SIDE_QUESTION_CSS}</style>
      <div className="side-panel-head">
        <h2 className="side-panel-title">Side question</h2>
      </div>
      <p className="side-panel-question">{side.question}</p>
      <p className={`side-panel-status${answer.state === "failed" ? " is-failed" : ""}`} role="status"
        aria-live="polite">{sideStatusText(answer)}</p>
      {/* Drawn as a reply is (the transcript's Markdown: no raw HTML);
          Bring back quotes the source text. */}
      <div className="side-panel-answer" aria-busy={running || undefined}>
        {answer.text !== "" && <Markdown text={answer.text} />}
      </div>
      <div className="side-panel-actions">
        <button type="button" className="side-panel-btn" disabled={closing} onClick={onClose}>Close</button>
        <button type="button" className="side-panel-btn is-primary"
          disabled={closing || running || answer.text === ""}
          title="Add the answer to this thread's draft as a quote, and close the side question"
          onClick={onBringBack}>Bring back</button>
      </div>
    </aside>
  );
}
