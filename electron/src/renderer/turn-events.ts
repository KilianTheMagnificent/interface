/**
 * When an engine event ends the current turn. One definition, read by the
 * steering queue (delivery and pause), approval visibility and the
 * display-history reserve for unresolved approvals, so those can never
 * disagree about whether a turn is over.
 */
export interface TurnEdgeEvent {
  kind: string;
  status?: string;
  retryable?: boolean;
}

/** The turn is over and none of its approvals can still be answered: the
 * session closed, the status settled to idle or failed, or a non-retryable
 * error arrived. */
export function eventEndsTurn(event: TurnEdgeEvent): boolean {
  return event.kind === "session.closed"
    || event.kind === "status.changed" && (event.status === "idle" || event.status === "failed")
    || event.kind === "error" && event.retryable === false;
}

/** The turn ended badly — a closed session, a failed status or a
 * non-retryable error — rather than settling to a clean idle. */
export function eventFailsTurn(event: TurnEdgeEvent): boolean {
  return event.kind === "session.closed"
    || event.kind === "status.changed" && event.status === "failed"
    || event.kind === "error" && event.retryable === false;
}
