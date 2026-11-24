import { useContext, useLayoutEffect, useMemo, useRef } from "react";
import type { EngineEvent } from "../../shared/engine";
import { contextUsageReports, latestPlanEntries } from "../conversation-reducer";
import {
  conversationProjectionFor, createConversationProjection, onlyStreamedTextSince,
} from "../conversation-projection";
import { Conversation, type ConversationProps } from "./Conversation";
import { PaneVisibleContext } from "./ConversationPaneSurface";

type Drawn = { events: EngineEvent[]; thinking: boolean; live: boolean; sessionKey?: string };

/**
 * The log this pane draws. A covered pane keeps the log it last drew while the
 * only change is streamed text, so a pane nobody can see does no per-frame
 * work. It catches up the moment it is shown, a turn starts or ends, liveness
 * or the session changes, or any other kind of event lands, so every transition
 * the transcript reacts to (the turn clock, the finished-turn announcement, an
 * approval) still happens on the frame it happened. Compared with the last
 * COMMITTED frame, so a render React discards cannot move what is held.
 */
function useDrawnEvents(events: EngineEvent[], visible: boolean, thinking: boolean, live: boolean,
    sessionKey: string | undefined): EngineEvent[] {
  const drawn = useRef<Drawn>({ events, thinking, live, sessionKey });
  const last = drawn.current;
  const shown = visible || last.thinking !== thinking || last.live !== live
    || last.sessionKey !== sessionKey || !onlyStreamedTextSince(last.events, events)
    ? events : last.events;
  useLayoutEffect(() => { drawn.current = { events: shown, thinking, live, sessionKey }; });
  return shown;
}

export function ProjectedConversation({
  events, ...props
}: Omit<ConversationProps, "items"> & { events: EngineEvent[] }) {
  const visible = useContext(PaneVisibleContext);
  const shown = useDrawnEvents(events, visible, props.thinking, props.live, props.sessionKey);
  // Keyless panes (fixtures, embeddings) keep a projector of their own; a
  // session's pane shares that session's with every other view of it.
  const local = useMemo(createConversationProjection, []);
  const items = useMemo(() => (props.sessionKey === undefined
    ? local : conversationProjectionFor(props.sessionKey))(shown), [shown, local, props.sessionKey]);
  const livePlan = useMemo(() => latestPlanEntries(shown), [shown]);
  const contextReports = useMemo(() => contextUsageReports(shown), [shown]);
  const historicalPlan = props.history?.status === "ready" ? props.history.plan : undefined;
  return <Conversation {...props} items={items} plan={livePlan ?? historicalPlan}
    context={contextReports.current} previousContext={contextReports.previous} />;
}
