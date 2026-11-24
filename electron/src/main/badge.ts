import type { SessionStatus } from "../shared/engine.js";

// Typed as SessionStatus rather than written inline as a bare string, so
// renaming the union member is a compile error here instead of a badge that
// silently counts to zero forever.
const WAITING: SessionStatus = "waiting";

// The one field the icon reads. The listing crosses BridgeHost.broadcast, whose
// signature is `...args: unknown[]`, so this narrows rather than casts: a
// payload that is not a session array must produce a number, never a throw on
// the broadcast path.
function isWaiting(session: unknown): boolean {
  return typeof session === "object" && session !== null
    && (session as { status?: unknown }).status === WAITING;
}

/**
 * How many sessions are blocked on the user, derived from a `CHANNELS.registry`
 * payload — `SessionRegistry.list()`, the same array the sidebar is about to
 * draw from.
 *
 * Deliberately the SAME predicate and the SAME input as the renderer's own
 * attention count (`status === "waiting"` over the whole listing), because the
 * dock badge and the in-app "N need you" strip are two renderings of one fact.
 * Deriving the badge from anything narrower — only live handles, only the
 * focused project — would let the icon and the window disagree about the number
 * that costs the user the most time, and the icon is the copy they see while
 * the window is behind something else.
 *
 * Archived sessions are outside it because `list()` filters them: archiving is
 * the user saying they do not want that row on screen, and a badge is a row on
 * screen with no way to click into it.
 */
export function waitingCount(listing: unknown): number {
  if (!Array.isArray(listing)) return 0;
  let count = 0;
  for (const session of listing) {
    if (isWaiting(session)) count += 1;
  }
  return count;
}
