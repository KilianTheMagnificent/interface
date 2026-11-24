import "./ThreadSchedules.css";
import { useId, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import type { ScheduleWhen, ThreadScheduleView } from "../../shared/threads";
import { ipcMessage } from "../ipc-message";
import {
  SCHEDULE_MIN_MINUTES, SCHEDULE_SLEEP_NOTE, scheduleDraft, scheduleDraftWhen, type ScheduleDraft,
  type ScheduleKind,
} from "../thread-schedules";
import { CONFIRM_DIALOG_CSS } from "./ConfirmDialog";
import { useModalFocus } from "./ModalBehavior";

/**
 * "Schedule a prompt…" from a thread pane's ⋯ (T3 section 7, protocol 62):
 * the prompt, an optional name, and when — once at a date and time, every N
 * minutes or hours (5 minutes at the least), or on a calendar (the time of
 * day, the days and the timezone teammate routines use). Rust checks it
 * again and owns every rule; its refusal is shown here. The app's
 * ConfirmDialog look.
 */
export interface ThreadScheduleDialogProps {
  threadTitle: string;
  /** The schedule being changed; absent for a new one. */
  editing?: ThreadScheduleView;
  /** Resolves once Rust saved it; rejects with the message to show. */
  onSave: (fields: { name?: string; prompt: string; when: ScheduleWhen }) => Promise<void>;
  onCancel: () => void;
  /** The clock, for tests. */
  now?: () => number;
}

const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;
const KINDS: readonly { kind: ScheduleKind; label: string }[] = [
  { kind: "once", label: "Once" },
  { kind: "interval", label: "Every" },
  { kind: "calendar", label: "On a calendar" },
];

export function ThreadScheduleDialog({ threadTitle, editing, onSave, onCancel, now = Date.now }: ThreadScheduleDialogProps) {
  const baseId = useId();
  const dialogRef = useRef<HTMLElement>(null);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const [draft, setDraft] = useState<ScheduleDraft>(() =>
    scheduleDraft(now(), Intl.DateTimeFormat().resolvedOptions().timeZone, editing));
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const handleKey = useModalFocus({
    open: true,
    containerRef: dialogRef,
    initialFocus: () => promptRef.current,
    onEscape: pending ? () => undefined : onCancel,
    escapeEnabled: !pending,
  });
  const change = (fields: Partial<ScheduleDraft>): void => { setDraft(current => ({ ...current, ...fields })); setError(null); };
  const save = (): void => {
    if (pending) return;
    const checked = scheduleDraftWhen(draft, now());
    if ("error" in checked) { setError(checked.error); return; }
    setPending(true);
    setError(null);
    const name = draft.name.trim();
    onSave({ ...(name === "" ? {} : { name }), prompt: draft.prompt, when: checked.when })
      .then(() => { setPending(false); }, (err: unknown) => { setPending(false); setError(ipcMessage(err)); });
  };
  const handleBackdrop = (event: ReactMouseEvent<HTMLDivElement>): void => {
    if (!pending && event.target === event.currentTarget) onCancel();
  };
  const field = (id: string): string => `${baseId}-${id}`;

  const surface = <>
    <style>{CONFIRM_DIALOG_CSS}</style>
    <div className="confirm-dialog-backdrop" onClick={handleBackdrop}>
      <section ref={dialogRef} className="confirm-dialog thread-schedule-dialog is-floating"
        role="dialog" aria-modal="true" tabIndex={-1} aria-busy={pending || undefined}
        aria-labelledby={field("title")} aria-describedby={field("note")} onKeyDown={handleKey}
        data-thread-schedule-dialog="">
        <form className="confirm-dialog-copy thread-schedule-form"
          onSubmit={event => { event.preventDefault(); save(); }}>
          <h2 id={field("title")} className="confirm-dialog-title">
            {editing === undefined ? "Schedule a prompt" : "Change scheduled prompt"}
          </h2>
          <p className="confirm-dialog-description">To {threadTitle}</p>
          <label className="thread-schedule-field">
            <span>Prompt</span>
            <textarea ref={promptRef} rows={4} value={draft.prompt} disabled={pending}
              onChange={event => { change({ prompt: event.target.value }); }} />
          </label>
          <label className="thread-schedule-field">
            <span>Name (optional)</span>
            <input type="text" value={draft.name} maxLength={80} disabled={pending}
              onChange={event => { change({ name: event.target.value }); }} />
          </label>
          <fieldset className="thread-schedule-when" disabled={pending}>
            <legend>When</legend>
            {KINDS.map(choice => (
              <label key={choice.kind} className="thread-schedule-kind">
                <input type="radio" name={field("kind")} value={choice.kind}
                  checked={draft.kind === choice.kind}
                  onChange={() => { change({ kind: choice.kind }); }} />
                {choice.label}
              </label>
            ))}
            {draft.kind === "once" && (
              <label className="thread-schedule-field">
                <span>At</span>
                <input type="datetime-local" value={draft.onceAt} data-schedule-once=""
                  onChange={event => { change({ onceAt: event.target.value }); }} />
              </label>
            )}
            {draft.kind === "interval" && (
              <div className="thread-schedule-row">
                <input type="number" min={1} step={1} aria-label="Every" value={draft.every}
                  onChange={event => { change({ every: event.target.value }); }} />
                <select aria-label="Unit" value={draft.unit}
                  onChange={event => { change({ unit: event.target.value === "minutes" ? "minutes" : "hours" }); }}>
                  <option value="minutes">minutes</option>
                  <option value="hours">hours</option>
                </select>
                <span className="thread-schedule-hint">At least {SCHEDULE_MIN_MINUTES} minutes.</span>
              </div>
            )}
            {draft.kind === "calendar" && <>
              <label className="thread-schedule-field">
                <span>Time of day</span>
                <input type="time" value={draft.time}
                  onChange={event => { change({ time: event.target.value }); }} />
              </label>
              <div className="thread-schedule-days" role="group" aria-label="Days (none means every day)">
                {DAYS.map((day, index) => (
                  <label key={day}>
                    <input type="checkbox" checked={draft.weekdays.includes(index + 1)}
                      onChange={event => {
                        change({ weekdays: event.target.checked
                          ? [...draft.weekdays, index + 1]
                          : draft.weekdays.filter(value => value !== index + 1) });
                      }} />
                    {day}
                  </label>
                ))}
              </div>
              <label className="thread-schedule-field">
                <span>Timezone (IANA)</span>
                <input type="text" value={draft.timezone}
                  onChange={event => { change({ timezone: event.target.value }); }} />
              </label>
            </>}
          </fieldset>
          <p id={field("note")} className="confirm-dialog-description">{SCHEDULE_SLEEP_NOTE}</p>
          {error !== null && <p className="confirm-dialog-description thread-schedule-error" role="alert">{error}</p>}
        </form>
        <div className="confirm-dialog-actions">
          <button type="button" className="confirm-dialog-button" disabled={pending} onClick={onCancel}>Cancel</button>
          <button type="button" className="confirm-dialog-button confirm-dialog-confirm" disabled={pending}
            onClick={save}>
            {pending ? "Saving…" : editing === undefined ? "Schedule" : "Save"}
          </button>
        </div>
      </section>
    </div>
  </>;
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}
