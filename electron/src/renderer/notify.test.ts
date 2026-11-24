import { afterEach, describe, it, expect, vi } from "vitest";
import {
  DEFAULT_NOTIFICATION_PREFERENCES, LAST_NOTIFIED_LIMIT, notificationPlan, postNotification,
  rememberNotified, shouldNotify, terminalsNewlyCompleted, terminalsNewlyWaiting,
} from "./notify";
import type { SessionKey } from "../shared/engine";

const key: SessionKey = "claude#s1";

describe("shouldNotify", () => {
  it("notifies when the user is not watching that session and the id is unseen", () => {
    expect(shouldNotify({}, key, "ap1", false)).toBe(true);
  });

  it("never notifies twice for the same event id", () => {
    const lastNotified: Record<SessionKey, string> = {};
    expect(shouldNotify(lastNotified, key, "ap1", false)).toBe(true);
    lastNotified[key] = "ap1";
    expect(shouldNotify(lastNotified, key, "ap1", false)).toBe(false);
  });

  it("notifies again for a new event id in the same session", () => {
    expect(shouldNotify({ [key]: "ap1" }, key, "ap2", false)).toBe(true);
  });

  it("never notifies when the user is already watching that session", () => {
    expect(shouldNotify({}, key, "ap1", true)).toBe(false);
    expect(shouldNotify({ [key]: "ap1" }, key, "ap2", true)).toBe(false);
  });

  it("tracks approvals per session key", () => {
    const lastNotified: Record<SessionKey, string> = { "claude#s1": "ap1" };
    expect(shouldNotify(lastNotified, "claude#s1", "ap1", false)).toBe(false);
    expect(shouldNotify(lastNotified, "claude#s2", "ap1", false)).toBe(true);
  });
});

describe("rememberNotified", () => {
  it("records the event and keeps every other session's", () => {
    const next = rememberNotified({ "claude#s2": "ap9" }, key, "ap1");
    expect(next).toEqual({ "claude#s2": "ap9", [key]: "ap1" });
    expect(shouldNotify(next, key, "ap1", false)).toBe(false);
    expect(shouldNotify(next, "claude#s2", "ap9", false)).toBe(false);
  });

  it("keeps the most recently notified sessions and drops the oldest beyond the cap", () => {
    let map: Record<SessionKey, string> = {};
    for (let index = 0; index < 5; index++) map = rememberNotified(map, `claude#${index}`, `done:${index}`, 3);
    expect(Object.keys(map)).toEqual(["claude#2", "claude#3", "claude#4"]);
    // Notifying an older session again makes it the most recent, not the next to go.
    map = rememberNotified(map, "claude#2", "done:5", 3);
    map = rememberNotified(map, "claude#6", "done:6", 3);
    expect(map).toEqual({ "claude#4": "done:4", "claude#2": "done:5", "claude#6": "done:6" });
  });

  it("bounds the stored map by default", () => {
    let map: Record<SessionKey, string> = {};
    for (let index = 0; index < LAST_NOTIFIED_LIMIT + 50; index++) {
      map = rememberNotified(map, `codex#${index}`, `done:${index}`);
    }
    expect(Object.keys(map)).toHaveLength(LAST_NOTIFIED_LIMIT);
    expect(map[`codex#${LAST_NOTIFIED_LIMIT + 49}`]).toBe(`done:${LAST_NOTIFIED_LIMIT + 49}`);
    expect(map["codex#0"]).toBeUndefined();
  });
});

describe("terminal completion delivery", () => {
  it("observes each completion once, including batched turns and restarted panes", () => {
    const terminal = { id: "pane", generation: 1, completedTurns: 0 };
    const initial = terminalsNewlyCompleted(new Map(), [terminal]);
    expect(initial.completed).toEqual([]);
    const done = { ...terminal, completedTurns: 2 };
    const first = terminalsNewlyCompleted(initial.index, [done]);
    expect(first.completed).toEqual([done]);
    expect(terminalsNewlyCompleted(first.index, [done]).completed).toEqual([]);
    expect(terminalsNewlyCompleted(first.index, [{ ...terminal, generation: 2 }]).completed).toEqual([]);
    expect(terminalsNewlyCompleted(first.index, [{ ...done, generation: 2 }]).completed).toHaveLength(1);
    expect(terminalsNewlyCompleted(first.index, []).index.size).toBe(0);
  });
});

