import { useCallback, useMemo, useState } from "react";
import type {
  ScheduleWhen, ThreadCommand, ThreadControlResponse, ThreadScheduleView, ThreadsView,
} from "../shared/threads";
import { ipcMessage } from "./ipc-message";
import { scheduleComposerLine, scheduleSettingsRows, type ScheduleComposerLine, type ScheduleSettingsRow } from "./thread-schedules";

/** The schedule dialog that is open: for which thread, and the schedule being changed. */
export interface ScheduleDialogState {
  key: string;
  title: string;
  editing?: ThreadScheduleView;
}

/**
 * Scheduled prompts' renderer state (T3 section 7): the dialog, each pane's
 * line above the composer and the Settings list, all from Rust's view, and
 * the commands they send through `iface:threadControl`. App passes the
 * request; this hook calls nothing itself.
 */
export function useThreadSchedules(
  view: ThreadsView | null,
  request: (command: ThreadCommand) => Promise<ThreadControlResponse>,
  titleOf: (key: string) => string,
) {
  const [dialog, setDialog] = useState<ScheduleDialogState | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const openDialog = useCallback((key: string, title: string, editing?: ThreadScheduleView) => {
    setDialog({ key, title, ...(editing === undefined ? {} : { editing }) });
  }, []);
  const closeDialog = useCallback(() => { setDialog(null); }, []);
  const save = useCallback(async (fields: { name?: string; prompt: string; when: ScheduleWhen }) => {
    if (dialog === null) return;
    await request({
      kind: "scheduleSet", key: dialog.key, prompt: fields.prompt, when: fields.when,
      ...(fields.name === undefined ? {} : { name: fields.name }),
      ...(dialog.editing === undefined ? {} : { id: dialog.editing.id }),
    });
    setDialog(null);
  }, [dialog, request]);
  const run = useCallback((command: ThreadCommand) => {
    setPending(true);
    setError(null);
    request(command).then(() => undefined, (err: unknown) => { setError(ipcMessage(err)); })
      .finally(() => { setPending(false); });
  }, [request]);
  const cancelSchedule = useCallback((id: string) => { run({ kind: "scheduleDelete", id }); }, [run]);
  const setWatch = useCallback((key: string, enabled: boolean) => { run({ kind: "watchSet", key, enabled }); }, [run]);

  const now = view?.now ?? 0;
  /** Each thread's line above its composer, by key; one stable object per view. */
  const lines = useMemo(() => {
    const byKey = new Map<string, ScheduleComposerLine>();
    for (const thread of view?.threads ?? []) {
      const line = scheduleComposerLine(thread.schedules, now);
      if (line !== null) byKey.set(thread.key, line);
    }
    return byKey;
  }, [view, now]);
  const rows = useMemo(() => scheduleSettingsRows(view, titleOf, now), [view, titleOf, now]);
  const settings = useMemo(() => ({
    rows,
    pending,
    error,
    onEnable: (row: ScheduleSettingsRow, enabled: boolean) => {
      run(row.kind === "watch" || row.id === undefined
        ? { kind: "watchSet", key: row.threadKey, enabled }
        : { kind: "scheduleEnable", id: row.id, enabled });
    },
    onDelete: (row: ScheduleSettingsRow) => {
      if (row.id !== undefined) run({ kind: "scheduleDelete", id: row.id });
    },
  }), [rows, pending, error, run]);

  return { dialog, openDialog, closeDialog, save, cancelSchedule, setWatch, lines, settings, now, error };
}
