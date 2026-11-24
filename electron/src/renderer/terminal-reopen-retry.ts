/** Reopen a saved terminal across the transient window right after a host
 * restart.
 *
 * A saved chat pane can miss its host reopen proof for a moment on the first
 * attempt: the managed-worktree ledger is still being re-verified with git, and
 * the host's private proof sidecar is mid-settle, so the create is refused with
 * `terminal-authority-unavailable`, `terminal-reopen-authority-unproven` or
 * `terminal-reopen-authority-mismatch`. The host authority gate is NOT relaxed
 * here — it re-runs in full on every attempt, so a session whose worktree is
 * genuinely gone still ends as a failed tab. These bounded retries only let a
 * transient startup window pass, rebuilding the request from the freshly
 * settled host roster so a request that raced the host's own startup write
 * matches the saved proof the second time.
 *
 * Any other refusal the host marks as final (a bad cwd, capacity, a provider
 * launch failure), or as not safe to replay, is not retried: each retry is
 * another provider launch, and a rebuilt request can carry a different chat
 * id. It goes straight to the shell fallbacks. The host's code reaches the
 * renderer in the error's text (the v1 error shape, shared/ipc-error.ts); a
 * refusal that arrives without it (main's own, such as the terminal data
 * channel still connecting, or an older build's) keeps the bounded retry this
 * module has always given it.
 */

import type {
  CreateTerminalRequest, TerminalLaunchSessionV1, TerminalSummary,
} from "../shared/ipc";
import { ipcErrorShape } from "./ipc-message";

export const REOPEN_RETRY_BACKOFF_MS: readonly number[] = [250, 600, 1400];

/** The host's refusals that a moment after its own restart can clear. */
const TRANSIENT_REOPEN_CODES: ReadonlySet<string> = new Set([
  "terminal-authority-unavailable",
  "terminal-reopen-authority-unproven",
  "terminal-reopen-authority-mismatch",
]);

/** Whether a refused reopen may be sent again. The facts are the host
 * error's own: a refusal without them says nothing, and keeps the retry; one
 * whose marker this build cannot read might say anything, and does not. */
function reopenRefusalRetryable(error: unknown): boolean {
  const shape = ipcErrorShape(error);
  if (shape.kind === "untyped") return true;
  if (shape.kind === "unrecognized" || !shape.replaySafe) return false;
  return shape.retryable || TRANSIENT_REOPEN_CODES.has(shape.code);
}

export interface ReopenAttemptDeps {
  readonly createTerminal: (request: CreateTerminalRequest) => Promise<TerminalSummary>;
  /** Rebuild the request from the current host roster, so a stale or racy
   * identity is replaced by the one the host proof expects before the retry. */
  readonly rebuild: (id: string) => Promise<CreateTerminalRequest | undefined>;
  /** Non-chat panes may come back as a plain shell; a chat pane gets `[]`. */
  readonly shellFallbacks: (request: CreateTerminalRequest) => readonly CreateTerminalRequest[];
  readonly sleep: (ms: number) => Promise<void>;
  readonly cancelled: () => boolean;
  readonly backoff?: readonly number[];
}

export type ReopenAttempt =
  | { readonly ok: true; readonly terminal: TerminalSummary; readonly launchSession?: TerminalLaunchSessionV1 }
  | { readonly ok: false; readonly cancelled: boolean; readonly error: unknown };

export async function reopenWithRetry(
  request: CreateTerminalRequest, deps: ReopenAttemptDeps,
): Promise<ReopenAttempt> {
  const backoff = deps.backoff ?? REOPEN_RETRY_BACKOFF_MS;
  let attempt = request;
  let error: unknown;
  for (let round = 0; round <= backoff.length; round++) {
    if (deps.cancelled()) return { ok: false, cancelled: true, error };
    try {
      const terminal = await deps.createTerminal(attempt);
      return { ok: true, terminal, launchSession: attempt.launchSession };
    } catch (failure) {
      error = failure;
      if (round === backoff.length || !reopenRefusalRetryable(failure)) break;
      await deps.sleep(backoff[round]);
      if (deps.cancelled()) return { ok: false, cancelled: true, error };
      // A missing rebuild (the pane left the roster) keeps the last request, so
      // the final fallback still runs rather than dropping the pane silently.
      try {
        attempt = (await deps.rebuild(request.id)) ?? attempt;
      } catch (failure) {
        // A failed roster read must reach the pane's failure UI, not escape
        // the reopen result and leave a permanently blank starting pane.
        return { ok: false, cancelled: deps.cancelled(), error: failure };
      }
    }
  }
  for (const fallback of deps.shellFallbacks(request)) {
    if (deps.cancelled()) return { ok: false, cancelled: true, error };
    try {
      const terminal = await deps.createTerminal(fallback);
      return { ok: true, terminal };
    } catch (retryError) { error = retryError; }
  }
  return { ok: false, cancelled: false, error };
}
