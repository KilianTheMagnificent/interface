import { useEffect, useRef } from "react";
import { attentionPlace, type AttentionRow } from "./attention-tray";
import {
  DEFAULT_NOTIFICATION_PREFERENCES, notificationPlan, postNotification, terminalsNewlyCompleted,
  terminalsNewlyWaiting, type NotificationPreferences,
} from "./notify";
import { productLabel } from "./product";
import type { TerminalActivitySummary } from "./terminal-activity";
import { terminalOutputStreamKey } from "./terminal-output-state";

/** Shared presentation path for the terminal center's completion and input alerts. */
export function useTerminalNotifications({ terminals, visible, focusedTerminalId, workspaces,
    onEvent, onActivate, notify = postNotification,
    preferences = DEFAULT_NOTIFICATION_PREFERENCES }: {
  terminals: readonly TerminalActivitySummary[];
  visible: boolean;
  focusedTerminalId: string | null;
  workspaces?: readonly { name: string; terminalIds: readonly string[] }[];
  onEvent: (event: Omit<AttentionRow, "live">) => void;
  onActivate: (terminalId: string) => void;
  notify?: typeof postNotification;
  /** Settings › Notifications; read when an alert fires, so a change re-runs nothing. */
  preferences?: NotificationPreferences;
}): void {
  const settings = useRef(preferences);
  settings.current = preferences;
  const activity = useRef<ReadonlyMap<string, string | undefined>>(new Map());
  const completions = useRef<ReadonlyMap<string, number>>(new Map());
  useEffect(() => {
    const { waiting, index } = terminalsNewlyWaiting(activity.current, terminals);
    const done = terminalsNewlyCompleted(completions.current, terminals);
    activity.current = index;
    completions.current = done.index;
    // Observing a watched completion consumes it too: switching away later
    // must not turn old work into a new alert.
    const watching = (id: string): boolean => document.hasFocus() && visible && focusedTerminalId === id;
    const activate = (id: string): (() => void) => () => {
      onActivate(id);
      window.focus();
    };
    for (const terminal of done.completed) {
      if (watching(terminal.id)) continue;
      const title = terminal.title || attentionPlace(terminal.cwd);
      onEvent({
        id: `terminal-done:${terminalOutputStreamKey(terminal.id, terminal.generation)}:${terminal.completedTurns}`,
        kind: "finished", target: { kind: "terminal", terminalId: terminal.id, generation: terminal.generation }, title,
        where: workspaces?.find(record => record.terminalIds.includes(terminal.id))?.name
          ?? attentionPlace(terminal.cwd),
        at: Date.now(),
      });
      // Record first, so denied or unavailable OS notifications cannot hide
      // the completion from the bell menu.
      notify(`${productLabel()} — agent finished`, title, activate(terminal.id),
        notificationPlan(settings.current, "finished"));
    }
    for (const terminal of waiting) {
      if (watching(terminal.id) || done.completed.some(doneTerminal => doneTerminal.id === terminal.id)) continue;
      notify(`${productLabel()} — needs input`, terminal.title || terminal.cwd, activate(terminal.id),
        notificationPlan(settings.current, "needs-input"));
    }
  }, [focusedTerminalId, notify, onActivate, onEvent, terminals, visible, workspaces]);
}
