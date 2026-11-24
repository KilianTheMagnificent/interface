import { describe, expect, it } from "vitest";
import fixtureV62 from "../fixtures/threads-schedules-v62.json";
import type { ThreadScheduleView, ThreadSchedules, ThreadsView } from "../shared/threads";
import {
  SCHEDULE_SLEEP_NOTE, intervalText, scheduleComposerLine, scheduleDraft, scheduleDraftWhen,
  scheduleLabel, scheduleNextText, scheduleRunText, scheduleSettingsRows, scheduleTimeText,
  scheduleWhenText, scheduledPromptFold, watchOffered, watchTitle, weekdaysText,
} from "./thread-schedules";

// Scheduled prompts and pull-request watch (T3 section 7, protocol 62): the
// words and layout the renderer draws over Rust's view. Times are built in
// the Mac's own zone, as the renderer shows them.
const NOW = new Date(2026, 10, 15, 14, 0).getTime();
const at = (day: number, hour: number, minute = 0): number => new Date(2026, 10, day, hour, minute).getTime();

function schedule(fields: Partial<ThreadScheduleView>): ThreadScheduleView {
  return {
    id: "s-1", prompt: "Check the build.", when: { kind: "interval", minutes: 60 }, enabled: true,
    createdAt: NOW - 1, nextAt: at(15, 15, 30), setBy: "owner", paused: false, ...fields,
  };
}

describe("schedule words", () => {
  it("names times, repeats and days the way the line and Settings say them", () => {
    expect(scheduleTimeText(at(15, 15, 30), NOW)).toBe("Today 15:30");
    expect(scheduleTimeText(at(16, 7), NOW)).toBe("Tomorrow 07:00");
    expect(scheduleTimeText(at(14, 9), NOW)).toBe("Yesterday 09:00");
    expect(scheduleTimeText(at(20, 9), NOW)).toBe("Fri 20 Nov 09:00");
    expect([5, 60, 180, 90].map(intervalText))
      .toEqual(["every 5 minutes", "every hour", "every 3 hours", "every 90 minutes"]);
    expect([[], [1, 2, 3, 4, 5], [6, 7], [3, 1]].map(weekdaysText))
      .toEqual(["every day", "weekdays", "weekends", "Mon, Wed"]);
    expect(scheduleWhenText({ kind: "calendar", calendar: { timezone: "UTC", weekdays: [1, 2, 3, 4, 5], hour: 7, minute: 30 } }))
      .toBe("weekdays at 07:30");
    expect(scheduleWhenText({ kind: "once", at: NOW })).toBe("once");
  });

  it("says what the last firing did, and never offers an unknown one again", () => {
    expect(scheduleRunText(undefined, NOW)).toBe("Not run yet");
    expect(scheduleRunText({ occurrence: 1, at: at(15, 13), outcome: "sent" }, NOW)).toBe("Sent Today 13:00");
    expect(scheduleRunText({ occurrence: 1, at: at(15, 13), outcome: "unknown" }, NOW))
      .toBe("May not have arrived (Today 13:00); not sent again");
    expect(scheduleRunText({ occurrence: 1, at: NOW - 5_000, outcome: "unknown", detail: "Sending…" }, NOW))
      .toBe("Sending…");
    // A send still marked as going out from before a restart never finished.
    expect(scheduleRunText({ occurrence: 1, at: at(15, 13), outcome: "unknown", detail: "Sending…" }, NOW))
      .toBe("May not have arrived (Today 13:00); not sent again");
    expect(scheduleRunText({ occurrence: 1, at: at(15, 13), outcome: "refused", detail: "An Away loop is running this thread." }, NOW))
      .toBe("Not sent Today 13:00: An Away loop is running this thread.");
    expect(scheduleRunText({ occurrence: 1, at: at(15, 13), outcome: "skipped", detail: "Paused while the thread is settled." }, NOW))
      .toBe("Skipped Today 13:00: Paused while the thread is settled.");
  });
});

describe("the line above the composer", () => {
  it("shows the soonest schedule's first 60 characters, when, and +N more", () => {
    const long = "Read every open issue in the tracker, sort them by how often they come up and summarize";
    const schedules: ThreadSchedules = {
      schedules: [
        schedule({ id: "a", prompt: long, setBy: "agent" }),
        schedule({ id: "b", nextAt: at(16, 9), when: { kind: "once", at: at(16, 9) } }),
        schedule({ id: "off", enabled: false }),
        schedule({ id: "done", nextAt: undefined, when: { kind: "once", at: at(14, 9) } }),
      ],
    };
    const line = scheduleComposerLine(schedules, NOW);
    expect(line?.id).toBe("a");
    expect(line?.what).toBe("Read every open issue in the tracker, sort them by how often…");
    expect([...line?.what ?? ""].length).toBeLessThanOrEqual(61);
    expect(line?.when).toBe("Today 15:30 · every hour");
    expect(line?.byAgent).toBe(true);
    expect(line?.more.map(more => more.id)).toEqual(["b"]);
    // Off and finished schedules leave the line to Settings.
    expect(scheduleComposerLine({ schedules: schedules.schedules.slice(2) }, NOW)).toBeNull();
    expect(scheduleComposerLine(undefined, NOW)).toBeNull();
    expect(scheduleLabel({ name: "Nightly check", prompt: long })).toBe("Nightly check");
  });

  it("says when a firing waits for the turn, and when the thread is settled", () => {
    expect(scheduleNextText(schedule({ waiting: NOW }), NOW)).toBe("Waiting for the turn to end");
    expect(scheduleNextText(schedule({ paused: true }), NOW)).toBe("Paused while settled");
    expect(scheduleNextText(schedule({ enabled: false }), NOW)).toBe("Off");
  });
});

