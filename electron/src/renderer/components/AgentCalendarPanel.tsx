import { useEffect, useState } from "react";
import type { AgentCalendarSchedule, AgentCalendarView } from "../../shared/agent-work";
import type { AgentRequestHandler } from "./AgentWorkspacePanel";
export function AgentCalendarPanel({ agentId, routineId, revision, onRequest, elapsedLabel }: {
  agentId: string; routineId: string; revision: number; elapsedLabel: string; onRequest: AgentRequestHandler;
}) {
  const [calendar, setCalendar] = useState<AgentCalendarView | null>(null);
  const [preview, setPreview] = useState<AgentCalendarView | null>(null);
  const [draft, setDraft] = useState<AgentCalendarSchedule>({ timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, weekdays: [], hour: 9, minute: 0, missed: "skip", pauseOnFailure: true });
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const load = async (): Promise<void> => {
    const result = await onRequest({ kind: "calendar", agentId, command: { action: "read", routineId } });
    if (!result.ok) { setError(result.error); return; }
    const next = result.response.calendar;
    if (next !== undefined) { setCalendar(next); if (next.schedule !== null) setDraft(next.schedule); }
  };
  const save = async (schedule: AgentCalendarSchedule | null): Promise<void> => {
    setPending(true);
    const result = await onRequest({ kind: "calendar", agentId, command: { action: "save", routineId, expectedRevision: revision, schedule } });
    setPending(false);
    if (!result.ok) { setError(result.error); return; }
    setCalendar(result.response.calendar ?? null); setPreview(null); setError(null);
  };
  useEffect(() => { void load(); }, [agentId, routineId]);
  useEffect(() => { setPreview(null); }, [draft]);
  return <details className="agent-calendar-controls" onToggle={event => { if (event.currentTarget.open) void load(); }}>
    <summary>{calendar?.schedule ? `Calendar · ${String(calendar.schedule.hour).padStart(2, "0")}:${String(calendar.schedule.minute).padStart(2, "0")} ${calendar.schedule.timezone}` : `${elapsedLabel} · Calendar schedule`}</summary>
    {error !== null && <p role="alert">{error}</p>}
    <p>{calendar?.schedule === null ? "Currently using the elapsed interval." : "Calendar times follow the selected timezone."} Editing pauses the routine. Run now performs a real test; inspect its results before enabling.</p>
    <p>Spring-forward missing times are skipped. Fall-back repeated times run once, at the first occurrence. Interface and this Mac must be running and awake.</p>
    <label>IANA timezone<input className="agent-dialog-input" value={draft.timezone} onChange={event => { setDraft({ ...draft, timezone: event.target.value }); }} /></label>
    <label>Local time<input className="agent-dialog-input" type="time" value={`${String(draft.hour).padStart(2, "0")}:${String(draft.minute).padStart(2, "0")}`} onChange={event => { const [hour, minute] = event.target.value.split(":").map(Number); if (hour !== undefined && minute !== undefined) setDraft({ ...draft, hour, minute }); }} /></label>
    <fieldset><legend>Days (none selected means every day)</legend>{["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map((day, index) => <label key={day}><input type="checkbox" checked={draft.weekdays.includes(index + 1)} onChange={event => { setDraft({ ...draft, weekdays: event.target.checked ? [...draft.weekdays, index + 1].sort() : draft.weekdays.filter(value => value !== index + 1) }); }} />{day}</label>)}</fieldset>
    <label>When a run is missed<select className="agent-dialog-input" value={draft.missed} onChange={event => { setDraft({ ...draft, missed: event.target.value as "skip" | "runOnce" }); }}><option value="skip">Skip missed runs</option><option value="runOnce">Run once when available</option></select></label>
    <label><input type="checkbox" checked={draft.pauseOnFailure} onChange={event => { setDraft({ ...draft, pauseOnFailure: event.target.checked }); }} />Pause after failure or uncertain execution</label>
    <div className="agent-workspace-actions">
      <button className="agent-dialog-button" type="button" disabled={pending} onClick={() => { void onRequest({ kind: "calendar", agentId, command: { action: "preview", schedule: draft, afterMs: Date.now() } }).then(result => { if (!result.ok) setError(result.error); else { setPreview(result.response.calendar ?? null); setError(null); } }); }}>Preview next runs</button>
      <button className="agent-dialog-button" type="button" disabled={pending} onClick={() => { void save(draft); }}>Save calendar and pause</button>
      <button className="agent-dialog-button" type="button" disabled={pending} onClick={() => { void save(null); }}>Use elapsed interval</button>
    </div>
    {calendar !== null && <><p>{calendar.tested ? "Current task passed its test run." : "Current task still needs a successful test run."}</p>{preview !== null && <p>Preview only — save to apply these times.</p>}<ul>{(preview ?? calendar).nextOccurrencesMs.map(at => <li key={at}>{new Date(at).toLocaleString(undefined, { timeZone: (preview ?? calendar).schedule?.timezone ?? "UTC", timeZoneName: "short" })}</li>)}</ul></>}
  </details>;
}
