import type {
  ScheduleRun, ScheduleWhen, ThreadSchedules, ThreadScheduleView, ThreadsView, ThreadView, ThreadWatchView,
} from "../shared/threads";

/**
 * Scheduled prompts and pull-request watch (T3 section 7, protocol 62), as
 * the renderer draws them: the quiet line above a thread's composer, the
 * schedule dialog's choices, the folded prompt in the transcript, the Ship
 * bar's Watch toggle and the Settings list. Rust owns every rule (due
 * times, the delivery gate, what a watch sends); these are words and
 * layout over its view. Times are ms since the epoch.
 */

/** The shortest repeat the dialog offers; Rust refuses anything shorter too. */
export const SCHEDULE_MIN_MINUTES = 5;
/** The prompt's first characters the composer line shows. */
export const SCHEDULE_LINE_CHARS = 60;
/** The first line of a prompt Interface sent on a schedule (Rust's `scheduled_prompt`). */
export const SCHEDULED_PROMPT_PREFIX = "[Interface] Scheduled prompt \"";
/** The first line of a Resume at reset prompt, before its colon (Rust's `resume_prompt`, protocol 66). */
export const RESUME_PROMPT_PREFIX = "[Interface] Resumed after the usage limit reset";
/** The first line of a pull-request watch's prompt (Rust's `watch_prompt`). */
export const WATCH_PROMPT_PREFIX = "[Interface] Watching pull request #";
/** What the dialog says about sleep: Interface does not hold the Mac awake for a schedule. */
export const SCHEDULE_SLEEP_NOTE = "Runs when Interface is open; a run missed while the Mac sleeps happens when it wakes.";

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

