import { useCallback, useEffect, type Dispatch, type RefObject, type SetStateAction } from "react";
import { sessionKey, type EngineEvent, type SessionKey, type SessionSummary } from "../shared/engine";
import type { IfaceApi } from "../shared/ipc";
import { conversationItems, type ConvItem } from "./conversation-reducer";
import type { HistoryView, TurnRestorePoint } from "./conversation-types";
import { ipcMessage } from "./ipc-message";
import type { AppState } from "./store";
import { settledEchoRowKey, type PendingEcho } from "./transcript-merge";
import {
  checkpointIdFor, recordTurnCheckpoint, rekeyTurnCheckpoint, turnRestorePointsFor,
} from "./turn-checkpoints";

// Shared empty fallbacks for the re-key walk, as App keeps its own.
const NO_EVENTS: EngineEvent[] = [];
const NO_CONVERSATION_ITEMS: readonly ConvItem[] = [];
const NO_PENDING_ECHOES: readonly PendingEcho[] = [];

type TurnRestoreApi = Pick<IfaceApi, "createReviewCheckpoint" | "restoreReviewCheckpoint">;
type RestoringTurn = { key: SessionKey; turnKey: string };

/** What the turn checkpoints read and write in App. The state stays App's. */
export interface TurnRestoreOptions {
  state: Pick<AppState, "events">;
  sessionsRef: RefObject<readonly SessionSummary[]>;
  conversationEchoes: Readonly<Record<SessionKey, readonly PendingEcho[]>>;
  historyByKey: Readonly<Record<SessionKey, HistoryView>>;
  /** Bumped by a batch carrying a live user turn; the re-key effect's one dependency. */
  settledTurnWake: number;
  setSettledTurnWake: Dispatch<SetStateAction<number>>;
  turnRestorePoints: Readonly<Record<SessionKey, ReadonlyMap<string, TurnRestorePoint>>>;
  setTurnRestorePoints: Dispatch<SetStateAction<Record<SessionKey, ReadonlyMap<string, TurnRestorePoint>>>>;
  restoringTurnRef: RefObject<RestoringTurn | null>;
  setRestoringTurn: Dispatch<SetStateAction<RestoringTurn | null>>;
  setTurnRestoreNotice: Dispatch<SetStateAction<{ message: string; error: boolean } | null>>;
}

/**
 * The per-turn Undo's capture, re-key and restore, moved out of App as they
 * were. The restore points, the busy turn and the notice stay App state:
 * forgetting a session clears them long before this runs, and the panes and
 * the notice strip read them. The re-key effect keeps its deliberately
 * partial dependency array (`[settledTurnWake]` alone) and its place among
 * App's effects, since App calls this where the block stood.
 */
