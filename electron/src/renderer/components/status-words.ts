import type { SessionStatus } from "../../shared/engine";

/* ============================================================================
   ONE STATUS, ONE WORD

   The sidebar and the workspace tile name the same four states,
   and until this module existed they named them differently: blocked-on-you was
   "Waiting for you" in the sidebar, "Needs attention" on the tile and "Needs
   you" elsewhere, while a merely idle session read "Idle" in two places and
   "Paused" on the tile. Three words for the single most important state make
   cross-surface scanning harder, and "Paused" was not even true — nothing had
   paused those sessions; they were idle, and often detached.

   So the words live here, once, and every surface that prints a status imports
   them. Adding a fifth state means adding it here; there is no second table to
   forget.

   "Needs attention" is the word for `waiting` because it is already the label
   of the sidebar's pinned attention section, so
   the row and the section that lifts it now say the same thing. The counts
   sentence ("3 need you") is deliberately NOT in this map: it is a sentence
   about a number, not the name of a state, and it is shared with the dock badge
   (see src/main/badge.ts).
   ========================================================================= */

const STATUS_WORD: Record<SessionStatus, string> = {
  waiting: "Needs attention",
  working: "Working",
  idle: "Idle",
  failed: "Failed",
};

/** A terminal agent that finished a turn nobody has looked at yet (the blue
    dot, owner 2026-09-25). Renderer-only, so not a SessionStatus: sessions and
    the engine never report it. */
export const DONE_WORD = "Done";

/** The one word for this state, on every surface that prints one. */
export function statusWord(status: SessionStatus): string {
  return STATUS_WORD[status];
}

/**
 * The same word, plus the one extra distinction a surface that OWNS a live
 * engine handle can honestly draw: an idle session with no attached process is
 * DETACHED. It is not paused — nothing paused it — and it is not "Ready"
 * either, which is what the tile used to promise for the live half of the same
 * state.
 *
 * Only the workspace tile passes `live`. The sidebar deliberately does not: it
 * lists every session in the store, almost none of which has a process
 * attached, so "Detached" there would be the word on nearly every card and
 * would say nothing at all.
 */
export function sessionStatusWord(status: SessionStatus, live: boolean): string {
  return status === "idle" && !live ? "Detached" : STATUS_WORD[status];
}
