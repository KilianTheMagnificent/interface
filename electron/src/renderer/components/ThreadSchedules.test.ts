import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ThreadScheduleView } from "../../shared/threads";
import { scheduleComposerLine, type ScheduleSettingsRow } from "../thread-schedules";
import { ThreadHandoffPrompt } from "./ThreadAwayParts";
import { ThreadPaneMenuItems } from "./ThreadPaneHead";
import { ThreadScheduleDialog } from "./ThreadScheduleDialog";
import { ThreadScheduleLine } from "./ThreadScheduleLine";
import { ThreadSchedulesSettings } from "./ThreadSchedulesSettings";

// Scheduled prompts' markup (T3 section 7, protocol 62): what each piece
// draws from its props. The decisions are thread-schedules.ts's.
const NOW = new Date(2026, 10, 15, 14, 0).getTime();
const schedule = (fields: Partial<ThreadScheduleView>): ThreadScheduleView => ({
  id: "s-1", prompt: "Check the build.", when: { kind: "interval", minutes: 60 }, enabled: true,
  createdAt: NOW, nextAt: new Date(2026, 10, 15, 15, 30).getTime(), setBy: "owner", paused: false, ...fields,
});
const noop = (): void => undefined;

describe("scheduled prompts' markup", () => {
  it("offers Schedule a prompt… in a thread pane's ⋯ only when the pane passes it", () => {
    const menu = (onSchedule?: () => void) => renderToStaticMarkup(createElement(ThreadPaneMenuItems, {
      title: "Fix login", maximized: false, splitLabel: "New thread", choose: noop, onToggleMaximize: noop,
      onClose: noop, ...(onSchedule === undefined ? {} : { onSchedule }),
    }));
    expect(menu(noop)).toContain("Schedule a prompt…");
    expect(menu()).not.toContain("Schedule a prompt");
  });

  it("draws the line above the composer with Edit, Cancel and +N more", () => {
    const line = scheduleComposerLine({
      schedules: [schedule({ name: "Nightly check", setBy: "agent" }), schedule({ id: "s-2" })],
    }, NOW);
    if (line === null) throw new Error("no line");
    const html = renderToStaticMarkup(createElement(ThreadScheduleLine, { line, now: NOW, onEdit: noop, onCancel: noop }));
    expect(html).toContain("Next:");
    expect(html).toContain("Nightly check");
    expect(html).toContain("Today 15:30 · every hour");
    expect(html).toContain("set by the agent");
    expect(html).toContain(">Edit<");
    expect(html).toContain(">Cancel<");
    expect(html).toContain("+1 more");
  });

  it("opens the dialog with the prompt, the three ways to say when and the sleep note", () => {
    const html = renderToStaticMarkup(createElement(ThreadScheduleDialog, {
      threadTitle: "Fix login", onSave: () => Promise.resolve(), onCancel: noop, now: () => NOW,
    }));
    for (const words of ["Schedule a prompt", "To Fix login", "Once", "Every", "On a calendar",
      "Runs when Interface is open; a run missed while the Mac sleeps happens when it wakes."]) {
      expect(html).toContain(words);
    }
    expect(html).toContain("type=\"datetime-local\"");
  });

  it("lists every schedule and watch in Settings with a switch, and Delete for schedules", () => {
    const rows: ScheduleSettingsRow[] = [
      { rowKey: "schedule:s-1", kind: "schedule", threadKey: "k", threadTitle: "Fix login", what: "Nightly check",
        next: "Today 15:30", last: "Not run yet", enabled: true, byAgent: true, id: "s-1" },
      { rowKey: "watch:k", kind: "watch", threadKey: "k", threadTitle: "Fix login", what: "Watch #42",
        next: "Needs gh", last: "Not run yet", enabled: true, byAgent: false },
    ];
    const html = renderToStaticMarkup(createElement(ThreadSchedulesSettings, {
      rows, locked: false, error: null, onEnable: noop, onDelete: noop,
    }));
    expect(html).toContain("Scheduled prompts");
    expect(html).toContain("Fix login · Nightly check (set by the agent)");
    expect(html).toContain("Next: Needs gh · Last: Not run yet");
    expect(html.match(/role="switch"/gu)).toHaveLength(2);
    expect(html.match(/>Delete</gu)).toHaveLength(1);
    expect(renderToStaticMarkup(createElement(ThreadSchedulesSettings, {
      rows: [], locked: false, error: null, onEnable: noop, onDelete: noop,
    }))).toContain("None yet. Schedule one");
  });

  it("folds a scheduled prompt in the transcript like a handoff, with its own glyph", () => {
    const html = renderToStaticMarkup(createElement(ThreadHandoffPrompt, {
      line: "Scheduled prompt · Nightly check", text: "Check the build.", icon: "watch",
    }));
    expect(html).toContain("codicon-watch");
    expect(html).toContain("Scheduled prompt · Nightly check");
    expect(html).toContain("What was sent");
  });
});