export function useTurnRestore(api: TurnRestoreApi, {
  state, sessionsRef, conversationEchoes, historyByKey, settledTurnWake, setSettledTurnWake,
  turnRestorePoints, setTurnRestorePoints, restoringTurnRef, setRestoringTurn, setTurnRestoreNotice,
}: TurnRestoreOptions) {
  /* ---- turn-bound checkpoints ---------------------------------------------
     A snapshot taken as a prompt goes out, remembered against the transcript
     row the prompt is about to occupy, so the turn itself can offer the way
     back. The map, its bound and why it never outlives this run are in
     turn-checkpoints.ts. */

  /**
   * Capture the working tree as one prompt is dispatched. `turnKey` is the row
   * the offer will hang on — mergeTranscript's key for the local echo minted
   * for this send — or null for a dispatch with no transcript row of its own
   * (a queued prompt), which still earns the snapshot as a safety net even
   * though no surface currently offers it back.
   *
   * FIRE AND FORGET, deliberately: nothing here is awaited, no failure reaches
   * the caller, and a session in no repository (main resolved no projectRoot)
   * is skipped before the IPC — a checkpoint is a safety net over the send,
   * never a condition of it.
   */
  const checkpointTurn = useCallback((key: SessionKey, turnKey: string | null) => {
    const summary = sessionsRef.current.find(session =>
      sessionKey(session.engine, session.sessionId) === key);
    // projectRoot IS main's answer to "is this cwd inside a repository"; the
    // checkpoint IPC would refuse anyway, and asking git again from here would
    // be a second, slower answer to a question already resolved.
    if (summary === undefined || summary.projectRoot === undefined) return;
    // The session's own cwd: that is the authorized root, and `git -C cwd`
    // resolves the repository from it.
    const root = summary.cwd;
    // "turn" is what makes this one prunable and, when it captures nothing,
    // unwritten: taken without anybody asking, it must not crowd the user's
    // own checkpoints out of the stored list.
    void api.createReviewCheckpoint(root, "turn").then(created => {
      // No files captured means nothing to roll back to, and the pane's rule
      // is that a turn with no entry keeps no control.
      if (turnKey === null || created.fileCount === 0) return;
      recordTurnCheckpoint(key, turnKey, created.id, created.fileCount);
      setTurnRestorePoints(current => ({ ...current, [key]: turnRestorePointsFor(key) }));
      // The capture takes longer than an ACP engine takes to stream the prompt
      // back, so this row may ALREADY be settled by the time it is recorded.
      // Ask the re-key to look now rather than waiting for a user turn that
      // has been and gone.
      setSettledTurnWake(value => value + 1);
    }, (err: unknown) => {
      // Logged and dropped. The prompt is already on its way, and a failed
      // snapshot must never be reported as a failed send.
      console.warn("[app] could not checkpoint this turn:", err);
    }).catch((err: unknown) => {
      // Nothing above may reject: the recording half throwing would otherwise
      // become an unhandled rejection in the middle of a send.
      console.warn("[app] could not record this turn's checkpoint:", err);
    });
  }, [api]);

  /**
   * Follow a turn across the row key it settles into. Every ACP engine streams
   * the user's own message back as a live `user.message`, and mergeTranscript
   * then drops the echo the checkpoint was recorded against — so an entry left
   * on `e:<id>` names a row that stopped existing a second after the send, and
   * the control disappears. The rule for WHICH row supplied it is the
   * transcript's own (settledEchoRowKey), never a second guess at it here.
   *
   * Driven by `settledTurnWake`, which is bumped in the same commit as the
   * events that carry a user turn: this walks a session's live items, so it
   * must not run on every batch of deltas.
   */
  useEffect(() => {
    for (const [key, points] of Object.entries(turnRestorePoints)) {
      const pending = [...points.keys()].filter(turnKey => turnKey.startsWith("e:"));
      if (pending.length === 0) continue;
      const echoes = conversationEchoes[key] ?? NO_PENDING_ECHOES;
      const live = conversationItems(state.events[key] ?? NO_EVENTS);
      const history = historyByKey[key];
      const past = history?.status === "ready" ? history.items : NO_CONVERSATION_ITEMS;
      let moved = false;
      for (const oldKey of pending) {
        const echo = echoes.find(candidate => `e:${candidate.id}` === oldKey);
        if (echo === undefined) continue;
        const settled = settledEchoRowKey(past, live, echo);
        if (settled === null) continue;
        rekeyTurnCheckpoint(key, oldKey, settled);
        moved = true;
      }
      if (moved) setTurnRestorePoints(current => ({ ...current, [key]: turnRestorePointsFor(key) }));
    }
    // Deliberately the wake alone: it changes in the same render as the events
    // and echoes read above, and depending on those directly would re-run this
    // for every streamed delta.
  }, [settledTurnWake]);

  const restoreTurnCheckpoint = useCallback((key: SessionKey, turnKey: string) => {
    if (restoringTurnRef.current !== null) return;
    const summary = sessionsRef.current.find(session =>
      sessionKey(session.engine, session.sessionId) === key);
    const id = checkpointIdFor(key, turnKey);
    if (summary === undefined || id === null) return;
    // NOT WHILE THE TURN IS STILL OPEN. A restore puts old bytes back under a
    // turn that is still editing files, so the tree would end up neither the
    // checkpoint nor the agent's work — and the agent would keep going against
    // files it no longer wrote. `waiting` is the same hazard and the likelier
    // click: a tile asking for an approval looks stopped, and answering it
    // resumes writing over whatever the restore just put back. Written as the
    // SETTLED states rather than the two live ones because the pair
    // `working`/`waiting` is spelled out elsewhere for the unrelated send gate,
    // and one rule reading like the other would invite them to be changed
    // together. Refused with the reason rather than disabled: the turn this
    // stands on is exactly the one the user wants back.
    if (summary.status !== "idle" && summary.status !== "failed") {
      setTurnRestoreNotice({
        message: "This agent is still running or waiting on an approval. Stop it "
          + "first, then restore — rolling files back underneath an open turn "
          + "leaves the tree in neither state.",
        error: true,
      });
      return;
    }
    restoringTurnRef.current = { key, turnKey };
    setRestoringTurn({ key, turnKey });
    setTurnRestoreNotice(null);
    void api.restoreReviewCheckpoint(summary.cwd, id).then(result => {
      // The pane renders no outcome of its own: a restore rewrites files
      // outside that transcript, so it is reported here — never "restored"
      // alone when paths were skipped or conflicted, and a conflict is the
      // only thing that makes this an error rather than a result.
      const issues = result.skipped.length + result.conflicts.length;
      setTurnRestoreNotice({
        message: `Restored ${result.restored.length} file${result.restored.length === 1
          ? "" : "s"}${issues > 0 ? `; ${issues} path${issues === 1 ? " was" : "s were"} `
          + "skipped or conflicted." : "."}${result.recoveryCheckpointId === undefined
          ? "" : " A recovery checkpoint was saved first."}`,
        error: result.conflicts.length > 0,
      });
    }, (err: unknown) => {
      setTurnRestoreNotice({ message: ipcMessage(err), error: true });
    }).finally(() => {
      restoringTurnRef.current = null;
      setRestoringTurn(null);
    });
  }, [api]);

  return { checkpointTurn, restoreTurnCheckpoint };
}
