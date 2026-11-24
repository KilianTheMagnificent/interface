import type { TurnRestorePoint } from "./conversation-types";

/* ==========================================================================
   TURN-BOUND CHECKPOINTS — the shell's half of the transcript's Restore.

   Conversation owns no checkpoint machinery: it draws a control exactly where
   this map has an entry for a transcript row key, confirms the click, and
   hands the key back. Everything else is here and in App — creating the
   snapshot at dispatch, remembering which turn it stands before, and looking
   the id up again when the user says yes.

   TWO RULES, both about what may be OFFERED:

    1. NO ENTRY, NO CONTROL. An empty checkpoint (`fileCount === 0`) is never
       recorded, so a turn with nothing to roll back to keeps the face it had
       before this feature existed. A missing entry costs a button; a wrong
       one costs the user's work.
    2. ENTRIES DO NOT OUTLIVE THE RUN THAT WROTE THEM. The keys are
       mergeTranscript's own, and the key a send is first recorded against is
       its local echo — `e:0`, `e:1` — an id minted per app run and restarted
       at 0 by the next one. Every ACP engine streams the user's own turn back
       live, which settles that echo into an `l:`/`h:` row within a second, and
       rekeyTurnCheckpoint moves the entry with it (see settledEchoRowKey); the
       live list those keys come from is per-run too. So whichever key an entry
       ends up on, one left over from an EARLIER run names either no row at all
       or, worse, the next run's first send: the rollback-to-the-wrong-moment
       this control exists to prevent. The map therefore lives in this
       renderer's memory and nowhere else, and a reload starts it empty.
       Nothing is lost by that: the checkpoints themselves live in main and are
       neither dropped nor rewritten by this map.
   ========================================================================== */

/** One recorded snapshot, against the transcript row of the turn it precedes. */
export interface TurnCheckpoint {
  /** mergeTranscript's row key (`e:3`) for the user turn this stands before. */
  turnKey: string;
  /** The main-process checkpoint id, as `createReviewCheckpoint` returned it. */
  checkpointId: string;
  /** Safe files captured. Always > 0 — see rule 1. */
  fileCount: number;
  /** Epoch ms of the record, which is also its order in the list. */
  at: number;
}

/**
 * How many turns of one session keep an offer. Twenty is a screenful of
 * scrollback: further back than that, the working tree has moved on so far
 * that a button on a turn is no longer an honest way to choose a checkpoint.
 */
export const TURN_CHECKPOINTS_PER_SESSION = 20;

const MAX_TURN_KEY = 200;
// The IPC refuses a longer id outright, so a longer one could never restore.
const MAX_CHECKPOINT_ID = 128;

/** This run's entries, one list per session so one busy chat cannot crowd out another. */
const entriesBySession = new Map<string, TurnCheckpoint[]>();

/**
 * Earlier builds kept these entries in localStorage, one key per session under
 * this prefix, and read back only the current run's. Nothing reads them now,
 * so they are removed rather than left in an origin quota the steering queue
 * and the workspace layout also write to.
 */
export const LEGACY_TURN_CHECKPOINTS_PREFIX = "iface.turnCheckpoints.";

export function sweepLegacyTurnCheckpoints(
    storage: Pick<Storage, "length" | "key" | "removeItem">): void {
  const stale: string[] = [];
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (key !== null && key.startsWith(LEGACY_TURN_CHECKPOINTS_PREFIX)) stale.push(key);
  }
  for (const key of stale) storage.removeItem(key);
}

try {
  sweepLegacyTurnCheckpoints(window.localStorage);
} catch {
  // No store (the project's node tests), or one that refuses to be read.
}

/**
 * The list after one turn is recorded. A turn key already present is REPLACED:
 * one row stands before one checkpoint, and a second record for the same key
 * is that row being re-minted, never a second offer on it.
 */
export function nextTurnCheckpoints(current: readonly TurnCheckpoint[],
    entry: TurnCheckpoint): TurnCheckpoint[] {
  const kept = current.filter(item => item.turnKey !== entry.turnKey);
  return [...kept, entry].slice(-TURN_CHECKPOINTS_PER_SESSION);
}

/**
 * What the turn's control says and what the confirm dialog promises. Shown
 * verbatim and unverifiable by the pane, so it states only the checkpoint's
 * own file count and the one consequence that matters.
 */
export function turnRestorePointFor(entry: TurnCheckpoint): TurnRestorePoint {
  const files = `${entry.fileCount} file${entry.fileCount === 1 ? "" : "s"}`;
  return {
    label: `Undo · ${files}`,
    description: `Restores ${files} to the checkpoint Interface saved when this `
      + "message was sent. Any changes made to those files since are lost.",
  };
}

/** Remember the checkpoint taken for one turn. Silent for an empty one. */
export function recordTurnCheckpoint(sessionKey: string, turnKey: string,
    checkpointId: string, fileCount: number): void {
  if (turnKey === "" || turnKey.length > MAX_TURN_KEY) return;
  if (checkpointId === "" || checkpointId.length > MAX_CHECKPOINT_ID) return;
  if (!Number.isInteger(fileCount) || fileCount <= 0) return;
  entriesBySession.set(sessionKey, nextTurnCheckpoints(entriesBySession.get(sessionKey) ?? [],
    { turnKey, checkpointId, fileCount, at: Date.now() }));
}

/**
 * Move one turn's entry to the row key the transcript settled it into — see
 * settledEchoRowKey. Silent when there is nothing to move or the destination is
 * not a key: a re-key that cannot be made honestly leaves the offer exactly
 * where it was, and the worst that follows is a control that stops being drawn.
 */
export function rekeyTurnCheckpoint(sessionKey: string, oldKey: string, newKey: string): void {
  if (oldKey === newKey || newKey === "" || newKey.length > MAX_TURN_KEY) return;
  const entries = entriesBySession.get(sessionKey) ?? [];
  const moving = entries.find(item => item.turnKey === oldKey);
  if (moving === undefined) return;
  // The destination cannot end up carrying two offers. A row already claimed is
  // overwritten by the turn moving onto it, which is the newer of the two and
  // the only one the transcript still has a row for.
  entriesBySession.set(sessionKey, entries
    .filter(item => item === moving || item.turnKey !== newKey)
    .map(item => (item === moving ? { ...item, turnKey: newKey } : item)));
}

/** The `turnRestorePoints` prop for one session's transcript. */
export function turnRestorePointsFor(sessionKey: string): Map<string, TurnRestorePoint> {
  const points = new Map<string, TurnRestorePoint>();
  for (const entry of entriesBySession.get(sessionKey) ?? []) {
    points.set(entry.turnKey, turnRestorePointFor(entry));
  }
  return points;
}

/** The id to restore for one turn, or null when this run recorded none. */
export function checkpointIdFor(sessionKey: string, turnKey: string): string | null {
  const entries = entriesBySession.get(sessionKey) ?? [];
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    if (entries[i].turnKey === turnKey) return entries[i].checkpointId;
  }
  return null;
}

/**
 * Forget one session's turns entirely — the session was deleted, archived or
 * otherwise dropped, and its entries would outlive everything they refer to.
 */
export function forgetTurnCheckpoints(sessionKey: string): void {
  entriesBySession.delete(sessionKey);
}
