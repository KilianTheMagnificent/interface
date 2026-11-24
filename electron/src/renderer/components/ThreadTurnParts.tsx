import { memo, useEffect, useRef, useState } from "react";
import type { ReactElement } from "react";
import { formatDiffStat, type DiffStat } from "../diff-stats";
import { isPreviewableOutput } from "../output-preview";
import { equalValue } from "../stable-values";
import type { ShipNext, ThreadShipCardView } from "../thread-ship";
import {
  threadChangesLabel, threadClockText, threadStampParts, threadWorkChips, threadWorkSentence, threadWorkSummary,
  type ThreadChangesRow, type ThreadChangesView, type ThreadWorkCounts,
} from "../thread-transcript";
import { DiffstatBar, FileBadge } from "./ProjectHueParts";
import { threadPathParts } from "./thread-look";
import { ThreadGlyph } from "./ThreadGlyph";

/**
 * The pieces a Threads pane draws around a turn (owner, 2026-10-06 redesign;
 * restyled 2026-10-23, the Threads look): the quiet line under a prompt, the
 * work line a finished turn folds into, the card of files it changed, and the
 * turn's controls. Every decision is made in thread-transcript.ts and by the
 * transcript that places these; each piece is a memo boundary taking values,
 * so a streamed token redraws none of them.
 */

/** "+12 −4" in the diff colours, or nothing at all where no diff counted the lines. */
function StatText({ stat, bar }: { stat: DiffStat | null; bar?: boolean }): ReactElement | null {
  if (formatDiffStat(stat) === "" || stat === null) return null;
  return (
    <>
      <span className="thread-stat">
        <span className="conv-stat-add">+{stat.added}</span>
        <span className="conv-stat-del">−{stat.removed}</span>
      </span>
      {/* The five squares say the same split at a glance; the words above are
          what a screen reader hears. */}
      {bar === true && <DiffstatBar added={stat.added} removed={stat.removed} />}
    </>
  );
}

/**
 * Copy, as an icon: confirmed for a moment by a check in its place and on its
 * label, as the code blocks' button confirms on its words.
 */
function CopyButton({ text, label, size }: { text: string; label: string; size: number }): ReactElement {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => { setCopied(false); }, 1_500);
    return () => { window.clearTimeout(id); };
  }, [copied]);
  return (
    <button type="button" className="thread-act thread-copy" aria-label={copied ? "Copied" : label}
      title={label}
      onClick={() => {
        // A context with no clipboard, or one that refuses, leaves the label
        // alone: nothing was copied, and the button says nothing that it was.
        navigator.clipboard?.writeText?.(text)?.then(() => { setCopied(true); }, () => undefined);
      }}>
      <ThreadGlyph name={copied ? "pass" : "copy"} size={size} />
    </button>
  );
}

/** The stamp a row prints, or nothing where Rust recorded none. */
function Clock({ at }: { at?: number }): ReactElement | null {
  if (at === undefined || at <= 0) return null;
  return (
    <time className="thread-time" dateTime={new Date(at).toISOString()}
      title={new Date(at).toLocaleString()}>
      {threadClockText(at)}
    </time>
  );
}

/**
 * Centred over a prompt, as TN Code stamps its messages: the day in a word,
 * then the time. Drawn only over the prompts `threadStampKeys` picks.
 */
export const ThreadStamp = memo(function ThreadStamp({ at }: { at: number }): ReactElement {
  const { day, time } = threadStampParts(at, Date.now());
  return (
    <time className="thread-stamp" dateTime={new Date(at).toISOString()}
      title={new Date(at).toLocaleString()}>
      <b>{day}</b> {time}
    </time>
  );
});

/**
 * Under a prompt, shown on hover or focus without moving anything: when it was
 * sent, Copy, and Edit (from here) where the row is a Rust turn that allows
 * it. Edit opens the pane's one ThreadEditDialog, unchanged; its name and
 * title still say that it starts a new thread from here.
 */
export const ThreadUserMeta = memo(function ThreadUserMeta({ turnKey, text, sentAt, edit, busy,
  onEdit }: {
  turnKey: string;
  text: string;
  sentAt?: number;
  edit: boolean;
  /** A Fork or an Edit from this pane is on its way. */
  busy: boolean;
  onEdit: (turnKey: string) => void;
}): ReactElement {
  return (
    <div className="thread-user-meta">
      <Clock at={sentAt} />
      <CopyButton text={text} label="Copy message" size={14} />
      {edit && (
        <button type="button" className="thread-act thread-edit"
          aria-label="Edit from this message: start a new thread from here"
          title="Edit from here" disabled={busy}
          onClick={() => { onEdit(turnKey); }}>
          Edit
        </button>
      )}
    </div>
  );
});

/**
 * A finished turn's work, folded into one line: "› Worked for 41s", then one
 * quiet chip per kind of work ("read 3", "edited 2", "ran 1"), all in the same
 * neutral tally. The line's name stays the whole sentence, so a screen reader
 * hears "Worked for 41s · read 3 files, edited 2 files" and a hover over the
 * chips shows it. It opens the work in place and folds it again. The rows it
 * stands for are siblings in the transcript, not children of this button, so
 * the state lives with the transcript and this only reports the press.
 */
