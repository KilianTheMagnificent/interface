/**
 * `iface:threadSecret` (protocol 67, T3 section 12): the private secret card.
 * An agent asks through team tools' `request_secret`; the thread's view
 * carries the open request (`ThreadView.secretRequest`). The owner's value
 * crosses the wire once, in `give`, and is never echoed back: every answer is
 * names only. Rust keeps the value in one owner-only file. Mirrors
 * `crates/interface-contracts/src/thread_secrets.rs`; both round-trip
 * `electron/src/fixtures/quick-wins-v67.json`.
 */

/** The open request a thread's view carries. */
export interface ThreadSecretRequest {
  id: string;
  name: string;
  reason: string;
  /** Who asks ("Claude"). */
  agent: string;
}

export type ThreadSecretCommand =
  | { kind: "list" }
  | { kind: "give"; key: string; id: string; value: string }
  | { kind: "decline"; key: string; id: string }
  | { kind: "forget"; key: string; name: string };

/** One secret Interface holds. */
export interface HeldSecret {
  key: string;
  name: string;
  /** When it is removed (ms since the epoch). */
  expiresAt: number;
}

/** Every answer: names only, never a value. */
export interface ThreadSecretsView {
  version: 1;
  secrets: HeldSecret[];
}