describe("the schedule dialog", () => {
  it("refuses an interval under five minutes, a past time and an empty prompt", () => {
    const draft = { ...scheduleDraft(NOW, "America/Chicago"), prompt: "Check the build." };
    expect(scheduleDraftWhen({ ...draft, kind: "interval", every: "4", unit: "minutes" }, NOW))
      .toEqual({ error: "Repeat every 5 minutes at the least." });
    expect(scheduleDraftWhen({ ...draft, kind: "interval", every: "5", unit: "minutes" }, NOW))
      .toEqual({ when: { kind: "interval", minutes: 5 } });
    expect(scheduleDraftWhen({ ...draft, kind: "interval", every: "2", unit: "hours" }, NOW))
      .toEqual({ when: { kind: "interval", minutes: 120 } });
    expect(scheduleDraftWhen({ ...draft, kind: "interval", every: "1.5", unit: "hours" }, NOW))
      .toHaveProperty("error");
    expect(scheduleDraftWhen({ ...draft, onceAt: "2026-11-15T13:59" }, NOW))
      .toEqual({ error: "Choose a time in the future." });
    expect(scheduleDraftWhen({ ...draft, onceAt: "2026-11-15T14:05" }, NOW))
      .toEqual({ when: { kind: "once", at: at(15, 14, 5) } });
    expect(scheduleDraftWhen({ ...draft, prompt: "  " }, NOW)).toEqual({ error: "Write the prompt to send." });
    expect(scheduleDraftWhen({ ...draft, kind: "calendar", time: "07:30", weekdays: [5, 1, 1] }, NOW))
      .toEqual({ when: { kind: "calendar", calendar: { timezone: "America/Chicago", weekdays: [1, 5], hour: 7, minute: 30 } } });
    expect(scheduleDraftWhen({ ...draft, kind: "calendar", time: "25:00" }, NOW)).toHaveProperty("error");
    expect(SCHEDULE_SLEEP_NOTE).toBe("Runs when Interface is open; a run missed while the Mac sleeps happens when it wakes.");
  });

  it("opens an hour ahead for a new schedule and as it was for an edit", () => {
    expect(scheduleDraft(NOW, "UTC").onceAt).toBe("2026-11-15T15:00");
    const edited = scheduleDraft(NOW, "UTC", schedule({ name: "Hourly", when: { kind: "interval", minutes: 90 } }));
    expect([edited.kind, edited.every, edited.unit, edited.name]).toEqual(["interval", "90", "minutes", "Hourly"]);
    const calendar = scheduleDraft(NOW, "UTC", schedule({
      when: { kind: "calendar", calendar: { timezone: "Europe/Paris", weekdays: [6], hour: 8, minute: 5 } },
    }));
    expect([calendar.kind, calendar.timezone, calendar.time, calendar.weekdays]).toEqual(["calendar", "Europe/Paris", "08:05", [6]]);
  });
});

describe("the transcript", () => {
  it("folds a scheduled prompt and a watch prompt to one line, with what was sent under it", () => {
    expect(scheduledPromptFold("[Interface] Scheduled prompt \"Nightly check\":\nCheck the build.\nThen say."))
      .toEqual({ line: "Scheduled prompt · Nightly check", body: "Check the build.\nThen say." });
    expect(scheduledPromptFold("[Interface] Watching pull request #42 (https://github.com/o/r/pull/42): checks are failing.\n\nfix")?.line)
      .toBe("Pull request watch · #42");
    expect(scheduledPromptFold("Scheduled prompt \"x\":\nnot from Interface")).toBeNull();
    expect(scheduledPromptFold("[Interface] The thread \"x\" (Claude) you started finished: ok")).toBeNull();
  });
});

describe("Watch and Settings", () => {
  it("offers Watch only for an open pull request and says what it is doing", () => {
    expect(watchOffered({ pullRequest: { url: "u", number: 1, state: "open", checkedAt: 0 } })).toBe(true);
    expect(watchOffered({ pullRequest: { url: "u", number: 1, state: "merged", checkedAt: 0 } })).toBe(false);
    expect(watchOffered(undefined)).toBe(false);
    const watch = { enabled: true, needsGh: true, failingChecks: 0, reviewThreads: 0, waiting: false };
    expect(watchTitle(watch)).toContain("gh auth login");
    expect(watchTitle(undefined)).toContain("Stops when the pull request is merged or closed");
  });

  it("lists every schedule and watch with its thread, next run and last result", () => {
    const view = fixtureV62.view as unknown as ThreadsView;
    const rows = scheduleSettingsRows(view, key => (key.startsWith("claude#6c1d") ? "Fix login" : key), view.now);
    expect(rows.map(row => [row.kind, row.what, row.enabled, row.byAgent])).toEqual([
      ["schedule", "Look at the queue again.", true, true],
      ["schedule", "Nightly check", true, false],
      ["schedule", "Run the test suite and tell me what fails.", false, false],
      ["watch", "Watch #42", true, false],
    ]);
    expect(rows.every(row => row.threadTitle === "Fix login")).toBe(true);
    expect(rows[1]?.next).toBe("Waiting for the turn to end");
    expect(rows[1]?.last).toContain("An Away loop is running this thread.");
    expect(rows[3]?.next).toBe("Watching");
    expect(rows[0]?.id).toBe("s-1a2b3c4d5e6f");
    expect(rows[3]?.id).toBeUndefined();
    expect(scheduleSettingsRows(null, key => key, NOW)).toEqual([]);
  });
});
