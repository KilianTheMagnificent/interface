import "./ThreadSchedules.css";
import { memo, useState } from "react";
import type { ThreadScheduleView } from "../../shared/threads";
import { resumeLineText, scheduleLabel, scheduleNextText, type ScheduleComposerLine } from "../thread-schedules";

/**
 * Above a thread's composer when it has scheduled prompts (T3 section 7):
 * the soonest one's first words and when it runs, with Edit and Cancel;
 * "+N more" lists the rest the same way. A Resume at reset (protocol 66)
 * reads "Resumes at 15:10 · Cancel". Rust's view says all of it.
 */
export const ThreadScheduleLine = memo(function ThreadScheduleLine({ line, now, onEdit, onCancel }: {
  line: ScheduleComposerLine;
  now: number;
  onEdit: (schedule: ThreadScheduleView) => void;
  /** Deletes the schedule. */
  onCancel: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const row = (schedule: ThreadScheduleView, lead: string) => schedule.kind === "resume" ? (
    <div key={schedule.id} className="thread-schedule-line" data-thread-schedule={schedule.id}
      data-thread-resume="">
      <span className="thread-schedule-line-when" title={schedule.prompt}>{resumeLineText(schedule, now)}</span>
      <span aria-hidden="true">·</span>
      <button type="button" className="thread-schedule-line-act" onClick={() => { onCancel(schedule.id); }}>Cancel</button>
    </div>
  ) : (
    <div key={schedule.id} className="thread-schedule-line" data-thread-schedule={schedule.id}>
      <span className="thread-schedule-line-lead">{lead}</span>
      <span className="thread-schedule-line-what" title={schedule.prompt}>{scheduleLabel(schedule)}</span>
      <span aria-hidden="true">·</span>
      <span className="thread-schedule-line-when">{scheduleNextText(schedule, now)}</span>
      {schedule.setBy === "agent" && <span className="thread-schedule-line-by">set by the agent</span>}
      <span aria-hidden="true">·</span>
      <button type="button" className="thread-schedule-line-act" onClick={() => { onEdit(schedule); }}>Edit</button>
      <span aria-hidden="true">·</span>
      <button type="button" className="thread-schedule-line-act" onClick={() => { onCancel(schedule.id); }}>Cancel</button>
    </div>
  );
  return (
    <div className="thread-schedules" role="group" aria-label="Scheduled prompts">
      {row(line.schedule, "Next:")}
      {line.more.length > 0 && !open && (
        <button type="button" className="thread-schedule-line-act thread-schedule-more"
          aria-expanded={false} onClick={() => { setOpen(true); }}>
          +{line.more.length} more
        </button>
      )}
      {open && line.more.map(schedule => row(schedule, "Then:"))}
    </div>
  );
});
