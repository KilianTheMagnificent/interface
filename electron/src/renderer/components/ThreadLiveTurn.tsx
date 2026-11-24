import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactElement, ReactNode } from "react";
import type { ConvItem } from "../conversation-reducer";
import { equalValue } from "../stable-values";
import { threadElapsedLabel } from "../thread-inbox";
import { THREAD_THINKING } from "../thread-live";
import {
  THREAD_WAITING_LINE, threadLiveAnnouncement, threadStepLabel, type ThreadLiveHeader,
  type ThreadLiveTurnModel, type ThreadTrailRow, type ThreadTrailStep,
} from "../thread-live-turn";
import type { TranscriptDensity } from "../transcript-density";
import {
  SearchableItem, type ConversationItemContext, type FindRenderContext,
} from "./ConversationRows";
import { DONE_WORD } from "./status-words";
import { threadStepGlyph } from "./thread-look";
import { ThinkingMark } from "./ThinkingMark";
import { ThreadGlyph } from "./ThreadGlyph";

/**
 * A RUNNING THREADS TURN, DRAWN UNDER ITS PROMPT (owner, 2026-10-07, "a live
 * turn"; restyled 2026-10-23, the Threads look): a railed trail of the turn's
 * work with its prose interleaved — each step wearing its tool's glyph on the
 * rail and its state on the right — and under it the status line, which says
 * what the agent is doing now and for how long ("Thinking" before anything
 * shows). thread-live-turn.ts decides every word; this draws them, opens a
 * step's own detail in place, and plays the finish.
 *
 * The clock is the only once-a-second state, in its own leaf, so a tick
 * redraws two short text nodes and never the trail. The status line's live
 * region hears state changes only — working, waiting for your approval,
 * finished — never a clock tick or a new step.
 */

/** The finish, as the owner approved it: when each beat begins, from the moment the turn ended. */
export const THREAD_FINISH_MS = { collapse: 160, arrive: 500, settle: 900 } as const;
/** How long a fold takes to open or close. */
export const THREAD_FOLD_MS = 340;

export type ThreadLivePhase = "live" | "done" | "collapse";

/**
 * "0:34", ticking each second, or frozen at `until`. Nothing where the start
 * is unknown. Waiting, it is the time since the request arrived, beside the
 * words that already say the run waits on you.
 */
export function ThreadLiveClock({ from, until, tone }: {
  from?: number;
  until?: number;
  tone?: "waiting" | "done";
}): ReactElement | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (until !== undefined || from === undefined) return;
    const tick = (): void => { setNow(Date.now()); };
    tick();
    const id = window.setInterval(tick, 1_000);
    return () => { window.clearInterval(id); };
  }, [from, until]);
  if (from === undefined) return null;
  return (
    <span className={tone === undefined ? "thread-live-clock" : `thread-live-clock is-${tone}`}
      aria-hidden="true">
      {threadElapsedLabel(from, until ?? now)}
    </span>
  );
}

