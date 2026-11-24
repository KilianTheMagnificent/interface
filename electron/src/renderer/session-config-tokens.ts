import type { SessionKey } from "../shared/engine";

/** Every setting a session's pickers can change mid-session. Each has its own token. */
export const CONFIG_FIELDS = ["model", "effort", "fastmode", "permission"] as const;
export type ConfigField = typeof CONFIG_FIELDS[number];

/** One (session, field) token's name; App's model command-refresh map uses it too. */
export const configToken = (key: SessionKey, field: ConfigField): string => `${key}\0${field}`;

/** One settings write's claim on its (session, field) token. */
export interface ConfigAttempt {
  readonly attempt: number;
  /** False once a newer write of the same field, or the session closing, superseded it. */
  isCurrent(): boolean;
  /** Release this write's send lock if it still holds it; true when it did. */
  settle(): boolean;
}

/**
 * The config-attempt token protocol. Every settings write owns a per-session,
 * per-field token, so a late rejection from an older picker choice can never
 * revert a newer accepted one. The optimistic picker value is not yet the
 * provider's policy either: each write also holds an attempt-owned send lock
 * until its IPC acknowledgement arrives, so a prompt cannot leave under the
 * previous model/effort/access policy while the UI already shows the new one.
 * `onChange` runs whenever a lock is taken or released.
 */
export class ConfigAttempts {
  private readonly attempts = new Map<string, number>();
  private readonly inFlight = new Map<string, number>();

  constructor(private readonly onChange: () => void) {}

  begin(key: SessionKey, field: ConfigField): ConfigAttempt {
    const token = configToken(key, field);
    const attempt = (this.attempts.get(token) ?? 0) + 1;
    this.attempts.set(token, attempt);
    this.inFlight.set(token, attempt);
    this.onChange();
    return {
      attempt,
      isCurrent: () => this.attempts.get(token) === attempt,
      settle: () => {
        if (this.inFlight.get(token) !== attempt) return false;
        this.inFlight.delete(token);
        this.onChange();
        return true;
      },
    };
  }

  /** The newest attempt of this (session, field), if any write has begun. */
  current(key: SessionKey, field: ConfigField): number | undefined {
    return this.attempts.get(configToken(key, field));
  }

  /** This field's write is still waiting for its acknowledgement. */
  inFlightFor(key: SessionKey, field: ConfigField): boolean {
    return this.inFlight.has(configToken(key, field));
  }

  /** Any field of this session is still waiting: nothing may be sent yet. */
  pending(key: SessionKey): boolean {
    return CONFIG_FIELDS.some(field => this.inFlightFor(key, field));
  }

  /**
   * The session's handle closed: supersede every outstanding write, so none of
   * their late answers lands on the next run, and release every lock.
   */
  invalidate(key: SessionKey): void {
    for (const field of CONFIG_FIELDS) {
      const token = configToken(key, field);
      this.attempts.set(token, (this.attempts.get(token) ?? 0) + 1);
      this.inFlight.delete(token);
    }
    this.onChange();
  }
}