describe("notification permission delivery", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("delivers the pending alert after permission is granted, with its click destination", async () => {
    let grant!: (permission: NotificationPermission) => void;
    const notes: FakeNotification[] = [];
    class FakeNotification {
      static permission: NotificationPermission = "default";
      static requestPermission = () => new Promise<NotificationPermission>(resolve => { grant = resolve; });
      onclick: (() => void) | null = null;
      constructor(readonly title: string, readonly options: NotificationOptions) { notes.push(this); }
    }
    vi.stubGlobal("Notification", FakeNotification);
    const clicked = vi.fn();
    postNotification("Finished", "Background pane", clicked);
    expect(notes).toHaveLength(0);
    grant("granted");
    await Promise.resolve();
    expect(notes).toHaveLength(1);
    expect(notes[0].options.body).toBe("Background pane");
    notes[0].onclick?.();
    expect(clicked).toHaveBeenCalledOnce();
  });

  it("uses the notification's own sound, so Focus and permission decide, and no chime (review D5)", () => {
    const notes: { options: NotificationOptions }[] = [];
    class GrantedNotification {
      static permission: NotificationPermission = "granted";
      static requestPermission = vi.fn();
      onclick: (() => void) | null = null;
      constructor(readonly title: string, readonly options: NotificationOptions) { notes.push(this); }
    }
    const audio = vi.fn();
    vi.stubGlobal("Notification", GrantedNotification);
    vi.stubGlobal("AudioContext", class { constructor() { audio(); } });
    postNotification("Finished", "Pane", undefined,
      notificationPlan(DEFAULT_NOTIFICATION_PREFERENCES, "finished"));
    expect(notes[0]?.options.silent).toBe(false);
    postNotification("Finished", "Pane", undefined,
      notificationPlan({ ...DEFAULT_NOTIFICATION_PREFERENCES, sound: false }, "finished"));
    expect(notes[1]?.options.silent).toBe(true);
    expect(audio).not.toHaveBeenCalled();
  });

  it("posts nothing for a kind the owner turned off", () => {
    const construct = vi.fn();
    class GrantedNotification {
      static permission: NotificationPermission = "default";
      static requestPermission = vi.fn(() => Promise.resolve("granted" as NotificationPermission));
      constructor() { construct(); }
    }
    vi.stubGlobal("Notification", GrantedNotification);
    const prefs = { sound: true, finished: false, needsInput: true };
    expect(notificationPlan(prefs, "finished").show).toBe(false);
    expect(notificationPlan(prefs, "needs-input").show).toBe(true);
    postNotification("Finished", "Pane", undefined, notificationPlan(prefs, "finished"));
    expect(GrantedNotification.requestPermission).not.toHaveBeenCalled();
    expect(construct).not.toHaveBeenCalled();
    expect(notificationPlan({ ...prefs, needsInput: false }, "needs-input").show).toBe(false);
  });

  it("does not request or construct a notification when permission is denied", () => {
    const construct = vi.fn();
    class DeniedNotification {
      static permission = "denied";
      static requestPermission = vi.fn();
      constructor() { construct(); }
    }
    const audio = vi.fn();
    vi.stubGlobal("Notification", DeniedNotification);
    vi.stubGlobal("AudioContext", class { constructor() { audio(); } });
    postNotification("Finished", "Background pane");
    expect(DeniedNotification.requestPermission).not.toHaveBeenCalled();
    expect(construct).not.toHaveBeenCalled();
    // Refused permission is silence too: nothing else plays a sound.
    expect(audio).not.toHaveBeenCalled();
  });
});

interface Pane {
  readonly id: string;
  readonly generation: number;
  readonly state: string;
  readonly activity?: string;
}

const pane = (over: Partial<Pane> = {}): Pane => ({
  id: "t1", generation: 1, state: "running", activity: "waiting", ...over,
});

describe("terminalsNewlyWaiting", () => {
  it("returns a running pane that just flipped to waiting and records it", () => {
    const { waiting, index } = terminalsNewlyWaiting(new Map(), [pane()]);
    expect(waiting.map(terminal => terminal.id)).toEqual(["t1"]);
    expect(index.get("t1:1")).toBe("waiting");
  });

  it("never returns the same pane twice while it stays waiting", () => {
    const first = terminalsNewlyWaiting(new Map(), [pane()]);
    expect(first.waiting).toHaveLength(1);
    expect(terminalsNewlyWaiting(first.index, [pane()]).waiting).toEqual([]);
  });

  it("never returns a pane that is no longer alive", () => {
    expect(terminalsNewlyWaiting(new Map(), [pane({ state: "exited" })]).waiting).toEqual([]);
    expect(terminalsNewlyWaiting(new Map(), [pane({ state: "failed" })]).waiting).toEqual([]);
    expect(terminalsNewlyWaiting(new Map(), [pane({ state: "starting" })]).waiting)
      .toHaveLength(1);
  });

  it("rings again after a restart, because a new generation is a new pane", () => {
    const first = terminalsNewlyWaiting(new Map(), [pane()]);
    const { waiting, index } = terminalsNewlyWaiting(first.index, [pane({ generation: 2 })]);
    expect(waiting.map(terminal => terminal.generation)).toEqual([2]);
    expect(index.get("t1:2")).toBe("waiting");
  });

  it("never returns a pane first seen idle or without activity", () => {
    const { waiting, index } = terminalsNewlyWaiting(new Map(), [
      pane({ activity: "idle" }),
      pane({ id: "t2", activity: undefined }),
      pane({ id: "t3", activity: "working" }),
    ]);
    expect(waiting).toEqual([]);
    expect(index.get("t1:1")).toBe("idle");
    expect(index.get("t2:1")).toBeUndefined();
    expect(index.has("t2:1")).toBe(true);
  });
});
