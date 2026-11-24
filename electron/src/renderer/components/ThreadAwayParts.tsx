import { memo } from "react";
import type { ReactElement } from "react";
import {
  awayReviewView, awayRoundLine, type AwayCardAction, type AwayReportView, type AwayTurnCard,
} from "../away-thread";
import { Icon } from "./glyphs";
import "./ThreadAway.css";

/**
 * What an Away loop's thread draws around its turns (review B3): a work
 * round's prompt as one line with its instructions folded, the fresh review
 * of that round under the turn's changes, and the loop's report at the end.
 * Values in, callbacks out; away-thread.ts made every decision.
 */

/** A work round's prompt: the round, with the loop's instructions folded. */
export const AwayRoundPrompt = memo(function AwayRoundPrompt({ card, text }: {
  card: Pick<AwayTurnCard, "iteration" | "maxIterations">;
  text: string;
}): ReactElement {
  return (
    <div className="taway-round">
      <Icon name="coffee" sm className="taway-glyph" />
      <span className="taway-round-line">{awayRoundLine(card)}</span>
      <details className="taway-instructions">
        <summary>Instructions</summary>
        <p>{text}</p>
      </details>
    </div>
  );
});

/**
 * A switch's handoff (protocol 59), drawn as an Away round's prompt is: one
 * line saying where the thread went ("Handed the thread to Codex"), with what
 * was sent folded under it. A prompt Interface sent on a schedule or for a
 * watched pull request (protocol 62) is drawn the same way, with its own
 * glyph ("Scheduled prompt · Nightly check").
 */
export const ThreadHandoffPrompt = memo(function ThreadHandoffPrompt({ line, text, icon = "arrow-swap" }: {
  line: string;
  text: string;
  icon?: string;
}): ReactElement {
  return (
    <div className="taway-round thread-handoff">
      <Icon name={icon} sm className="taway-glyph" />
      <span className="taway-round-line">{line}</span>
      <details className="taway-instructions">
        <summary>What was sent</summary>
        <p>{text}</p>
      </details>
    </div>
  );
});

/** The review of a work round, under its turn. */
export const AwayReviewCard = memo(function AwayReviewCard({ card, reviewer, onAction }: {
  card: AwayTurnCard;
  /** The provider whose session reviewed the round. */
  reviewer: string;
  onAction?: (action: AwayCardAction) => void;
}): ReactElement | null {
  if (card.review === undefined) return null;
  const view = awayReviewView(card.review, reviewer);
  const glyph = view.tone === "ok" ? "pass" : view.tone === "pending" ? "comment-discussion"
    : view.tone === "revise" ? "git-compare" : "error";
  return (
    <section className={`taway-card taway-review is-${view.tone}`}
      aria-label={`${view.title}: ${view.verdict}`}>
      <header className="taway-card-head">
        <Icon name={glyph} sm className="taway-glyph" />
        <span className="taway-card-title">{view.title}</span>
        <span className="taway-verdict">{view.verdict}</span>
        {view.sessionKey !== undefined && onAction !== undefined && (
          <button type="button" className="thread-act taway-open"
            onClick={() => { onAction({ kind: "session", key: view.sessionKey as string }); }}>
            Open review
          </button>
        )}
      </header>
      {view.summary !== "" && <p className="taway-summary">{view.summary}</p>}
      {view.checks.length > 0 && (
        <ul className="taway-checks">
          {view.checks.map((check, index) => <li key={index}>{check}</li>)}
        </ul>
      )}
      <p className="taway-note">{view.note}</p>
    </section>
  );
});

/** The loop's report at the end of its thread; its status while it runs. */
export const AwayReportCard = memo(function AwayReportCard({ report, onAction }: {
  report: AwayReportView;
  onAction?: (action: AwayCardAction) => void;
}): ReactElement {
  return (
    <section className={`taway-card taway-report is-${report.live ? "live" : report.state}`}
      aria-label={report.title} aria-live="polite">
      <header className="taway-card-head">
        <Icon name="coffee" sm className="taway-glyph" />
        <span className="taway-card-title">{report.title}</span>
        {onAction !== undefined && (report.live ? (
          <button type="button" className="thread-act taway-stop" disabled={report.state === "stopping"}
            onClick={() => { onAction({ kind: "stop", runId: report.runId }); }}>
            {report.state === "stopping" ? "Stopping…" : "Stop loop"}
          </button>
        ) : (
          <button type="button" className="thread-review taway-review-changes"
            onClick={() => { onAction({ kind: "review" }); }}>
            Review changes
          </button>
        ))}
      </header>
      <p className="taway-detail">{report.detail}</p>
      {report.verdict !== undefined && !report.live && (
        <div className="taway-final">
          <p><strong>{report.verdict.label}.</strong> {report.verdict.summary}</p>
          {report.verdict.checks.length > 0 && (
            <ul className="taway-checks">
              {report.verdict.checks.map((check, index) => <li key={index}>{check}</li>)}
            </ul>
          )}
        </div>
      )}
      <p className="taway-meta">{report.meta}</p>
      {!report.live && (
        <p className="taway-note">
          The work stays in this thread&apos;s worktree. Review it, then commit and open a pull request from Changes.
        </p>
      )}
    </section>
  );
});
