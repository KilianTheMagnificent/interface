import type { HeldSecret, ThreadSecretsView } from "../shared/thread-secrets";

/* ============================================================================
   PRIVATE SECRET CARD (protocol 67, T3 section 12) — the words

   Rust keeps every rule: one request per thread, the name's rules, the file,
   the expiry. These helpers only turn its answers into the card's and the
   Settings list's words. Pure: no store, no `window.iface`.
   ========================================================================= */

/** One row of Settings → Threads → Secrets. */
export interface HeldSecretRow {
  rowKey: string;
  key: string;
  name: string;
  threadTitle: string;
  /** "Removed in 23 h", "Removed in 40 min", "Removed soon". */
  expires: string;
}

/** When a held secret goes, in words. */
export function secretExpiryText(expiresAt: number, now: number): string {
  const minutes = Math.floor((expiresAt - now) / 60_000);
  if (minutes < 1) return "Removed soon";
  if (minutes < 60) return `Removed in ${minutes} min`;
  return `Removed in ${Math.floor(minutes / 60)} h`;
}

/** Rust's list as rows, in its order (soonest expiry first). */
export function heldSecretRows(view: ThreadSecretsView | null, titleOf: (key: string) => string,
    now: number): HeldSecretRow[] {
  return (view?.secrets ?? []).map((secret: HeldSecret) => ({
    rowKey: `${secret.key}\n${secret.name}`,
    key: secret.key,
    name: secret.name,
    threadTitle: titleOf(secret.key),
    expires: secretExpiryText(secret.expiresAt, now),
  }));
}

/** What the card says under its field: where the value goes, and that it never shows. */
export const SECRET_CARD_NOTE =
  "Saved to a private file on this Mac that only this thread's agent is told about, never into the conversation. Removed after 24 hours.";
