import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import type { EngineEvent, SessionKey, SessionSummary } from "../shared/engine";
import { conversationProjectionFor } from "./conversation-projection";
import { settleDetachedSubagents, type ConvItem } from "./conversation-reducer";
import type { HistoryView } from "./conversation-types";
import type { AppState } from "./store";
import { subagentParent, subagentPrompt } from "./subagent-detail";
import { findSubagentItem } from "./tool-activity";
import type { WorkspaceState } from "./workspace";

// Shared empty fallbacks: a stable identity keeps detailView's memo still.
const NO_EVENTS: EngineEvent[] = [];
const NO_CONVERSATION_ITEMS: readonly ConvItem[] = [];

/* ---- the subagent detail column ------------------------------------------
   The transcript cell for a spawned subagent is one row. It is allowed to be
   one row because everything it cannot hold lives in a third app-level column
   beside it, and this is the state that column runs on. */

/** WHICH subagent the detail column is showing.
 *
 *  A PAIR, never a bare agentKey. Up to four tiles each mount their own
 *  transcript for their own session, and an agentKey is a tool_use id — unique
 *  inside one session and nothing more. Keyed on the key alone, a collision
 *  would paint a selected cell in a pane the user never clicked and resolve the
 *  panel against the wrong transcript. */
export type SubagentSelection = {
  sessionKey: SessionKey;
  agentKey: string;
  /** The activation came from a key rather than a pointer. A keyboard user is
   *  taken to what they just opened; a mouse user stays in the transcript they
   *  are reading. This is the one place the two inputs differ. */
  viaKeyboard: boolean;
  /** Changes on every keyboard activation, including re-activating the same
   * row, so the inspector can honor a fresh focus request. */
  focusRequest: number;
};

/** The workspace tile a session key is rendered in. */
export function workspaceTile(key: SessionKey): HTMLElement | undefined {
  return Array.from(document.querySelectorAll<HTMLElement>(
    ".workspace-agent[data-agent-key]"))
    .find(node => node.dataset.agentKey === key);
}

/** The transcript cell for an agentKey. `.sub[data-agent-key]` and not the bare
 *  attribute: a workspace tile carries one too, keyed by sessionKey. */
function subagentCell(selection: SubagentSelection): HTMLElement | undefined {
  const tile = workspaceTile(selection.sessionKey);
  return Array.from(tile?.querySelectorAll<HTMLElement>(".sub[data-agent-key]") ?? [])
    .find(node => node.dataset.agentKey === selection.agentKey);
}

/** Focus back to the cell that opened the panel, on every close route.
 *
 *  QUERIED, not remembered: the panel can be re-targeted at a nested agent or
 *  at its parent while it is open, so "the row that opened it" is the row for
 *  whatever the selection now names. If that row has left the document — the
 *  tile was closed, the transcript reloaded — focus goes to the owning tile's
 *  title, never to document.body. */
function focusSubagentRow(
  selection: SubagentSelection, ancestor?: HTMLElement | null,
): void {
  const row = subagentCell(selection)
    ?.querySelector<HTMLButtonElement>(".sub-row");
  if (row !== null && row !== undefined) {
    row.focus();
    return;
  }
  // A NESTED cell is in the document only because the selection force-opens the
  // agent above it, so clearing the selection unmounts the very row we are
  // returning to. The parent cell survives that collapse; land there rather
  // than at the top of the tile, which is where a keyboard user was NOT.
  const near = ancestor?.isConnected === true
    ? ancestor.querySelector<HTMLButtonElement>(".sub-row") : null;
  if (near !== null && near !== undefined) {
    near.focus();
    return;
  }
  workspaceTile(selection.sessionKey)
    ?.querySelector<HTMLButtonElement>(".workspace-tile-title")?.focus();
}

/** What the detail column reads from App. */
export interface SubagentDetailOptions {
  state: Pick<AppState, "events" | "composerOpen">;
  workspace: Pick<WorkspaceState, "openKeys" | "maximizedKey">;
  historyByKey: Readonly<Record<SessionKey, HistoryView>>;
  historyPages: RefObject<Record<SessionKey, { events: EngineEvent[]; items: ConvItem[] }>>;
  liveKeys: RefObject<Set<SessionKey>>;
  sessionByKey: ReadonlyMap<SessionKey, SessionSummary>;
  settingsOpen: boolean;
  centerView: "sessions" | "canvas" | "terminal";
  focusWorkspaceTile: (key: SessionKey) => void;
}

/**
 * The subagent detail column's selection, its open, close, re-target and
 * reveal routes, the effects that close it when its subject goes, and the
 * agent, parent and prompt it draws. Moved out of App as it was; App calls it
 * where the block stood, so its two effects keep their place among App's.
 */