/** A steer's return arrow on the rail: the Threads set has no such glyph. */
function SteerGlyph(): ReactElement {
  return (
    <svg className="thread-steer-glyph" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"
      focusable="false">
      <path d="M12.5 3.5v4a2 2 0 0 1-2 2h-7m3-3-3 3 3 3" fill="none" stroke="currentColor"
        strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** The node on the rail: the step's tool glyph, masking the rail behind it. */
function StepNode({ step, pop }: { step: ThreadTrailStep; pop: boolean }): ReactElement {
  return (
    <span className={`thread-step-node is-${step.state}${pop ? " is-pop" : ""}`} aria-hidden="true">
      <ThreadGlyph name={threadStepGlyph(step.item)} />
    </span>
  );
}

/**
 * The step's state, on the right of its row: a turning ring while it runs, a
 * still amber ring and its note while it waits on you, a failure's note in the
 * failed colour, and a finished call's lines (only once it has a result —
 * unknown is never zero) or its short note.
 */
function StepEnd({ step }: { step: ThreadTrailStep }): ReactElement | null {
  const ring = step.state === "running" || step.state === "waiting";
  const stat = step.stat !== null && (step.stat.added > 0 || step.stat.removed > 0) ? step.stat : null;
  if (!ring && step.meta === "" && stat === null) return null;
  return (
    <span className="thread-step-end">
      {ring && <span className={`thread-step-ring is-${step.state}`} aria-hidden="true" />}
      {step.meta !== "" && <span className="thread-step-meta">{step.meta}</span>}
      {stat !== null && (
        <span className="thread-stat thread-step-stat" aria-hidden="true">
          <span className="conv-stat-add">+{stat.added}</span>
          <span className="conv-stat-del">−{stat.removed}</span>
        </span>
      )}
    </span>
  );
}

/** What an opened step shows: the transcript's own row for the call, subagent or change. */
function stepDetailItem(step: ThreadTrailStep): ConvItem {
  // A waiting approval with no call of its own shows what it asks to run as a
  // call would: never the interactive card, which the composer docks.
  if (step.item.kind === "approval" && step.state === "waiting") {
    return { kind: "tool", id: step.item.id, tool: step.item.tool, summary: step.item.summary,
      input: step.item.input };
  }
  return step.item;
}

const NO_FIND_REGISTER = (): void => undefined;

/** One step: a button that opens its detail in place, and a running edit's peek. */
const ThreadStepRow = memo(function ThreadStepRow({ step, open, fresh, pop, onToggle, all, find }: {
  step: ThreadTrailStep;
  open: boolean;
  fresh: boolean;
  pop: boolean;
  onToggle: (key: string) => void;
  all: ConversationItemContext;
  find: FindRenderContext;
}): ReactElement {
  // The opened detail is the transcript's own row, at its fullest, and is
  // found through this row: it registers nothing of its own.
  const detailFind = useMemo(() => ({ ...find, register: NO_FIND_REGISTER }), [find]);
  const hit = find.active && step.keys.some(key => find.matches.has(key));
  const current = hit && find.currentKey !== null && step.keys.includes(find.currentKey);
  // The find hit being shown opens its step, as it opens a folded card.
  const shown = open || current;
  const words = step.words;
  return (
    <div role="listitem"
      ref={node => { for (const key of step.keys) find.register(key, node); }}
      className={`thread-step is-${step.state}${fresh ? " is-new" : ""}${
        current ? " conv-find-current" : hit ? " conv-find-hit" : ""}`}
      data-transcript-match={hit ? "true" : undefined}>
      <button type="button" className="thread-step-row" aria-expanded={shown}
        aria-label={threadStepLabel(step)} title={step.title}
        onClick={() => { onToggle(step.key); }}>
        <StepNode step={step} pop={pop} />
        <span className="thread-step-words">
          {step.prefix !== "" && <span className="thread-step-prefix">{step.prefix}</span>}
          <span className="thread-step-verb">{words.verb}</span>
          {words.target !== "" && (
            <span className={words.mono ? "thread-step-target is-mono" : "thread-step-target"}>
              {words.target}
            </span>
          )}
        </span>
        <StepEnd step={step} />
      </button>
      {step.peek.length > 0 && (
        <div className="thread-peek" aria-hidden="true">
          {step.peek.map((line, index) => (
            <div key={index} className="thread-peek-line">{line}</div>
          ))}
        </div>
      )}
      {shown && (
        <div className="thread-step-detail">
          <SearchableItem item={stepDetailItem(step)} all={all} findKey={step.key}
            find={detailFind} density="detailed" densityEpoch={0} />
        </div>
      )}
    </div>
  );
}, (previous, next) => previous.open === next.open && previous.fresh === next.fresh
  && previous.pop === next.pop && previous.onToggle === next.onToggle && previous.all === next.all
  && previous.find === next.find && equalValue(previous.step, next.step));

function TrailRows({ rows, open, fresh, pop, onToggle, all, find }: {
  rows: readonly ThreadTrailRow[];
  open: ReadonlySet<string>;
  fresh: (key: string) => boolean;
  pop: string | null;
  onToggle: (key: string) => void;
  all: ConversationItemContext;
  find: FindRenderContext;
}): ReactElement {
  return (
    <div className="thread-trail" role="list">
      {rows.map(row => row.kind === "steer" ? (
        <div key={row.key} role="listitem"
          ref={node => { find.register(row.key, node); }}
          className={fresh(row.key) ? "thread-steer is-new" : "thread-steer"}>
          <span className="thread-step-node is-steer" aria-hidden="true"><SteerGlyph /></span>
          <span className="thread-steer-label">You steered</span>
          <span className="thread-steer-text" title={row.text}>{row.text}</span>
        </div>
      ) : (
        <ThreadStepRow key={row.key} step={row} open={open.has(row.key)} fresh={fresh(row.key)}
          pop={pop === row.key} onToggle={onToggle} all={all} find={find} />
      ))}
    </div>
  );
}

/** Which steps are opened, and the one stable callback that opens or folds one. */
function useOpenSteps(): [ReadonlySet<string>, (key: string) => void] {
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const onToggle = useCallback((key: string): void => {
    setOpen(current => {
      const next = new Set(current);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }, []);
  return [open, onToggle];
}

const NEVER_FRESH = (): boolean => false;

/** Steps drawn on their own, outside a live turn: an approval still waiting in a finished turn. */
export function ThreadStepList({ rows, all, find }: {
  rows: readonly ThreadTrailRow[];
  all: ConversationItemContext;
  find: FindRenderContext;
}): ReactElement {
  const [open, onToggle] = useOpenSteps();
  return <TrailRows rows={rows} open={open} fresh={NEVER_FRESH} pop={null} onToggle={onToggle}
    all={all} find={find} />;
}

/** The status line's words: the step's verb, its target in mono; or Thinking, Waiting, Done. */
function StatusWords({ header, done }: { header: ThreadLiveHeader; done: boolean }): ReactElement {
  if (done) return <span className="thread-live-verb">{DONE_WORD}</span>;
  if (header.state === "waiting") {
    return <span className="thread-live-verb is-waiting">{THREAD_WAITING_LINE}</span>;
  }
  if (header.words === null) return <span className="thread-live-verb">{THREAD_THINKING}</span>;
  return (
    <>
      <span className="thread-live-verb">{header.words.verb}</span>
      {header.words.target !== "" && (
        <span className={header.words.mono ? "thread-live-target is-mono" : "thread-live-target"}>
          {header.words.target}
        </span>
      )}
    </>
  );
}

export interface ThreadLiveTurnProps {
  model: ThreadLiveTurnModel;
  phase: ThreadLivePhase;
  /** When the run began; the clock prints nothing where nobody knows. */
  startedAt?: number;
  /** When it ended, for the finish's frozen clock. */
  endedAt?: number;
  /** The work line the trail and its status line collapse into at the finish. */
  fold?: ReactNode;
  all: ConversationItemContext;
  find: FindRenderContext;
  density: TranscriptDensity;
  densityEpoch: number;
}

/**
 * The live turn. It stays mounted from the turn's first frame through the
 * finish, so it knows which rows were there when it appeared (those do not
 * rise again), which step was running last (the finish pops its glyph) and
 * when the waiting began (a request it saw arrive is timed from then).
 */
export function ThreadLiveTurn({ model, phase, startedAt, endedAt, fold, all, find, density,
  densityEpoch }: ThreadLiveTurnProps): ReactElement {
  const [initial] = useState(() => new Set(model.segments.flatMap(segment =>
    segment.kind === "trail" ? segment.rows.map(row => row.key) : [segment.key])));
  const fresh = useCallback((key: string): boolean => !initial.has(key), [initial]);
  const [open, onToggle] = useOpenSteps();
  // The last step seen running and the last start seen, kept across the finish.
  const lastRunning = useRef<string | null>(null);
  const lastStart = useRef<number | undefined>(startedAt);
  useEffect(() => {
    if (phase === "live" && model.runningKey !== null) lastRunning.current = model.runningKey;
    if (startedAt !== undefined) lastStart.current = startedAt;
  });
  const from = startedAt ?? lastStart.current;
  // A request this turn saw arrive is timed from then; one already waiting at
  // mount is timed with the run, since nobody here knows when it came.
  const [waitSince, setWaitSince] = useState<{ id: string; at?: number } | null>(
    () => model.waitingId === null ? null : { id: model.waitingId });
  let waiting = waitSince;
  if (model.waitingId !== (waitSince?.id ?? null)) {
    waiting = model.waitingId === null ? null : { id: model.waitingId, at: Date.now() };
    setWaitSince(waiting);
  }
  const done = phase !== "live";
  const header = model.header;
  const state = done ? "done" : header.state;
  const pop = phase === "done" ? lastRunning.current : null;
  // The work line arrives above while the trail and its status line collapse into it.
  const folding = phase === "collapse" && fold !== null && fold !== undefined;
  return (
    <div className={`thread-live${done ? " is-done" : ""}${phase === "collapse" ? " is-collapsing" : ""}`}>
      {folding && fold}
      <div className="thread-live-body">
        <div className="thread-live-inner">
          {model.segments.map(segment => segment.kind === "trail" ? (
            <TrailRows key={segment.key} rows={segment.rows} open={open} fresh={fresh} pop={pop}
              onToggle={onToggle} all={all} find={find} />
          ) : segment.row.kind === "mark" ? (
            <div key={segment.key} className="conv-mark">{segment.row.label ?? "New messages"}</div>
          ) : (
            <div key={segment.key} className={fresh(segment.key) ? "thread-live-row is-new" : "thread-live-row"}>
              <SearchableItem item={segment.row.item} all={all} findKey={segment.key} find={find}
                density={density} densityEpoch={densityEpoch} />
            </div>
          ))}
          {/* Under the trail: the mark, what the agent is doing now, and for
              how long. Thinking until anything shows; at the finish, Done. */}
          <div className={`thread-live-status is-${state}`}>
            <span className="thread-live-mark" aria-hidden="true">
              {state === "waiting"
                ? <span className="thread-live-dot" />
                : <ThinkingMark className="thread-live-pair" />}
              {done && <ThreadGlyph name="pass" className="thread-live-check" />}
            </span>
            <span className="thread-live-words" aria-hidden="true">
              <StatusWords header={header} done={done} />
            </span>
            {state === "waiting"
              ? <ThreadLiveClock from={waiting?.at ?? from} tone="waiting" />
              : <ThreadLiveClock from={from} until={done ? endedAt ?? Date.now() : undefined}
                  tone={done ? "done" : undefined} />}
          </div>
        </div>
      </div>
      {/* State changes only: working, waiting for your approval, finished. */}
      <span className="conv-sr" role="status" aria-live="polite" aria-atomic="true">
        {threadLiveAnnouncement(state)}
      </span>
    </div>
  );
}

/**
 * What the jump capsule says while a turn is live: the thinking mark (an amber
 * dot while the run waits on you), the step's short line, the clock, the way down.
 */
export function ThreadLivePill({ header, startedAt }: {
  header: ThreadLiveHeader;
  startedAt?: number;
}): ReactElement {
  const waiting = header.state === "waiting";
  return (
    <>
      {waiting
        ? <span className="thread-live-dot" aria-hidden="true" />
        : <ThinkingMark size={14} className="thread-live-pair" />}
      <span className={waiting ? "conv-jump-full thread-pill-line is-waiting"
        : "conv-jump-full thread-pill-line"}>{header.short}</span>
      <ThreadLiveClock from={startedAt} tone={waiting ? "waiting" : undefined} />
      <ThreadGlyph name="down" size={14} />
    </>
  );
}

/** One beat of a watched turn's finish, and when it began. */
export interface ThreadFinishBeat {
  key: string;
  phase: "done" | "collapse" | "arrive";
  at: number;
}

/**
 * THE FINISH (owner, 2026-10-07), for a turn this pane instance watched go
 * from running to finished — never one already finished when it mounted.
 * `done` (the last step's glyph pops and the status line turns to a check),
 * then `collapse` (the trail folds into the work line), then `arrive` (the
 * changes card and the turn's controls come in), then nothing: the turn draws
 * as every finished turn does. Detected during render, so the frame that ends
 * the turn already draws its first beat; each beat's timer is cleared on
 * unmount. A turn plays its finish once, however its live state flickers.
 * `motion` false (reduced motion, or a layout with no live turn) skips
 * straight to the end.
 */
export function useThreadFinishBeat(liveKey: string | null, scope: string | undefined, motion: boolean,
    collapse: boolean): ThreadFinishBeat | null {
  const [seen, setSeen] = useState({ liveKey, scope });
  const [beat, setBeat] = useState<ThreadFinishBeat | null>(null);
  const played = useRef(new Set<string>());
  let current = beat;
  if (seen.liveKey !== liveKey || seen.scope !== scope) {
    const ended = seen.scope === scope && seen.liveKey !== null && liveKey === null;
    current = ended && motion && !played.current.has(seen.liveKey!)
      ? { key: seen.liveKey!, phase: "done", at: Date.now() } : null;
    setSeen({ liveKey, scope });
    setBeat(current);
  }
  const key = current?.key;
  const phase = current?.phase;
  const at = current?.at;
  useEffect(() => {
    if (key === undefined || phase === undefined || at === undefined) return;
    played.current.add(key);
    const next = phase === "done" ? (collapse ? "collapse" : "arrive")
      : phase === "collapse" ? "arrive" : null;
    const due = phase === "done" ? THREAD_FINISH_MS.collapse
      : phase === "collapse" ? THREAD_FINISH_MS.arrive : THREAD_FINISH_MS.settle;
    const id = window.setTimeout(() => {
      setBeat(previous => previous === null || previous.key !== key || previous.phase !== phase
        ? previous : next === null ? null : { ...previous, phase: next });
    }, Math.max(0, at + due - Date.now()));
    return () => { window.clearTimeout(id); };
  }, [key, phase, at, collapse]);
  return current;
}

/** A fold opening or closing: which turn's, and which way. */
export interface ThreadFoldMotion { key: string; dir: "open" | "close" }

/**
 * The work line's expand and collapse. Opening shows the rows at once and
 * plays them in; closing plays them out and then folds them (`settle`). A
 * second press mid-way settles the first before starting. The timer is
 * cleared on unmount.
 */
export function useThreadFoldMotion(): [ThreadFoldMotion | null,
    (next: ThreadFoldMotion, settle: () => void) => void] {
  const [motion, setMotion] = useState<ThreadFoldMotion | null>(null);
  const timer = useRef<number | null>(null);
  const pending = useRef<(() => void) | null>(null);
  useEffect(() => () => {
    if (timer.current !== null) window.clearTimeout(timer.current);
  }, []);
  const start = useCallback((next: ThreadFoldMotion, settle: () => void): void => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      pending.current?.();
    }
    pending.current = settle;
    setMotion(next);
    timer.current = window.setTimeout(() => {
      timer.current = null;
      const finish = pending.current;
      pending.current = null;
      finish?.();
      setMotion(null);
    }, THREAD_FOLD_MS);
  }, []);
  return [motion, start];
}
