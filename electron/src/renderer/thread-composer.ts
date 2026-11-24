import type { ContextUsage } from "../shared/engine";
import { formatContextTokens } from "../shared/engine";
import { contextMeterDetails, contextMeterLabel } from "./conversation-reducer";

/**
 * THE THREADS COMPOSER's pure rules (owner-approved redesign, 2026-10-06): the
 * words in the field, the "+" menu's keyboard and the session-context readout
 * that menu carries. `components/ConversationComposer.tsx` draws them when a
 * pane is in the Threads grid; send, queue and steer are the composer's own
 * and are not decided here.
 */

/**
 * What the empty field says. Only words change, and only to what the field
 * really does (owner, 2026-10-07, "a live turn"): while a turn runs Enter
 * queues and Steer (its chip names ⌥↵) delivers into the running turn, so the
 * field offers steering only where Steer is offered — and names no key, the
 * chips beside it do. While a request waits on the owner in the dock above,
 * the field says so: answer it there (an empty field's ⌘↵ and ⌘⌫ do), or type
 * what to do instead. A locked composer promises nothing at all.
 */
export function threadComposerPlaceholder(state: {
  locked: boolean; queueing: boolean; steer: boolean; waiting?: boolean;
}): string {
  if (state.locked) return "Ask for a change or a follow-up";
  if (state.waiting === true) return "Answer above, or tell it what to do instead";
  if (!state.queueing) return "Ask for a change or a follow-up";
  return state.steer ? "Steer now, or queue a follow-up" : "Queue a follow-up";
}

/**
 * Whether the field's own ⌘↵ / ⌘⌫ answer the request docked above it (owner,
 * 2026-10-07): only while a request waits on the owner, only from an EMPTY
 * field — an empty draft has nothing to send or queue, while with text in it
 * ⌘↵ is Enter and queues or sends exactly as it always did — and never from a
 * locked field, which takes no keys at all.
 */
export function threadComposerAnswersApproval(state: {
  waiting: boolean; draft: string; locked: boolean;
}): boolean {
  return state.waiting && state.draft === "" && !state.locked;
}

/** What a key does inside the "+" menu. */
export type ThreadMenuKeyAction =
  | { type: "move"; index: number }
  /** Drill into the active row's own view ("Add context"). */
  | { type: "open" }
  /** Leave a drilled-in view for the row that opened it. */
  | { type: "back" }
  /** Escape at the top: close, and hand focus back to the "+" button. */
  | { type: "close" }
  /** Tab: close, and let focus travel on as it would have. */
  | { type: "leave" };

/**
 * Roving focus over every row, disabled ones included — a disabled row's
 * reason is only reachable if the row can be focused (the context picker's
 * rule). Movement wraps. Enter and Space are the row button's own click.
 */
export function threadMenuKeyAction(count: number, active: number, key: string,
    where: { nested: boolean; opens: boolean }): ThreadMenuKeyAction | null {
  if (key === "Escape") return where.nested ? { type: "back" } : { type: "close" };
  if (key === "Tab") return { type: "leave" };
  if (key === "ArrowLeft") return where.nested ? { type: "back" } : null;
  if (key === "ArrowRight") return where.opens ? { type: "open" } : null;
  if (count <= 0) return null;
  const at = active >= 0 && active < count ? active : 0;
  if (key === "ArrowDown") return { type: "move", index: (at + 1) % count };
  if (key === "ArrowUp") return { type: "move", index: (at - 1 + count) % count };
  if (key === "Home") return { type: "move", index: 0 };
  if (key === "End") return { type: "move", index: count - 1 };
  return null;
}

/** The "+" button's own arrows: open on the first row, or on the last. */
export function threadMenuTriggerKeyAction(key: string): "first" | "last" | null {
  if (key === "ArrowDown") return "first";
  if (key === "ArrowUp") return "last";
  return null;
}

export interface ThreadContextReadout {
  /** The meter's own word: "12%", "<1%" or "Compacting". */
  meta: string;
  /** At or past 80% and not compacting: the meter's warning tier. */
  high: boolean;
  /** Every figure the meter's details panel shows, in its order. */
  lines: readonly { label: string; value: string }[];
}

/**
 * The session-context meter as a menu row. It is the same reading the pane's
 * meter chip opens — the same figures in the same order, "Provider reported"
 * included — and it is absent exactly where that chip draws nothing.
 */
export function threadContextReadout(usage: ContextUsage | undefined,
    previous?: ContextUsage): ThreadContextReadout | null {
  if (usage === undefined) return null;
  const meta = contextMeterLabel(usage);
  if (meta === "") return null;
  const compacting = usage.compacting === true;
  const details = contextMeterDetails(usage, previous);
  const lines: { label: string; value: string }[] = [];
  if (compacting) lines.push({ label: "Now", value: "Compacting the session window" });
  else if (usage.used !== undefined && usage.size !== undefined) {
    lines.push({ label: "Used", value:
      `${formatContextTokens(usage.used)} / ${formatContextTokens(usage.size)}` });
  }
  if (details.windowRemaining !== undefined) {
    lines.push({ label: "Window remaining", value: details.windowRemaining });
  }
  if (details.reportChange !== undefined) {
    lines.push({ label: "Since prior report", value: details.reportChange });
  }
  for (const row of usage.categories ?? []) {
    lines.push({ label: row.name, value: formatContextTokens(row.tokens) });
  }
  if (details.autoCompact !== undefined) {
    lines.push({ label: "Auto-compact", value: details.autoCompact });
  }
  if (details.autoCompactThreshold !== undefined) {
    lines.push({ label: "Auto-compact threshold", value: details.autoCompactThreshold });
  }
  lines.push({ label: "Source", value: "Provider reported" });
  return { meta, high: !compacting && (usage.percent ?? 0) >= 80, lines };
}
