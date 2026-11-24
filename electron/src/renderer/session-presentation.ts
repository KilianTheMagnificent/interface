import type { SessionSummary } from "../shared/engine";

/** Project references need provider tools; plain Chat has none. */
export function sessionHasWorkspaceContext(session: Pick<SessionSummary, "kind">): boolean {
  return session.kind !== "chat";
}

export function sessionHasPermissionControls(session: Pick<SessionSummary, "kind">): boolean {
  return session.kind !== "chat" && session.kind !== "voice";
}

/**
 * The mode shown in a pane and restored before its first send must agree.
 * Chat/Voice adapters reject permission changes, including presets saved by Code.
 * This only projects the adapter's contract; Rust still enforces it on every call.
 *
 * A mode picked or launched in this window wins; then the mode the host says
 * it runs the session in (protocol 68), so a reloaded window, or a thread the
 * host started, shows the session's own mode; the mode saved for the model is
 * only the default for a session the host said nothing about.
 */
export function sessionPermissionMode(
  session: Pick<SessionSummary, "kind" | "permissionMode">,
  current: string | undefined,
  remembered: string | undefined,
  chosen?: string,
): string | undefined {
  if (!sessionHasPermissionControls(session)) return undefined;
  // A pick in this window, Manual included, shows before the host confirms it.
  if (chosen !== undefined && chosen !== "") return chosen;
  if (current !== undefined && current !== "" && current !== "default") return current;
  if (session.permissionMode !== undefined && session.permissionMode !== "") {
    return session.permissionMode;
  }
  return remembered || "default";
}

/**
 * The mode a session runs in now, as a change of it starts from: picked in
 * this window, else launched by it, else the host's (protocol 68), else the
 * provider default. Without the host's mode, picking Manual for a thread the
 * host started (or after a reload) looked like no change and did nothing.
 */
export function modeInForce(
  session: Pick<SessionSummary, "permissionMode"> | undefined,
  known: { chosen: string | undefined; launched: string | undefined },
): string {
  const host = session?.permissionMode;
  return known.chosen ?? known.launched
    ?? (host !== undefined && host !== "" ? host : undefined) ?? "default";
}

/** What the window knows about a session's mode before its first send. */
export interface ModeToRestoreFacts {
  /** The window has a live handle for the session. */
  live: boolean;
  /** A mode picked in this window. */
  chosen: string | undefined;
  /** The mode this window launched it in. */
  launched: string | undefined;
  /** The mode saved for the session's model. */
  saved: string | undefined;
}

/**
 * The mode to set before a session's first send in this window, or none.
 * A session the host already runs in a mode of its own (protocol 68) keeps
 * it: the host resumes it in that mode, and the mode saved for its model
 * belongs to another thread, so applying it would narrow or widen this one.
 * Otherwise, as before: the chosen, launched or saved mode, so a session the
 * host said nothing about does not resume at the provider default while its
 * pane says otherwise.
 */
export function modeToRestore(
  session: Pick<SessionSummary, "kind" | "permissionMode">,
  facts: ModeToRestoreFacts,
): string | undefined {
  if (facts.live || !sessionHasPermissionControls(session)) return undefined;
  if (session.permissionMode !== undefined && session.permissionMode !== "") return undefined;
  const mode = facts.chosen ?? facts.launched ?? facts.saved;
  return mode === undefined || mode === "" || mode === "default" ? undefined : mode;
}

/**
 * The permission mode a session was given here — changed to in its pane,
 * else launched with — for a fork of it to start in. Nothing on the wire
 * carries a mode, so without this a fork starts in the provider default.
 * Chat and Voice have no mode to carry, and a session not listed has none
 * known.
 */
export function givenPermissionMode(
  session: Pick<SessionSummary, "kind"> | undefined,
  chosen: string | undefined,
  launched: string | undefined,
): string | undefined {
  if (session === undefined || !sessionHasPermissionControls(session)) return undefined;
  return chosen ?? launched;
}
