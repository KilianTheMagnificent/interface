import type { SessionKey } from "../shared/engine";
import { terminalOutputStreamKey } from "./terminal-output-state";

/**
 * Edge trigger for OS notifications. Fire once per event id, and only when the
 * user is not already looking at that session. `watching` is true when the
 * window is focused AND this session is the focused agent pane — another agent
 * finishing in the background still notifies.
 *
 * The caller records `lastNotified[key] = eventId` after firing.
 */
export function shouldNotify(lastNotified: Record<SessionKey, string>,
  key: SessionKey, eventId: string, watching: boolean): boolean {
  return !watching && lastNotified[key] !== eventId;
}

/** Sessions whose last notified event is remembered across restarts. */
export const LAST_NOTIFIED_LIMIT = 200;

/**
 * Record `eventId` as the last one notified for `key`. The key moves to the
 * end, so insertion order is recency, and the oldest keys beyond `limit` are
 * dropped: the stored map shares this origin's quota with the steering queue.
 * The cost is bounded: a session pushed out by `limit` more recent ones can
 * notify once more for an event it was already told about.
 */
export function rememberNotified(lastNotified: Readonly<Record<SessionKey, string>>,
  key: SessionKey, eventId: string, limit = LAST_NOTIFIED_LIMIT): Record<SessionKey, string> {
  const entries = Object.entries(lastNotified).filter(([seen]) => seen !== key);
  entries.push([key, eventId]);
  return Object.fromEntries(entries.slice(-limit));
}

/** What an OS notification is about, for its setting in Settings › Notifications. */
export type NotificationKind = "finished" | "needs-input";

/** The three Notifications settings, as the renderer reads them. */
export interface NotificationPreferences {
  readonly sound: boolean;
  readonly finished: boolean;
  readonly needsInput: boolean;
}

export const DEFAULT_NOTIFICATION_PREFERENCES: NotificationPreferences =
  Object.freeze({ sound: true, finished: true, needsInput: true });

/** Whether to post a notification of `kind`, and whether macOS plays its sound. */
export interface NotificationPlan {
  readonly show: boolean;
  readonly silent: boolean;
}

export function notificationPlan(preferences: NotificationPreferences,
    kind: NotificationKind): NotificationPlan {
  return {
    show: kind === "finished" ? preferences.finished : preferences.needsInput,
    silent: !preferences.sound,
  };
}

const POST_WITH_SOUND: NotificationPlan = Object.freeze({ show: true, silent: false });

/**
 * Post an OS notification. The sound is the notification's own (`silent:
 * false`), so Focus modes and System Settings › Notifications decide whether
 * it plays — the app used to play its own chime first, before the permission
 * check, so a refused or focused notification still rang. Refused permission
 * now means no banner and no sound. While permission is still being asked
 * for, the first alert waits for the answer.
 */
export function postNotification(title: string, body: string, onClick?: () => void,
    plan: NotificationPlan = POST_WITH_SOUND): void {
  if (!plan.show || typeof Notification === "undefined") return;
  const show = (): void => {
    try {
      const note = new Notification(title, { body, silent: plan.silent });
      note.onclick = () => { onClick?.(); };
    } catch (error: unknown) {
      console.warn("[app] notification failed:", error);
    }
  };
  if (Notification.permission === "granted") show();
  else if (Notification.permission !== "denied") {
    void Notification.requestPermission().then(permission => {
      if (permission === "granted") show();
    }).catch((error: unknown) => {
      console.warn("[app] notification permission request failed:", error);
    });
  }
}

/** Completion is an explicit CLI signal, never just a pause in output. */
export function terminalsNewlyCompleted<T extends {
  readonly id: string; readonly generation: number; readonly completedTurns: number;
}>(previous: ReadonlyMap<string, number>, terminals: readonly T[]): {
  completed: T[]; index: Map<string, number>;
} {
  const index = new Map<string, number>();
  const completed: T[] = [];
  for (const terminal of terminals) {
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    index.set(key, terminal.completedTurns);
    if (terminal.completedTurns > (previous.get(key) ?? 0)) completed.push(terminal);
  }
  return { completed, index };
}

/**
 * Edge trigger for terminal panes: the ones whose activity just flipped to
 * "waiting" while still alive. Identity is the output stream key, so a
 * restarted pane carries a new generation and may ring again; that is intended,
 * a restart is a new agent asking.
 *
 * The caller keeps the returned `index` and passes it back as `previous`.
 */
export function terminalsNewlyWaiting<T extends {
  readonly id: string; readonly generation: number;
  readonly state: string; readonly activity?: string;
}>(
  previous: ReadonlyMap<string, string | undefined>, terminals: readonly T[],
): { waiting: T[]; index: Map<string, string | undefined> } {
  const index = new Map<string, string | undefined>();
  const waiting: T[] = [];
  for (const terminal of terminals) {
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    index.set(key, terminal.activity);
    if (terminal.activity !== "waiting" || previous.get(key) === "waiting") continue;
    if (terminal.state === "running" || terminal.state === "starting") waiting.push(terminal);
  }
  return { waiting, index };
}
