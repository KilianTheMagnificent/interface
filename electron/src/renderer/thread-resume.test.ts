import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import fixtureV66 from "../fixtures/threads-resume-v66.json";
import type { ThreadScheduleView, ThreadsView } from "../shared/threads";
import { ThreadOpinionCards } from "./components/ThreadOpinionCards";
import { ThreadScheduleLine } from "./components/ThreadScheduleLine";
import type { EngineEvent } from "../shared/engine";
import {
  NO_THREAD_OFFERS, latestRateLimitNotice, resumeQueuedPrompt, threadLimitNotices, threadLimitOffer,
  threadLimitReset, type ThreadLimitOffer,
} from "./thread-opinion";
import {
  resumeLineText, scheduleComposerLine, scheduleSettingsRows, scheduledPromptFold,
} from "./thread-schedules";

// Resume at reset (T3 section 10, protocol 66): the limit line's two new
// actions, the resume on the line above the composer, its fold in the
// transcript and its Settings row. Rust decides the reset, the time and the
// prompt; these are words and layout over its view. Times are built in the
// Mac's own zone, as the renderer shows them.
const NOW = new Date(2026, 10, 15, 14, 0).getTime();
const at = (day: number, hour: number, minute = 0): number => new Date(2026, 10, day, hour, minute).getTime();
const RESET = at(15, 15, 10);
const noop = (): void => undefined;

function resume(fields: Partial<ThreadScheduleView> = {}): ThreadScheduleView {
  return {
    id: "s-r", prompt: "Continue from where you stopped.", when: { kind: "once", at: RESET + 60_000 },
    enabled: true, createdAt: NOW, nextAt: RESET + 60_000, setBy: "owner", paused: false, kind: "resume",
    ...fields,
  };
}

describe("the limit line's Resume at reset and Snooze until reset", () => {
  it("offers both once the reset is known, and neither for an unknown or past reset", () => {
    const resetsAt = new Date(RESET).toISOString();
    expect(threadLimitReset({ resetsAt }, "claude", undefined, undefined, NOW)).toEqual({
      resetsAt: RESET, message: "Claude Code hit its usage limit; it resets in 1h 10m.", resume: true, snooze: true,
    });
    expect(threadLimitReset({}, "claude", undefined, undefined, NOW)).toBeNull();
    expect(threadLimitReset(undefined, "claude", undefined, undefined, NOW)).toBeNull();
    expect(threadLimitReset({ resetsAt: "soon" }, "claude", undefined, undefined, NOW)).toBeNull();
    expect(threadLimitReset({ resetsAt: new Date(NOW - 1).toISOString() }, "claude", undefined, undefined, NOW))
      .toBeNull();
    // A resume already set for this reset: the schedule line says it instead.
    const set = threadLimitReset({ resetsAt }, "claude", { schedules: [resume()] }, undefined, NOW);
    expect(set?.resume).toBe(false);
    // One set for an earlier reset, or one that already ran, is offered again.
    const earlier = threadLimitReset({ resetsAt }, "claude",
      { schedules: [resume({ nextAt: RESET - 3_600_000 })] }, undefined, NOW);
    expect(earlier?.resume).toBe(true);
    const ran = threadLimitReset({ resetsAt }, "claude",
      { schedules: [resume({ enabled: false, nextAt: undefined })] }, undefined, NOW);
    expect(ran?.resume).toBe(true);
    // Snoozed until then already: no second snooze.
    expect(threadLimitReset({ resetsAt }, "claude", undefined, RESET, NOW)?.snooze).toBe(false);
    expect(threadLimitReset({ resetsAt }, "claude", undefined, RESET - 1, NOW)?.snooze).toBe(true);
  });

  it("words the switch offer's reset from Rust's limit before the notice's own text", () => {
    const offers = { opinion: [], switchTo: [{ engine: "codex" as const, label: "Codex" }] };
    const offer = threadLimitOffer("Rate limit reached", "claude", offers, {}, NOW, undefined,
      new Date(RESET).toISOString());
    expect(offer?.message).toBe("Claude Code hit its usage limit; it resets in 1h 10m.");
    expect(threadLimitOffer("Rate limit reached", "claude", offers, {}, NOW)?.message)
      .toBe("Claude Code hit its usage limit.");
    expect(threadLimitOffer("Rate limit reached", "claude", NO_THREAD_OFFERS, {}, NOW)).toBeNull();
  });

  it("offers Switch to on a Claude limit that its turn's result already left out of the event log", () => {
    // What engine-claude maps from a limited turn: the rejected rate_limit_event,
    // then the error result's token usage, which names no limit.
    const key = "claude#limited";
    const log = [
      { kind: "usage.updated", inputTokens: 0, outputTokens: 0,
        rateLimited: { message: "Rate limit reached", resetsAt: new Date(RESET).toISOString() } },
      { kind: "usage.updated", inputTokens: 0, outputTokens: 0 },
    ] as EngineEvent[];
    const rateLimits: Record<string, string> = {};
    const notice = latestRateLimitNotice(log);
    if (notice !== undefined) rateLimits[key] = notice;
    // Rust still holds the limit (ThreadView.limit) until a run completes.
    const notices = threadLimitNotices(rateLimits,
      [{ key, limit: { resetsAt: new Date(RESET).toISOString() } }, { key: "claude#fine" },
        { key: "claude#unknown", limit: {} }, { key: "claude#past", limit: { resetsAt: new Date(NOW - 1).toISOString() } }],
      NOW);
    expect(Object.keys(notices)).toEqual([key, "claude#unknown"]);
    const offers = { opinion: [], switchTo: [{ engine: "acp:grok" as const, label: "Grok" }] };
    const offer = threadLimitOffer(notices[key], "claude", offers, {}, NOW, undefined,
      new Date(RESET).toISOString());
    expect(offer).toEqual({ engine: "acp:grok", label: "Grok",
      message: "Claude Code hit its usage limit; it resets in 1h 10m." });
  });

  it("draws Resume at reset and Snooze until reset beside Switch to, and alone when there is nowhere to go", () => {
    const reset = threadLimitReset({ resetsAt: new Date(RESET).toISOString() }, "claude", undefined, undefined, NOW);
    const snoozed: number[] = [];
    const props = {
      cards: [], pending: [], onAction: noop, onSwitch: noop, reset,
      onResume: noop, onSnooze: (until: number) => { snoozed.push(until); },
    };
    const alone = renderToStaticMarkup(createElement(ThreadOpinionCards, { ...props, limit: null }));
    expect(alone).toContain("Claude Code hit its usage limit; it resets in 1h 10m.");
    expect(alone).toContain(">Resume at reset</button>");
    expect(alone).toContain(">Snooze until reset</button>");
    expect(alone).not.toContain("Switch to");
    const limit: ThreadLimitOffer = { engine: "codex", label: "Codex", message: "Claude Code hit its usage limit; it resets in 1h 10m." };
    const beside = renderToStaticMarkup(createElement(ThreadOpinionCards, { ...props, limit }));
    expect(beside.indexOf("Switch to Codex")).toBeGreaterThan(-1);
    expect(beside.indexOf("Switch to Codex")).toBeLessThan(beside.indexOf("Resume at reset"));
    // A resume already set: only Snooze; neither without a known reset.
    const set = renderToStaticMarkup(createElement(ThreadOpinionCards, {
      ...props, limit, reset: reset === null ? null : { ...reset, resume: false },
    }));
    expect(set).not.toContain("Resume at reset");
    expect(set).toContain("Snooze until reset");
    const unknown = renderToStaticMarkup(createElement(ThreadOpinionCards, { ...props, limit, reset: null }));
    expect(unknown).not.toContain("Resume at reset");
    expect(unknown).not.toContain("Snooze until reset");
    expect(renderToStaticMarkup(createElement(ThreadOpinionCards, { ...props, limit: null, reset: null }))).toBe("");
  });

  it("takes the queue's first row only when it never left and has no attachments", () => {
    expect(resumeQueuedPrompt([{ id: "q1", text: "Then run the tests.", deliveryState: "paused" },
      { id: "q2", text: "And deploy." }])).toEqual({ id: "q1", text: "Then run the tests." });
    expect(resumeQueuedPrompt([{ id: "q1", text: "Then run the tests." }]))
      .toEqual({ id: "q1", text: "Then run the tests." });
    // In flight: its delivery is unknown, so neither it nor a row behind it goes.
    expect(resumeQueuedPrompt([{ id: "q1", text: "Delete it.", deliveryState: "inflight" },
      { id: "q2", text: "And deploy." }])).toBeNull();
    expect(resumeQueuedPrompt([{ id: "q1", text: "Read this.",
      context: [{ id: "c", kind: "file", label: "a.md", value: "/p/a.md" }] }])).toBeNull();
    expect(resumeQueuedPrompt([{ id: "q1", text: "   " }])).toBeNull();
    expect(resumeQueuedPrompt([])).toBeNull();
  });
});

