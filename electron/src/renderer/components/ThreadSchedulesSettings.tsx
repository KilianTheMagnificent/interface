import "./ThreadSchedules.css";
import type { ScheduleSettingsRow } from "../thread-schedules";
import { SettingsSwitch } from "./SettingsControls";

/**
 * Settings → Threads → Scheduled prompts (T3 section 7): every thread's
 * scheduled prompts and pull-request watch, with what each sends, when it
 * runs next and how its last run went; each can be turned on or off, and a
 * schedule deleted. The rows are Rust's view in words (thread-schedules.ts).
 * Drawn as rows of the "Kept for your threads" card (owner, 2026-11-15): a
 * count row, then one row per schedule.
 */
export function ThreadSchedulesSettings({ rows, locked, error, onEnable, onDelete }: {
  rows: readonly ScheduleSettingsRow[];
  locked: boolean;
  error: string | null;
  onEnable: (row: ScheduleSettingsRow, enabled: boolean) => void;
  onDelete: (row: ScheduleSettingsRow) => void;
}) {
  return <>
    <div className="settings-row" data-thread-schedules-settings="">
      <div className="settings-row-copy">
        <span className="settings-row-label">Scheduled prompts</span>
        <span className="settings-row-description">
          {rows.length === 0
            ? "None yet. Schedule one from a thread's ⋯ menu, or turn on Watch in its Ship bar."
            : "Each sends its prompt to its thread when it is due."}
        </span>
      </div>
      <span className="settings-row-count">{rows.length}</span>
    </div>
    {rows.map(row => (
      <div key={row.rowKey} className="settings-row is-item" data-schedule-row={row.rowKey}>
        <div className="settings-row-copy">
          <span className="settings-row-label" title={row.what}>
            {row.threadTitle} · {row.what}{row.byAgent ? " (set by the agent)" : ""}
          </span>
          <span className="settings-row-description">Next: {row.next} · Last: {row.last}</span>
        </div>
        <div className="settings-row-action">
          {row.kind === "schedule" && (
            <button type="button" className="settings-button" disabled={locked}
              onClick={() => { onDelete(row); }}>Delete</button>
          )}
          <SettingsSwitch label={`${row.what} on ${row.threadTitle}`} checked={row.enabled} disabled={locked}
            onChange={enabled => { onEnable(row, enabled); }} />
        </div>
      </div>
    ))}
    {error !== null && <p className="settings-error" role="alert">Scheduled prompts: {error}</p>}
  </>;
}