/** "Today 15:30", "Tomorrow 07:00", "Mon 7 Dec 09:00", in the Mac's own time. */
export function scheduleTimeText(at: number, now: number): string {
  const date = new Date(at);
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const day = new Date(now);
  const startOf = (value: Date): number => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const days = Math.round((startOf(date) - startOf(day)) / 86_400_000);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Tomorrow ${time}`;
  if (days === -1) return `Yesterday ${time}`;
  const weekday = WEEKDAYS[(date.getDay() + 6) % 7] ?? "";
  const month = date.toLocaleString("en-US", { month: "short" });
  return `${weekday} ${date.getDate()} ${month} ${time}`;
}

/** "every 5 minutes", "every hour", "every 3 hours", "every 90 minutes". */
export function intervalText(minutes: number): string {
  if (minutes % 60 === 0) {
    const hours = minutes / 60;
    return hours === 1 ? "every hour" : `every ${hours} hours`;
  }
  return minutes === 1 ? "every minute" : `every ${minutes} minutes`;
}

/** The days a calendar runs: "every day", "weekdays", "Mon, Wed". */
export function weekdaysText(weekdays: readonly number[]): string {
  const days = [...new Set(weekdays)].sort((a, b) => a - b);
  if (days.length === 0 || days.length === 7) return "every day";
  if (days.join() === "1,2,3,4,5") return "weekdays";
  if (days.join() === "6,7") return "weekends";
  return days.map(day => WEEKDAYS[day - 1] ?? String(day)).join(", ");
}

/** How a schedule repeats, in words: "once", "every 2 hours", "weekdays at 07:30". */
export function scheduleWhenText(when: ScheduleWhen): string {
  switch (when.kind) {
    case "once": return "once";
    case "interval": return intervalText(when.minutes);
    case "calendar": {
      const { calendar } = when;
      return `${weekdaysText(calendar.weekdays)} at ${pad(calendar.hour)}:${pad(calendar.minute)}`;
    }
  }
}

/** What the last firing did, in a few words: "Sent Today 15:30", "Not sent: …". */
export function scheduleRunText(run: ScheduleRun | undefined, now: number): string {
  if (run === undefined) return "Not run yet";
  const at = scheduleTimeText(run.at, now);
  switch (run.outcome) {
    case "sent": return `Sent ${at}`;
    // "Sending…" is Rust's mark while the send is out; one left from before a
    // restart never finished, so it reads as unknown.
    case "unknown": return run.detail === "Sending…" && now - run.at < 60_000
      ? "Sending…" : `May not have arrived (${at}); not sent again`;
    case "skipped": return `Skipped ${at}${run.detail === undefined ? "" : `: ${run.detail}`}`;
    case "refused": return `Not sent ${at}${run.detail === undefined ? "" : `: ${run.detail}`}`;
  }
}

/** A schedule's label: its name, else its prompt's first line. */
export function scheduleLabel(schedule: Pick<ThreadScheduleView, "name" | "prompt">, limit = SCHEDULE_LINE_CHARS): string {
  const name = schedule.name?.trim() ?? "";
  if (name !== "") return name;
  const line = schedule.prompt.split("\n").map(part => part.trim()).find(part => part !== "") ?? "";
  const collapsed = line.replace(/\s+/gu, " ");
  return [...collapsed].length > limit ? `${[...collapsed].slice(0, limit).join("").trimEnd()}…` : collapsed;
}

/** When a schedule runs next, as the line and Settings say it. */
export function scheduleNextText(schedule: ThreadScheduleView, now: number): string {
  if (!schedule.enabled) return "Off";
  if (schedule.paused) return "Paused while settled";
  if (schedule.waiting !== undefined) return "Waiting for the turn to end";
  if (schedule.nextAt === undefined) return "Done";
  const repeat = schedule.when.kind === "once" ? "" : ` · ${scheduleWhenText(schedule.when)}`;
  return `${scheduleTimeText(schedule.nextAt, now)}${repeat}`;
}

/**
 * A Resume at reset on the line above the composer (protocol 66): "Resumes at
 * 15:10" today, "Resumes Tomorrow 07:00" later, "Resumes when the turn ends"
 * while its time came during a turn.
 */
export function resumeLineText(schedule: ThreadScheduleView, now: number): string {
  if (schedule.waiting !== undefined) return "Resumes when the turn ends";
  if (schedule.nextAt === undefined) return "Resumed";
  const when = scheduleTimeText(schedule.nextAt, now);
  return when.startsWith("Today ") ? `Resumes at ${when.slice("Today ".length)}` : `Resumes ${when}`;
}

/** A schedule's label in Settings: a resume says what it is, a prompt its first words. */
export function scheduleRowLabel(schedule: ThreadScheduleView): string {
  return schedule.kind === "resume" ? "Resume at reset" : scheduleLabel(schedule);
}

/** The quiet line above a thread's composer: its soonest schedule, and how many more. */
export interface ScheduleComposerLine {
  id: string;
  /** The prompt's first 60 characters (or its name). */
  what: string;
  when: string;
  /** "set by the agent" for a schedule its agent made. */
  byAgent: boolean;
  /** Every other active schedule of the thread, for "+N more". */
  more: readonly ThreadScheduleView[];
  schedule: ThreadScheduleView;
}

/** The composer line, or null when the thread has nothing on (off and finished ones stay in Settings). */
export function scheduleComposerLine(schedules: ThreadSchedules | undefined, now: number): ScheduleComposerLine | null {
  const active = (schedules?.schedules ?? [])
    .filter(schedule => schedule.enabled && (schedule.nextAt !== undefined || schedule.waiting !== undefined));
  const [first, ...more] = active;
  if (first === undefined) return null;
  return {
    id: first.id,
    what: scheduleLabel(first),
    when: scheduleNextText(first, now),
    byAgent: first.setBy === "agent",
    more,
    schedule: first,
  };
}

// --- The dialog --------------------------------------------------------------------

export type ScheduleKind = "once" | "interval" | "calendar";
export type IntervalUnit = "minutes" | "hours";

/** What the dialog holds while the owner edits it. */
export interface ScheduleDraft {
  name: string;
  prompt: string;
  kind: ScheduleKind;
  /** `<input type="datetime-local">`'s value, in the Mac's own time. */
  onceAt: string;
  every: string;
  unit: IntervalUnit;
  timezone: string;
  /** "HH:MM". */
  time: string;
  weekdays: number[];
}

function localInputValue(at: number): string {
  const date = new Date(at);
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** A new schedule's draft, an hour from now; or an existing one's to edit. */
export function scheduleDraft(now: number, timezone: string, existing?: ThreadScheduleView): ScheduleDraft {
  const draft: ScheduleDraft = {
    name: existing?.name ?? "",
    prompt: existing?.prompt ?? "",
    kind: "once",
    onceAt: localInputValue(now + 3_600_000),
    every: "1",
    unit: "hours",
    timezone,
    time: "09:00",
    weekdays: [1, 2, 3, 4, 5],
  };
  const when = existing?.when;
  if (when?.kind === "once") return { ...draft, kind: "once", onceAt: localInputValue(when.at) };
  if (when?.kind === "interval") {
    return when.minutes % 60 === 0
      ? { ...draft, kind: "interval", every: String(when.minutes / 60), unit: "hours" }
      : { ...draft, kind: "interval", every: String(when.minutes), unit: "minutes" };
  }
  if (when?.kind === "calendar") {
    return {
      ...draft, kind: "calendar", timezone: when.calendar.timezone,
      time: `${pad(when.calendar.hour)}:${pad(when.calendar.minute)}`, weekdays: [...when.calendar.weekdays],
    };
  }
  return draft;
}

/** The `when` a draft asks for, or why it cannot be sent yet. Rust checks it again. */
export function scheduleDraftWhen(draft: ScheduleDraft, now: number): { when: ScheduleWhen } | { error: string } {
  if (draft.prompt.trim() === "") return { error: "Write the prompt to send." };
  if (new TextEncoder().encode(draft.prompt).length > 16 * 1024) {
    return { error: "A scheduled prompt is at most 16 KiB of text." };
  }
  switch (draft.kind) {
    case "once": {
      const at = new Date(draft.onceAt).getTime();
      if (!Number.isFinite(at)) return { error: "Choose a date and time." };
      if (at <= now) return { error: "Choose a time in the future." };
      return { when: { kind: "once", at } };
    }
    case "interval": {
      const count = Number(draft.every);
      if (!Number.isInteger(count) || count < 1) return { error: "Repeat every whole number of minutes or hours." };
      const minutes = draft.unit === "hours" ? count * 60 : count;
      if (minutes < SCHEDULE_MIN_MINUTES) return { error: `Repeat every ${SCHEDULE_MIN_MINUTES} minutes at the least.` };
      if (minutes > 30 * 24 * 60) return { error: "Repeat every 30 days at the most." };
      return { when: { kind: "interval", minutes } };
    }
    case "calendar": {
      const match = /^(\d{1,2}):(\d{2})$/u.exec(draft.time);
      const hour = Number(match?.[1]);
      const minute = Number(match?.[2]);
      if (match === null || hour > 23 || minute > 59) return { error: "Choose a time of day." };
      if (draft.timezone.trim() === "") return { error: "Choose a timezone." };
      return {
        when: {
          kind: "calendar",
          calendar: { timezone: draft.timezone.trim(), weekdays: [...new Set(draft.weekdays)].sort((a, b) => a - b), hour, minute },
        },
      };
    }
  }
}

// --- The transcript ------------------------------------------------------------------

/**
 * A prompt Interface sent on a schedule or for a watched pull request,
 * folded to one line ("Scheduled prompt · Nightly check", "Pull request
 * watch · #42", "Resumed after the limit reset") with what was sent under
 * it; null for any other prompt.
 */
export function scheduledPromptFold(text: string): { line: string; body: string } | null {
  const newline = text.indexOf("\n");
  const first = newline < 0 ? text : text.slice(0, newline);
  const rest = newline < 0 ? "" : text.slice(newline + 1);
  if (first === `${RESUME_PROMPT_PREFIX}:`) return { line: "Resumed after the limit reset", body: rest };
  if (first.startsWith(SCHEDULED_PROMPT_PREFIX) && first.endsWith("\":")) {
    return { line: `Scheduled prompt · ${first.slice(SCHEDULED_PROMPT_PREFIX.length, -2)}`, body: rest };
  }
  const watched = /^\[Interface\] Watching pull request #(\d+) /u.exec(first);
  if (watched !== null) return { line: `Pull request watch · #${watched[1]}`, body: text.slice("[Interface] ".length) };
  return null;
}