export function useSubagentDetail({
  state, workspace, historyByKey, historyPages, liveKeys, sessionByKey, settingsOpen, centerView,
  focusWorkspaceTile,
}: SubagentDetailOptions) {
  // The subagent the detail column is describing, or null when the column is
  // not rendered at all. There is no persistent empty inspector: 340px reading
  // "nothing selected" is 340px of nothing on every screen with no subagents,
  // which is most of them.
  const [subagentDetail, setSubagentDetail] = useState<SubagentSelection | null>(null);
  const subagentDetailRef = useRef<SubagentSelection | null>(null);
  const subagentFocusRequest = useRef(0);
  // The last name this selection resolved to, so a selection that outlives its
  // subject can say WHICH agent is gone rather than "a subagent".
  const subagentNameRef = useRef<string>("");

  // Read by the close and reveal routes, which must keep a stable identity —
  // they are props of the panel, and re-creating them on every selection change
  // would re-run the panel's focus effect on data it did not ask about.
  subagentDetailRef.current = subagentDetail;

  const openSubagentDetail = useCallback(
    (key: SessionKey, agentKey: string, viaKeyboard: boolean) => {
      subagentNameRef.current = "";
      subagentFocusRequest.current += 1;
      setSubagentDetail({ sessionKey: key, agentKey, viaKeyboard,
        focusRequest: subagentFocusRequest.current });
    }, []);

  const closeSubagentDetail = useCallback(() => {
    const current = subagentDetailRef.current;
    // Resolved BEFORE the state clears, while the nested row is still drawn.
    const ancestor = current === null ? null
      : subagentCell(current)?.parentElement
        ?.closest<HTMLElement>(".sub[data-agent-key]") ?? null;
    setSubagentDetail(null);
    if (current === null) return;
    // Deferred one frame: the column is still mounted this tick, and focus
    // moved into it would otherwise be dropped on document.body as it goes.
    window.requestAnimationFrame(() => { focusSubagentRow(current, ancestor); });
  }, []);

  // Re-target rather than open: the panel's own breadcrumb and its nested-spawn
  // rows. viaKeyboard is true for both because the control that was activated
  // belongs to the panel and is replaced by the re-target — leaving focus on it
  // would leave it on a node the next render may not draw.
  const retargetSubagentDetail = useCallback((agentKey: string) => {
    subagentNameRef.current = "";
    subagentFocusRequest.current += 1;
    setSubagentDetail(current => current === null
      ? null : { sessionKey: current.sessionKey, agentKey, viaKeyboard: true,
        focusRequest: subagentFocusRequest.current });
  }, []);

  const revealSubagentDetail = useCallback(() => {
    const current = subagentDetailRef.current;
    if (current === null) return;
    // Focus the owning tile first, so a
    // reveal cannot scroll a transcript the user is not looking at.
    focusWorkspaceTile(current.sessionKey);
    window.requestAnimationFrame(() => {
      subagentCell(current)?.scrollIntoView({ block: "nearest" });
      focusSubagentRow(current);
    });
  }, [focusWorkspaceTile]);

  // SELECTION DIES WITH ITS SUBJECT. Closing the owning tile and pointing that
  // tile at another session both arrive here as the key leaving openKeys. A
  // column that outlives what it describes is presentation with no data behind
  // it, which is the failure this app keeps having to undo.
  useEffect(() => {
    if (subagentDetail === null) return;
    if (workspace.openKeys.includes(subagentDetail.sessionKey)) return;
    setSubagentDetail(null);
  }, [subagentDetail, workspace.openKeys]);

  const detailEvents = subagentDetail === null
    ? NO_EVENTS : state.events[subagentDetail.sessionKey] ?? NO_EVENTS;
  const detailHistory = subagentDetail === null
    ? undefined : historyByKey[subagentDetail.sessionKey];
  const detailHistoryItems = detailHistory?.status === "ready"
    ? detailHistory.items : NO_CONVERSATION_ITEMS;
  const detailHistoryEvents = subagentDetail === null
    ? NO_EVENTS : historyPages.current[subagentDetail.sessionKey]?.events ?? NO_EVENTS;
  const detailSessionLive = subagentDetail !== null
    && liveKeys.current.has(subagentDetail.sessionKey);
  // The session's shared projection, the one its pane reads: called first here,
  // it does the frame's work once and the pane's own call returns the same
  // items. Unchanged subagent cards keep their objects, so a frame that did not
  // touch the selected agent hands the panel the agent it already drew.
  const detailView = useMemo(() => {
    if (subagentDetail === null) return null;
    const currentItems = conversationProjectionFor(subagentDetail.sessionKey)(detailEvents);
    const liveItems = detailSessionLive
      ? currentItems : settleDetachedSubagents(currentItems);
    const historyItems = detailSessionLive
      ? detailHistoryItems : settleDetachedSubagents(detailHistoryItems);
    const agent = findSubagentItem(liveItems, subagentDetail.agentKey)
      ?? findSubagentItem(historyItems, subagentDetail.agentKey);
    const parent = subagentParent(liveItems, subagentDetail.agentKey)
      ?? subagentParent(historyItems, subagentDetail.agentKey);
    return {
      agent,
      parent,
      prompt: subagentPrompt(detailEvents, subagentDetail.agentKey)
        ?? subagentPrompt(detailHistoryEvents, subagentDetail.agentKey),
    };
  }, [detailEvents, detailHistoryEvents, detailHistoryItems, detailSessionLive, subagentDetail]);
  const detailSession = subagentDetail === null ? undefined
    : sessionByKey.get(subagentDetail.sessionKey);
  // The agent is resolved fresh on every event, so the last name it had is the
  // only thing left to say when it stops resolving.
  if (detailView?.agent !== undefined) {
    subagentNameRef.current = detailView.agent.subagentType;
  }

  // An inspector is meaningful only beside the transcript that owns it. A
  // destination change is an explicit context switch, so close the selection
  // instead of leaving a detail column beside New Chat or the canvas.
  useEffect(() => {
    if (subagentDetail === null) return;
    const sourceHidden = state.composerOpen || settingsOpen || centerView !== "sessions"
      || workspace.maximizedKey !== null
        && workspace.maximizedKey !== subagentDetail.sessionKey;
    if (sourceHidden) setSubagentDetail(null);
  }, [centerView, settingsOpen, state.composerOpen,
    subagentDetail, workspace.maximizedKey]);

  return {
    subagentDetail, setSubagentDetail, subagentDetailRef, subagentNameRef,
    openSubagentDetail, closeSubagentDetail, retargetSubagentDetail, revealSubagentDetail,
    detailView, detailSession,
  };
}