describe("a resume on the line above the composer, in the transcript and in Settings", () => {
  it("reads Resumes at <time> · Cancel, without Edit", () => {
    expect(resumeLineText(resume(), NOW)).toBe("Resumes at 15:11");
    expect(resumeLineText(resume({ nextAt: at(16, 7) }), NOW)).toBe("Resumes Tomorrow 07:00");
    expect(resumeLineText(resume({ waiting: RESET + 60_000 }), NOW)).toBe("Resumes when the turn ends");
    const line = scheduleComposerLine({ schedules: [resume()] }, NOW);
    if (line === null) throw new Error("no line");
    const cancelled: string[] = [];
    const html = renderToStaticMarkup(createElement(ThreadScheduleLine, {
      line, now: NOW, onEdit: noop, onCancel: (id: string) => { cancelled.push(id); },
    }));
    expect(html).toContain("Resumes at 15:11");
    expect(html).toContain(">Cancel</button>");
    expect(html).not.toContain("Edit");
    expect(html).not.toContain("Next:");
    expect(html).toContain('data-thread-resume=""');
  });

  it("folds the resume's prompt to one line with what was sent under it", () => {
    expect(scheduledPromptFold("[Interface] Resumed after the usage limit reset:\nContinue from where you stopped."))
      .toEqual({ line: "Resumed after the limit reset", body: "Continue from where you stopped." });
    expect(scheduledPromptFold("[Interface] Resumed after the usage limit reset")).toBeNull();
  });

  it("lists the shared fixture's resume in Settings as Resume at reset", () => {
    const view = fixtureV66.view as unknown as ThreadsView;
    const rows = scheduleSettingsRows(view, key => key, view.now);
    expect(rows.map(row => row.what)).toEqual(["Resume at reset"]);
  });
});