// --- The Ship bar ---------------------------------------------------------------------

/** Whether the Ship bar offers Watch: the thread tracks an open pull request (or is already watched). */
export function watchOffered(thread: Pick<ThreadView, "pullRequest" | "schedules"> | undefined): boolean {
  return thread?.pullRequest?.state === "open" || thread?.schedules?.watch?.enabled === true;
}

/** The Watch toggle's title: what it does now. */
export function watchTitle(watch: ThreadWatchView | undefined): string {
  if (watch?.enabled !== true) {
    return "Send this thread a prompt when a check fails or a new review comment arrives. Stops when the pull request is merged or closed.";
  }
  if (watch.needsGh) return "Watching needs the GitHub CLI: install gh and run gh auth login.";
  if (watch.waiting) return "A prompt waits for the thread's turn to end.";
  return "Watching for failing checks and new review comments.";
}

// --- Settings -------------------------------------------------------------------------

/** One row of Settings → Threads → Scheduled prompts. */
export interface ScheduleSettingsRow {
  /** Stable across views: `schedule:<id>` or `watch:<thread key>`. */
  rowKey: string;
  kind: "schedule" | "watch";
  threadKey: string;
  threadTitle: string;
  what: string;
  next: string;
  last: string;
  enabled: boolean;
  byAgent: boolean;
  /** The schedule's id; absent for a watch. */
  id?: string;
}

/**
 * Every listed thread's schedules and watch, for Settings: by thread
 * (newest activity first, as Threads lists them), soonest schedule first.
 */
export function scheduleSettingsRows(view: ThreadsView | null, titleOf: (key: string) => string, now: number):
    ScheduleSettingsRow[] {
  const rows: ScheduleSettingsRow[] = [];
  for (const thread of view?.threads ?? []) {
    const schedules = thread.schedules;
    if (schedules === undefined) continue;
    const threadTitle = titleOf(thread.key);
    for (const schedule of schedules.schedules) {
      rows.push({
        rowKey: `schedule:${schedule.id}`, kind: "schedule", threadKey: thread.key, threadTitle,
        what: scheduleRowLabel(schedule), next: scheduleNextText(schedule, now),
        last: scheduleRunText(schedule.lastRun, now), enabled: schedule.enabled,
        byAgent: schedule.setBy === "agent", id: schedule.id,
      });
    }
    const watch = schedules.watch;
    if (watch !== undefined) {
      rows.push({
        rowKey: `watch:${thread.key}`, kind: "watch", threadKey: thread.key, threadTitle,
        what: thread.pullRequest === undefined ? "Pull request watch" : `Watch #${thread.pullRequest.number}`,
        next: !watch.enabled ? "Off" : watch.needsGh ? "Needs gh" : watch.waiting ? "Waiting for the turn to end" : "Watching",
        last: scheduleRunText(watch.lastRun, now), enabled: watch.enabled, byAgent: false,
      });
    }
  }
  return rows;
}
