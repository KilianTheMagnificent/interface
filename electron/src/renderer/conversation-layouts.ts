import { sessionKey } from "../shared/engine";
import type { SessionKey, SessionKind, SessionSummary } from "../shared/engine";
import { sessionSurfaceOf, type ConversationSurface } from "./sidebar-view";
import { EMPTY_WORKSPACE_STATE, reconcileWorkspaceState, type WorkspaceState } from "./workspace";

/**
 * The conversation pane layouts, one per surface that holds conversations.
 *
 * Code holds terminal workspaces only (`sessionSurfaceOf` never answers
 * "code"), so it has no layout: a slot of its own was reconciled against
 * sessions that cannot exist and emptied at every registry push, which is how
 * the adversarial-review pair and Agent's Files touched reveal used to vanish
 * within a second of opening. Every session reveal now lands on the surface
 * that owns the session (`conversationSurfaceFor`).
 */
export type ConversationLayouts = Readonly<Record<ConversationSurface, WorkspaceState>>;

export const CONVERSATION_SURFACES: readonly ConversationSurface[] = ["chat", "agent"];

/** The layout `surface` shows; Code's is always empty. */
export function layoutFor(layouts: ConversationLayouts, surface: SessionKind): WorkspaceState {
  return surface === "code" ? EMPTY_WORKSPACE_STATE : layouts[surface];
}

/**
 * Where a session opens: its listed kind's surface. One the listing does not
 * hold yet stays on the conversation surface on screen, or goes to Chat (where
 * every non-agent session lives) from Code.
 */
export function conversationSurfaceFor(summary: Pick<SessionSummary, "kind"> | undefined,
    current: SessionKind): ConversationSurface {
  if (summary !== undefined) return sessionSurfaceOf(summary);
  return current === "code" ? "chat" : current;
}

/**
 * Drops each layout's panes whose sessions the listing no longer holds under
 * that surface. Hands back `layouts` itself when nothing was dropped, so an
 * unchanged registry push costs no render.
 */
export function reconcileConversationLayouts(layouts: ConversationLayouts,
    sessions: readonly SessionSummary[]): ConversationLayouts {
  let known: Map<ConversationSurface, SessionKey[]> | undefined;
  const keysOf = (surface: ConversationSurface): SessionKey[] => {
    if (known === undefined) {
      known = new Map(CONVERSATION_SURFACES.map(kind => [kind, [] as SessionKey[]]));
      for (const summary of sessions) {
        known.get(sessionSurfaceOf(summary))!.push(sessionKey(summary.engine, summary.sessionId));
      }
    }
    return known.get(surface)!;
  };
  // An empty layout has nothing to drop, so its key list is never built.
  const reconcile = (surface: ConversationSurface): WorkspaceState =>
    layouts[surface].openKeys.length === 0
      ? layouts[surface] : reconcileWorkspaceState(layouts[surface], keysOf(surface));
  const chat = reconcile("chat");
  const agent = reconcile("agent");
  return chat === layouts.chat && agent === layouts.agent ? layouts : { chat, agent };
}