export const ThreadWorkFold = memo(function ThreadWorkFold({ turnKey, lead, work, open,
  onToggle, arriving }: {
  turnKey: string;
  lead: string;
  work: ThreadWorkCounts;
  open: boolean;
  onToggle: (turnKey: string) => void;
  /** A live turn is collapsing into the line at its finish: it fades in above the trail. */
  arriving?: boolean;
}): ReactElement {
  const chips = threadWorkChips(work);
  return (
    <button type="button"
      className={`thread-fold${open ? " is-open" : ""}${arriving === true ? " is-arriving" : ""}`}
      aria-expanded={open} title={open ? "Hide the work" : "Show the work"}
      aria-label={threadWorkSentence(lead, work)}
      onClick={() => { onToggle(turnKey); }}>
      <ThreadGlyph name="expand" size={14}
        className={open ? "thread-fold-chev is-open" : "thread-fold-chev"} />
      <span className="thread-fold-lead">{lead}</span>
      {chips.length > 0 && (
        <span className="thread-fold-chips" title={threadWorkSummary(work)}>
          {chips.map(chip => (
            <span key={chip.kind} className={`thread-work-chip is-${chip.kind}`}>{chip.label}</span>
          ))}
        </span>
      )}
    </button>
  );
}, (previous, next) => previous.turnKey === next.turnKey && previous.lead === next.lead
  && previous.open === next.open && previous.onToggle === next.onToggle
  && previous.arriving === next.arriving && equalValue(previous.work, next.work));

/**
 * What a finished turn changed: a hairline card with the count, the lines and
 * their five squares, up to six files (each led by its type's badge, its
 * directory dimmed) and "+ N more", and Review, a quiet bordered button that
 * opens that turn in the diff panel. Review is drawn only where the turn's
 * diff can be shown.
 */
export const ThreadChangesCard = memo(function ThreadChangesCard({ view, ordinal, onReview, arriving,
  ship, onShip, onPreview }: {
  view: ThreadChangesView;
  /** The turn whose diff Review opens; absent, there is no Review. */
  ordinal?: number;
  onReview: (ordinal: number) => void;
  /** A turn this pane watched finish: the card rises in and its rows follow. */
  arriving?: boolean;
  /** The latest card's ship row (protocol 37); absent on every other card. */
  ship?: ThreadShipCardView;
  /** The ship row's next step, or "read" when the row first shows. */
  onShip?: (step: ShipNext | "read") => void;
  /** Preview a PDF, image or Markdown file beside the thread (review B5). */
  onPreview?: (path: string) => void;
}): ReactElement {
  const label = threadChangesLabel(view);
  return (
    <div className={arriving === true ? "thread-changes is-arriving" : "thread-changes"}
      role="group" aria-label={label}>
      <div className="thread-changes-head">
        <ThreadGlyph name="changes" size={15} className="thread-changes-glyph" />
        <span className="thread-changes-count">
          {view.files} {view.files === 1 ? "file" : "files"} changed
        </span>
        <StatText stat={view.stat} bar />
        {ordinal !== undefined && (
          <button type="button" className="thread-review"
            aria-label={`Review the changes from this turn: ${label}`}
            onClick={() => { onReview(ordinal); }}>
            Review
          </button>
        )}
      </div>
      {view.rows.length > 0 && (
        <ul className="thread-changes-files">
          {view.rows.map(row => (
            <li key={row.path} className="thread-changes-file" title={row.path}>
              {/* The file's type, in its language's colour. What git would
                  call the change is said; a new or deleted file also says so
                  after its name, quietly and only to the eye. */}
              <FileBadge path={row.path} />
              {row.letter !== "" && (
                <span className="conv-sr">
                  {row.verb === "add" ? "added" : row.verb === "delete" ? "deleted" : "modified"}
                </span>
              )}
              <FilePath shown={row.shown} verb={row.verb} />
              <StatText stat={row.stat} />
              {onPreview !== undefined && row.verb !== "delete" && isPreviewableOutput(row.path) && (
                <button type="button" className="thread-act thread-file-preview"
                  aria-label={`Preview ${row.shown}`} title="Preview beside the thread"
                  onClick={() => { onPreview(row.path); }}>
                  <ThreadGlyph name="preview" size={14} />
                  Preview
                </button>
              )}
            </li>
          ))}
          {view.more > 0 && (
            <li className="thread-changes-file thread-changes-more">+ {view.more} more</li>
          )}
        </ul>
      )}
      {onShip !== undefined && <ThreadShipRow view={ship} onShip={onShip} />}
    </div>
  );
}, (previous, next) => previous.onReview === next.onReview && previous.ordinal === next.ordinal
  && previous.arriving === next.arriving && previous.onShip === next.onShip
  && previous.onPreview === next.onPreview
  && equalValue(previous.ship, next.ship) && equalValue(previous.view, next.view));

/** A changed file's shown path: its directory dimmed, its name in the text colour. */
function FilePath({ shown, verb }: Pick<ThreadChangesRow, "shown" | "verb">): ReactElement {
  const { dir, name } = threadPathParts(shown);
  return (
    <span className="thread-file-path">
      {dir !== "" && <span className="thread-file-dir">{dir}</span>}
      <span className="thread-file-name">{name}</span>
      {(verb === "add" || verb === "delete") && (
        <span className="thread-file-note" aria-hidden="true">
          {verb === "add" ? " · new" : " · deleted"}
        </span>
      )}
    </span>
  );
}

/**
 * Shipping under the latest turn's changes (protocol 37): Rust's last read in
 * a few words ("2 files to commit", "#42 · 1 failing") beside the branch
 * glyph, and the one next step as the card's one filled button — Commit…,
 * Create branch… and Merge… open the Changes panel, where the message, the
 * branch name and the confirmation are; Push & open PR, Fix failing checks
 * and Check again run from here. Asks for a read when it first shows; draws
 * nothing until one has answered.
 */
function ThreadShipRow({ view, onShip }: { view: ThreadShipCardView | undefined;
    onShip: (step: ShipNext | "read") => void }): ReactElement | null {
  const ask = useRef(onShip);
  ask.current = onShip;
  useEffect(() => { ask.current("read"); }, []);
  if (view === undefined) return null;
  return (
    <div className="thread-ship" role="group" aria-label="Ship this thread's work">
      <ThreadGlyph name="branch" size={13} className="thread-changes-glyph" />
      <span className="thread-ship-text">{view.text}</span>
      {view.label !== "" && (
        <button type="button" className="thread-ship-act" disabled={view.busy}
          onClick={() => { onShip(view.next); }}>
          {view.label}
        </button>
      )}
    </div>
  );
}

/**
 * The turn's controls, drawn once: always under the latest finished turn, and
 * under an earlier one only while its answer is hovered or holds focus. Icons
 * — Copy, Continue (a failed turn's retry), Fork from here, Undo turn — each
 * named and titled, then the time the turn ended. Continue and Undo add their
 * words while they work. Fork from here keeps the pane's one-at-a-time lock;
 * Undo turn is the pane's restore control and asks first, as it always did.
 */
export const ThreadTurnFoot = memo(function ThreadTurnFoot({ turnKey, latest, endedAt, answer,
  fork, busy, undo, restoring, restoreBusy, onFork, onUndo, retry, onRetry, arriving }: {
  turnKey: string;
  latest: boolean;
  /** A turn this pane watched finish: the controls fade in last. */
  arriving?: boolean;
  endedAt?: number;
  /** The final answer's text, for Copy; "" draws no Copy. */
  answer: string;
  /** The Rust turn's ordinal Fork keeps through; absent draws no Fork. */
  fork?: number;
  /** A Fork or an Edit from this pane is on its way. */
  busy: boolean;
  /** The restore point's own label (its title); absent draws no Undo. */
  undo?: string;
  restoring: boolean;
  /** A restore is running anywhere in this pane. */
  restoreBusy: boolean;
  onFork: (ordinal: number) => void;
  onUndo: (turnKey: string) => void;
  /**
   * Retry this failed turn: "ready" draws it, "sending" draws it held while
   * the send is on its way; absent draws none (`threadRetryKey`).
   */
  retry?: "ready" | "sending";
  onRetry?: (turnKey: string) => void;
}): ReactElement | null {
  if (endedAt === undefined && answer === "" && fork === undefined && undo === undefined
    && retry === undefined) return null;
  return (
    <div className={`thread-turn-foot${latest ? " is-latest" : ""}${arriving === true ? " is-arriving" : ""}`}>
      {answer !== "" && <CopyButton text={answer} label="Copy answer" size={15} />}
      {retry !== undefined && onRetry !== undefined && (
        <button type="button" className="thread-act thread-retry"
          aria-label="Continue: inspect completed work before resuming"
          title="Continue from the current state with preserved context"
          disabled={retry === "sending" || busy || restoreBusy}
          onClick={() => { onRetry(turnKey); }}>
          <ThreadGlyph name="retry" size={15} />
          {retry === "sending" && "Continuing…"}
        </button>
      )}
      {fork !== undefined && (
        <button type="button" className="thread-act thread-fork"
          aria-label={`Fork from here: a new thread with the first ${fork + 1} ${
            fork === 0 ? "message" : "messages"} of this one`}
          title="Fork from here" disabled={busy}
          onClick={() => { onFork(fork); }}>
          <ThreadGlyph name="fork" size={15} />
        </button>
      )}
      {undo !== undefined && (
        <button type="button" className="thread-act thread-undo"
          aria-label="Restore files to before this message" title={undo}
          disabled={restoreBusy}
          onClick={() => { onUndo(turnKey); }}>
          <ThreadGlyph name="undo" size={15} />
          {restoring && "Restoring…"}
        </button>
      )}
      <Clock at={endedAt} />
    </div>
  );
});
